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
/// plus what `/proc` says that process is running.
///
/// The uid is checked before any request is read; the rest is for daemons
/// that want to know *which* of the user's programs is asking (Opal binds
/// paired local apps to their executable). Any process of the same user can
/// exec the real program, so this is a second lock on the door, not a wall.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Peer {
    pub uid: u32,
    pub pid: Option<i32>,
    /// `/proc/<pid>/exe`, read right after accept; `None` if unreadable.
    pub exe: Option<PathBuf>,
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
        Self { uid, pid, exe }
    }
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
}

/// Serve `svc` on `path` until the listener fails. `name` is used in the
/// "already running" message.
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
    loop {
        let (stream, _) = listener.accept().await?;
        let peer = match stream.peer_cred() {
            Ok(cred) if cred.uid() == uid => Peer::for_pid(uid, cred.pid()),
            _ => {
                tracing::warn!("rejected a connection from another user");
                continue;
            }
        };
        let svc = svc.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(svc, stream, peer).await {
                tracing::debug!("client disconnected: {e}");
            }
        });
    }
}

async fn handle<S: Service>(svc: Arc<S>, stream: UnixStream, peer: Peer) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<String>(64);

    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if write.write_all(line.as_bytes()).await.is_err()
                || write.write_all(b"\n").await.is_err()
            {
                break;
            }
        }
    });

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
                spawn_forwarder(svc.events(), tx.clone());
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
    drop(tx);
    writer.await.ok();
    Ok(())
}

fn spawn_forwarder(mut events: broadcast::Receiver<IpcEvent>, tx: mpsc::Sender<String>) {
    tokio::spawn(async move {
        loop {
            let ev: IpcEvent = match events.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let Ok(line) = serde_json::to_string(&ev) else {
                continue;
            };
            if tx.send(line).await.is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_reads_its_own_executable() {
        let uid = rustix::process::geteuid().as_raw();
        let me = Peer::for_pid(uid, Some(std::process::id() as i32));
        assert_eq!(me.exe, std::env::current_exe().ok());
        assert_eq!(Peer::for_pid(uid, None).exe, None);
    }
}
