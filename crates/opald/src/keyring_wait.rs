//! Starting while the keyring is locked.
//!
//! Opal's items live in the Secret Service's default collection. When that
//! isn't the login keyring PAM unlocks at login, it is still locked when
//! opald starts, and every read fails with `IsLocked`. Rather than exit
//! (and crash-loop into systemd's start limit), opald asks the Secret
//! Service to unlock it, which shows the keyring's own prompt, and waits.
//!
//! Meanwhile the socket answers with `{"keyring_locked": true}` so the bar
//! can say why Opal isn't ready, and `keyring.unlock` shows the prompt again
//! after it was dismissed. Nothing else is served: there is no vault yet.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use futures::future::BoxFuture;
use opal_core::ipc::IpcEvent;
use opal_core::keystore::SecretStore;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

/// How often to look again while waiting (unlocking it elsewhere, say).
const POLL: Duration = Duration::from_secs(2);

pub const LOCKED: &str = "your keyring is locked; unlock it to start Opal";

/// Return once `store` is unlocked, serving the waiting socket on `socket`
/// until then. Returns at once if it isn't locked.
pub async fn wait_until_unlocked(store: &SecretStore, socket: &Path) -> Result<()> {
    if !store.is_locked().await? {
        return Ok(());
    }
    tracing::warn!("the keyring is locked; asking to unlock it and waiting");
    let (unlock_tx, mut unlock_rx) = mpsc::channel(1);
    let svc = Arc::new(Waiting {
        unlock: unlock_tx,
        events: broadcast::channel(1).0,
    });
    let server = opal_kit::ipc::serve(svc, socket, "opald");
    tokio::pin!(server);

    // Prompt once on start; again only when asked from the bar.
    let mut prompt: Option<BoxFuture<'_, opal_core::Result<()>>> = Some(Box::pin(store.unlock()));
    loop {
        tokio::select! {
            r = server.as_mut() => {
                r?;
                bail!("the control socket closed");
            }
            r = async { prompt.as_mut().expect("guarded").await }, if prompt.is_some() => {
                prompt = None;
                if let Err(e) = r {
                    tracing::warn!("the keyring is still locked: {e}");
                }
            }
            Some(()) = unlock_rx.recv() => {
                if prompt.is_none() {
                    prompt = Some(Box::pin(store.unlock()));
                }
            }
            _ = tokio::time::sleep(POLL) => {}
        }
        if !store.is_locked().await? {
            break;
        }
    }
    tracing::info!("the keyring is unlocked");
    // Dropping the server closes every connection, so the shell reconnects
    // to the real one; the stale socket file is replaced by `serve`.
    Ok(())
}

struct Waiting {
    unlock: mpsc::Sender<()>,
    /// Never sent on: there's nothing to report until the real start.
    events: broadcast::Sender<IpcEvent>,
}

fn snapshot() -> Value {
    json!({ "keyring_locked": true })
}

impl opal_kit::ipc::Service for Waiting {
    fn dispatch(
        self: Arc<Self>,
        method: String,
        _params: Value,
    ) -> BoxFuture<'static, Result<Value>> {
        Box::pin(async move {
            match method.as_str() {
                "ping" => Ok(json!("pong")),
                "status" => Ok(snapshot()),
                // Any program of the user's could ask the Secret Service for
                // this prompt itself; it carries nothing of Opal's.
                "keyring.unlock" => {
                    let _ = self.unlock.try_send(());
                    Ok(json!(true))
                }
                _ => bail!(LOCKED),
            }
        })
    }

    fn snapshot(self: Arc<Self>) -> BoxFuture<'static, Value> {
        Box::pin(async { snapshot() })
    }

    fn events(&self) -> broadcast::Receiver<IpcEvent> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_kit::ipc::Service;

    #[tokio::test]
    async fn only_reports_the_lock_and_asks_to_unlock() {
        let (tx, mut rx) = mpsc::channel(1);
        let w = Arc::new(Waiting {
            unlock: tx,
            events: broadcast::channel(1).0,
        });
        let s = w
            .clone()
            .dispatch("status".into(), Value::Null)
            .await
            .unwrap();
        assert_eq!(s["keyring_locked"], true);
        for m in ["accounts.list", "unlock", "sign", "config.get"] {
            let e = w.clone().dispatch(m.into(), Value::Null).await.unwrap_err();
            assert_eq!(e.to_string(), LOCKED, "{m}");
        }
        w.clone()
            .dispatch("keyring.unlock".into(), Value::Null)
            .await
            .unwrap();
        assert_eq!(rx.try_recv(), Ok(()));
    }

    #[tokio::test]
    async fn an_unlocked_store_starts_at_once() {
        let dir = std::env::temp_dir().join(format!("opal-kw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("opal.sock");
        wait_until_unlocked(&SecretStore::memory(), &sock)
            .await
            .unwrap();
        assert!(!sock.exists(), "nothing is served when there's no wait");
        std::fs::remove_dir_all(&dir).ok();
    }
}
