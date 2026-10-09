//! The control socket: newline-delimited JSON over a Unix socket that only
//! the current user can reach. Each daemon plugs in its own [`Service`].

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures::future::BoxFuture;
use opal_core::ipc::{IpcEvent, IpcRequest, IpcResponse};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use zeroize::Zeroize;

/// Longest request line we accept (a pasted key or URI fits easily).
const MAX_LINE: usize = 64 * 1024;

/// Who is on the other end of a connection: the socket's peer credentials
/// plus what `/proc` says about that process.
///
/// The uid is checked before any request is read; the rest is for daemons
/// that want to know *which* of the user's programs is asking (Opal binds
/// paired local apps to it). The systemd unit comes from the process's
/// cgroup, which systemd sets and any reader can see. The executable path
/// is only readable with ptrace rights over the peer, which a sandboxed
/// daemon doesn't have over another user service, so it's often `None`.
/// Any process of the same user can start the real program, so this is a
/// second lock on the door, not a wall.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Peer {
    pub uid: u32,
    pub pid: Option<i32>,
    /// `/proc/<pid>/exe`, read right after accept; `None` if unreadable.
    pub exe: Option<PathBuf>,
    /// The systemd unit (`peridot.service`, or an app scope) the process
    /// runs in, from `/proc/<pid>/cgroup`; `None` outside systemd.
    pub unit: Option<String>,
    /// State a daemon keeps per connection, shared with the event forwarder
    /// (opald: whether the caller has shown the UI session token).
    #[serde(skip)]
    pub session: Arc<Session>,
}

/// Per-connection state. `privileged` means whatever the daemon says it does.
#[derive(Debug, Default)]
pub struct Session {
    privileged: std::sync::atomic::AtomicBool,
}

impl Session {
    pub fn is_privileged(&self) -> bool {
        self.privileged.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn set_privileged(&self) {
        self.privileged
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Peer {
    /// Look the process up in `/proc` now, before its pid can be reused.
    pub fn for_pid(uid: u32, pid: Option<i32>) -> Self {
        let exe = pid.and_then(|pid| {
            std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .map(|p| match p.to_str() {
                    // A binary replaced by an upgrade while it runs shows as
                    // deleted; it is still the same program at that path.
                    Some(s) if s.ends_with(" (deleted)") => {
                        PathBuf::from(s.trim_end_matches(" (deleted)"))
                    }
                    _ => p,
                })
        });
        let unit = pid
            .and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok())
            .and_then(|s| unit_from_cgroup(&s));
        Self {
            uid,
            pid,
            exe,
            unit,
            session: Arc::new(Session::default()),
        }
    }
}

/// The innermost systemd unit in a `/proc/<pid>/cgroup` listing (cgroup v2:
/// one `0::/path` line).
fn unit_from_cgroup(s: &str) -> Option<String> {
    let path = s.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    path.rsplit('/')
        .find(|seg| seg.ends_with(".service") || seg.ends_with(".scope"))
        .map(String::from)
}

/// What a daemon exposes on its socket.
pub trait Service: Send + Sync + 'static {
    /// Handle one request.
    fn dispatch(
        self: Arc<Self>,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, Result<Value>>;
    /// Handle one request, knowing who sent it. Daemons that don't care
    /// about the caller get the plain [`Service::dispatch`].
    fn dispatch_with_peer(
        self: Arc<Self>,
        _peer: Peer,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, Result<Value>> {
        self.dispatch(method, params)
    }
    /// The full state, sent in reply to `subscribe`.
    fn snapshot(self: Arc<Self>) -> BoxFuture<'static, Value>;
    /// Events pushed to subscribed clients.
    fn events(&self) -> broadcast::Receiver<IpcEvent>;
    /// What of an event a connection may see: the event, a redacted copy,
    /// or nothing. Daemons that don't care pass everything on.
    fn filter_event(&self, _peer: &Peer, event: &IpcEvent) -> Option<IpcEvent> {
        Some(event.clone())
    }
}

/// Serve `svc` on `path` until the listener fails. `name` is used in the
/// "already running" message. Dropping the future closes the listener and
/// every open connection with it, so clients notice and reconnect.
pub async fn serve<S: Service>(svc: Arc<S>, path: &Path, name: &str) -> Result<()> {
    if path.exists() {
        // Refuse to start twice; a stale socket from a crash is replaced.
        if UnixStream::connect(path).await.is_ok() {
            bail!("{name} is already running ({})", path.display());
        }
        std::fs::remove_file(path).ok();
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("listening on {}", path.display());

    let uid = rustix::process::geteuid().as_raw();
    let mut conns = tokio::task::JoinSet::new();
    loop {
        let stream = tokio::select! {
            r = listener.accept() => r?.0,
            Some(_) = conns.join_next() => continue,
        };
        let peer = match stream.peer_cred() {
            Ok(cred) if cred.uid() == uid => Peer::for_pid(uid, cred.pid()),
            _ => {
                tracing::warn!("rejected a connection from another user");
                continue;
            }
        };
        let svc = svc.clone();
        conns.spawn(async move {
            if let Err(e) = handle(svc, stream, peer).await {
                tracing::debug!("client disconnected: {e}");
            }
        });
    }
}

async fn handle<S: Service>(svc: Arc<S>, stream: UnixStream, peer: Peer) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<String>(64);

    let mut writer = AbortOnDrop(tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if write.write_all(line.as_bytes()).await.is_err()
                || write.write_all(b"\n").await.is_err()
            {
                break;
            }
        }
    }));
    let mut forwarder = None;

    let mut reader = BufReader::new(read);
    let mut line = String::new();
    let mut subscribed = false;
    loop {
        line.zeroize();
        let n = (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_line(&mut line)
            .await?;
        if n == 0 {
            break;
        }
        if line.len() > MAX_LINE {
            bail!("request too long");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: IpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = IpcResponse {
                    id: 0,
                    result: None,
                    error: Some(format!("bad request: {e}")),
                };
                tx.send(serde_json::to_string(&resp)?).await.ok();
                continue;
            }
        };

        if req.method == "subscribe" {
            if !subscribed {
                subscribed = true;
                forwarder = Some(spawn_forwarder(svc.clone(), peer.clone(), tx.clone()));
            }
            let resp = IpcResponse {
                id: req.id,
                result: Some(svc.clone().snapshot().await),
                error: None,
            };
            if tx.send(serde_json::to_string(&resp)?).await.is_err() {
                break;
            }
            continue;
        }
        // Each request runs on its own, so a slow one (a network lookup, an
        // external signer) doesn't hold up others on the same connection.
        let svc = svc.clone();
        let tx = tx.clone();
        let peer = peer.clone();
        tokio::spawn(async move {
            let resp = match svc.dispatch_with_peer(peer, req.method, req.params).await {
                Ok(v) => IpcResponse {
                    id: req.id,
                    result: Some(v),
                    error: None,
                },
                Err(e) => IpcResponse {
                    id: req.id,
                    result: None,
                    error: Some(format!("{e:#}")),
                },
            };
            if let Ok(line) = serde_json::to_string(&resp) {
                let _ = tx.send(line).await;
            }
        });
    }
    line.zeroize();
    drop(forwarder);
    drop(tx);
    (&mut writer.0).await.ok();
    Ok(())
}

/// A task that ends with the connection, also when [`serve`] is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn spawn_forwarder<S: Service>(svc: Arc<S>, peer: Peer, tx: mpsc::Sender<String>) -> AbortOnDrop {
    let mut events = svc.events();
    AbortOnDrop(tokio::spawn(async move {
        loop {
            let ev: IpcEvent = match events.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let Some(ev) = svc.filter_event(&peer, &ev) else {
                continue;
            };
            let Ok(line) = serde_json::to_string(&ev) else {
                continue;
            };
            if tx.send(line).await.is_err() {
                break;
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_reads_its_own_executable_and_unit() {
        let uid = rustix::process::geteuid().as_raw();
        let me = Peer::for_pid(uid, Some(std::process::id() as i32));
        assert_eq!(me.exe, std::env::current_exe().ok());
        let cgroup = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
        assert_eq!(me.unit, unit_from_cgroup(&cgroup));
        let none = Peer::for_pid(uid, None);
        assert_eq!((none.exe, none.unit), (None, None));
    }

    #[test]
    fn unit_is_the_innermost_service_or_scope() {
        assert_eq!(
            unit_from_cgroup(
                "0::/user.slice/user-1000.slice/user@1000.service/app.slice/peridot.service\n"
            ),
            Some("peridot.service".into())
        );
        assert_eq!(
            unit_from_cgroup(
                "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-x.scope"
            ),
            Some("app-x.scope".into())
        );
        assert_eq!(unit_from_cgroup("0::/"), None);
        assert_eq!(unit_from_cgroup("1:name=systemd:/x.service"), None);
    }

    struct Echo(broadcast::Sender<IpcEvent>);

    impl Service for Echo {
        fn dispatch(
            self: Arc<Self>,
            method: String,
            _: Value,
        ) -> BoxFuture<'static, Result<Value>> {
            Box::pin(async move { Ok(Value::String(method)) })
        }
        fn snapshot(self: Arc<Self>) -> BoxFuture<'static, Value> {
            Box::pin(async { Value::Null })
        }
        fn events(&self) -> broadcast::Receiver<IpcEvent> {
            self.0.subscribe()
        }
    }

    /// opald serves a stand-in while the keyring is locked and drops it
    /// when it unlocks; connected clients (subscribed ones too) must see
    /// the end so they reconnect to the real one.
    #[tokio::test]
    async fn dropping_the_server_closes_its_connections() {
        let dir = std::env::temp_dir().join(format!("opal-ipc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.sock");
        let svc = Arc::new(Echo(broadcast::channel(1).0));
        let at = path.clone();
        let server = tokio::spawn(async move { serve(svc, &at, "test").await });
        let mut client = loop {
            if let Ok(c) = UnixStream::connect(&path).await {
                break BufReader::new(c);
            }
            tokio::task::yield_now().await;
        };
        client
            .get_mut()
            .write_all(b"{\"id\":1,\"method\":\"subscribe\",\"params\":null}\n{\"id\":2,\"method\":\"ping\",\"params\":null}\n")
            .await
            .unwrap();
        let mut line = String::new();
        for _ in 0..2 {
            line.clear();
            client.read_line(&mut line).await.unwrap();
            assert!(line.contains("\"result\""), "{line}");
        }
        server.abort();
        line.clear();
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_line(&mut line),
        )
        .await
        .expect("the connection closes")
        .unwrap();
        assert_eq!(n, 0, "EOF, got {line}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
