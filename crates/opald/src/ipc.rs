//! The control socket: newline-delimited JSON over a Unix socket that only
//! the current user can reach.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use opal_core::ipc::{IpcEvent, IpcRequest, IpcResponse};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use zeroize::Zeroize;

use crate::api;
use crate::app::App;

/// Longest request line we accept (a pasted nsec or URI fits easily).
const MAX_LINE: usize = 64 * 1024;

pub async fn serve(app: Arc<App>, path: &Path) -> Result<()> {
    if path.exists() {
        // Refuse to start twice; a stale socket from a crash is replaced.
        if UnixStream::connect(path).await.is_ok() {
            bail!("opald is already running ({})", path.display());
        }
        std::fs::remove_file(path).ok();
    }
    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("listening on {}", path.display());

    let uid = unsafe_free_uid();
    loop {
        let (stream, _) = listener.accept().await?;
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == uid => {}
            _ => {
                tracing::warn!("rejected a connection from another user");
                continue;
            }
        }
        let app = app.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(app, stream).await {
                tracing::debug!("client disconnected: {e}");
            }
        });
    }
}

/// Our effective uid.
fn unsafe_free_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

async fn handle(app: Arc<App>, stream: UnixStream) -> Result<()> {
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
                spawn_forwarder(&app, tx.clone());
            }
            let resp = IpcResponse {
                id: req.id,
                result: Some(app.status().await),
                error: None,
            };
            if tx.send(serde_json::to_string(&resp)?).await.is_err() {
                break;
            }
            continue;
        }
        // Each request runs on its own, so a slow one (a NIP-05 lookup, an
        // external signer) doesn't hold up approvals on the same connection.
        let app = app.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            let resp = match api::dispatch(&app, &req.method, req.params).await {
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

fn spawn_forwarder(app: &Arc<App>, tx: mpsc::Sender<String>) {
    let mut events = app.events.subscribe();
    tokio::spawn(async move {
        loop {
            let ev: IpcEvent = match events.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
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
