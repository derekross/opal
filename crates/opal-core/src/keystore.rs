//! Secret storage backed by the Secret Service (gnome-keyring, KWallet, …).
//!
//! Opal account keys are only ever written here as NIP-49 `ncryptsec`
//! strings, so a copied keyring file is useless without the Opal passphrase.
//! Peridot's device identity is the exception: it is meant to work without a
//! passphrase, so it is only as safe as the keyring itself. So is the
//! notification engine's copy of your private mutes, kept so they apply
//! while Opal is locked.
//!
//! Items live in the Secret Service's *default* collection. That is usually
//! the login keyring, which PAM unlocks at login, but it can be a separate
//! collection that stays locked until you unlock it: see
//! [`SecretStore::is_locked`] and [`SecretStore::unlock`].

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};
use zeroize::Zeroizing;

use crate::Result;

/// What an item in the store holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    /// A user account, secret = `ncryptsec1…`.
    Account,
    /// A per-connection NIP-46 transport key, secret = hex secret key.
    ConnKey,
    /// Our client key for an external bunker, secret = hex secret key.
    ClientKey,
    /// Peridot's passwordless identity, secret = hex secret key.
    DeviceIdentity,
    /// Peridot's random key for encrypting synced data, secret = hex.
    SyncSecret,
    /// A local app's pairing token for opald (Peridot), secret = hex.
    AppToken,
    /// An account's decrypted private mutes, secret = JSON (see opal-notify).
    MuteCache,
}

impl ItemKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::ConnKey => "conn-key",
            Self::ClientKey => "client-key",
            Self::DeviceIdentity => "device-identity",
            Self::SyncSecret => "sync-secret",
            Self::AppToken => "app-token",
            Self::MuteCache => "mute-cache",
        }
    }
}

pub enum SecretStore {
    /// Items tagged `application = <app>`.
    Keyring {
        /// The Secret Service client. Its session lives in the daemon, so a
        /// dead one is swapped for a fresh client on the first failed read
        /// (see `attempt`).
        client: RwLock<Arc<oo7::Keyring>>,
        app: &'static str,
    },
    /// In-process store for tests.
    Memory(Mutex<BTreeMap<(ItemKind, String), Zeroizing<String>>>),
}

impl SecretStore {
    /// Opal's items in the default keyring.
    pub async fn keyring() -> Result<Self> {
        Self::keyring_for(crate::paths::AppDirs::OPAL).await
    }

    /// Another app's items in the default keyring.
    pub async fn keyring_for(app: crate::paths::AppDirs) -> Result<Self> {
        Ok(Self::Keyring {
            client: RwLock::new(Arc::new(oo7::Keyring::new().await?)),
            app: app.name,
        })
    }

    pub fn memory() -> Self {
        Self::Memory(Mutex::new(BTreeMap::new()))
    }

    /// Whether the collection is locked: nothing can be read or written
    /// until it's unlocked (`org.freedesktop.Secret.Error.IsLocked`).
    pub async fn is_locked(&self) -> Result<bool> {
        match self {
            Self::Keyring { client, .. } => {
                Self::attempt(client, |k| async move { k.is_locked().await }).await
            }
            Self::Memory(_) => Ok(false),
        }
    }

    /// Ask the Secret Service to unlock the collection. It shows its own
    /// password prompt, and this waits until that's answered; dismissing
    /// it is an error.
    pub async fn unlock(&self) -> Result<()> {
        match self {
            Self::Keyring { client, .. } => {
                Self::attempt(client, |k| async move { k.unlock().await }).await
            }
            Self::Memory(_) => Ok(()),
        }
    }

    /// Run one Secret Service operation, reconnecting once if the stored
    /// client no longer has a session with the daemon.
    ///
    /// The session lives in the daemon, not in the client: when gnome-keyring
    /// crashes or is restarted, the session path the client holds is gone and
    /// every read fails with it. opald runs for weeks at a time, so a dead
    /// session has to heal itself — the first failed operation builds a new
    /// client and tries once more, and only a second failure is reported.
    async fn attempt<T, F, Fut>(client: &RwLock<Arc<oo7::Keyring>>, op: F) -> Result<T>
    where
        F: Fn(Arc<oo7::Keyring>) -> Fut,
        Fut: Future<Output = oo7::Result<T>>,
    {
        let cached = client.read().await.clone();
        match op(cached).await {
            Ok(value) => Ok(value),
            // A lock, a dismissed prompt, a missing item: states the user or
            // the data is in, not a dead session. A new client would hit the
            // same wall, so only a lost session is worth reconnecting for.
            Err(e) if !session_gone(&e) => Err(e.into()),
            Err(first) => {
                tracing::warn!("the secret service session is gone ({first}); reconnecting");
                let fresh = Arc::new(oo7::Keyring::new().await?);
                *client.write().await = fresh.clone();
                Ok(op(fresh).await?)
            }
        }
    }

    fn attributes(app: &str, kind: ItemKind, id: &str) -> HashMap<&'static str, String> {
        HashMap::from([
            ("application", app.to_string()),
            ("kind", kind.as_str().to_string()),
            ("id", id.to_string()),
        ])
    }

    pub async fn put(&self, kind: ItemKind, id: &str, label: &str, secret: &str) -> Result<()> {
        match self {
            Self::Keyring { client, app } => {
                let attributes = Self::attributes(app, kind, id);
                Self::attempt(client, |k| {
                    let attributes = attributes.clone();
                    async move {
                        k.create_item(label, &attributes, secret, true)
                            .await
                            .map(drop)
                    }
                })
                .await?
            }
            Self::Memory(m) => {
                m.lock()
                    .await
                    .insert((kind, id.to_string()), Zeroizing::new(secret.to_string()));
            }
        }
        Ok(())
    }

    pub async fn get(&self, kind: ItemKind, id: &str) -> Result<Option<Zeroizing<String>>> {
        match self {
            Self::Keyring { client, app } => {
                let attributes = Self::attributes(app, kind, id);
                Self::attempt(client, |k| {
                    let attributes = attributes.clone();
                    async move {
                        match k.search_items(&attributes).await?.first() {
                            Some(item) => Ok(Some(secret_text(&item.secret().await?))),
                            None => Ok(None),
                        }
                    }
                })
                .await
            }
            Self::Memory(m) => Ok(m.lock().await.get(&(kind, id.to_string())).cloned()),
        }
    }

    /// All `(id, secret)` pairs of one kind.
    pub async fn list(&self, kind: ItemKind) -> Result<Vec<(String, Zeroizing<String>)>> {
        match self {
            Self::Keyring { client, app } => {
                let query = HashMap::from([
                    ("application", app.to_string()),
                    ("kind", kind.as_str().to_string()),
                ]);
                Self::attempt(client, |k| {
                    let query = query.clone();
                    async move {
                        let mut out = Vec::new();
                        for item in k.search_items(&query).await? {
                            let Some(id) = item.attributes().await?.remove("id") else {
                                continue;
                            };
                            out.push((id, secret_text(&item.secret().await?)));
                        }
                        out.sort_by(|a, b| a.0.cmp(&b.0));
                        Ok(out)
                    }
                })
                .await
            }
            Self::Memory(m) => Ok(m
                .lock()
                .await
                .iter()
                .filter(|((k, _), _)| *k == kind)
                .map(|((_, id), s)| (id.clone(), s.clone()))
                .collect()),
        }
    }

    pub async fn delete(&self, kind: ItemKind, id: &str) -> Result<()> {
        match self {
            Self::Keyring { client, app } => {
                let attributes = Self::attributes(app, kind, id);
                Self::attempt(client, |k| {
                    let attributes = attributes.clone();
                    async move { k.delete(&attributes).await }
                })
                .await?
            }
            Self::Memory(m) => {
                m.lock().await.remove(&(kind, id.to_string()));
            }
        }
        Ok(())
    }
}

fn secret_text(secret: &oo7::Secret) -> Zeroizing<String> {
    Zeroizing::new(String::from_utf8_lossy(secret.as_bytes()).into_owned())
}

/// Whether the error means the client's session with the daemon is gone — a
/// restart, a crash, a closed connection — rather than a state the user or the
/// data is in.
fn session_gone(e: &oo7::Error) -> bool {
    match e {
        oo7::Error::DBus(e) => matches!(
            e,
            oo7::dbus::Error::ZBus(_)
                | oo7::dbus::Error::Service(oo7::dbus::ServiceError::NoSession(_))
                | oo7::dbus::Error::Service(oo7::dbus::ServiceError::NoSuchObject(_))
        ),
        oo7::Error::File(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oo7::dbus::{Error as DBusError, ServiceError};

    fn service(e: ServiceError) -> oo7::Error {
        oo7::Error::DBus(DBusError::Service(e))
    }

    #[test]
    fn only_a_lost_session_is_worth_reconnecting_for() {
        // What a gnome-keyring restart leaves behind: the session the client
        // holds no longer exists.
        assert!(session_gone(&service(ServiceError::NoSession(
            "gone".into()
        ))));
        assert!(session_gone(&service(ServiceError::NoSuchObject(
            "gone".into()
        ))));

        // States, not dead sessions: a second client would hit the same wall,
        // and reconnecting would ask the user again.
        assert!(!session_gone(&service(ServiceError::IsLocked(
            "locked".into()
        ))));
        assert!(!session_gone(&oo7::Error::DBus(DBusError::Dismissed)));
        assert!(!session_gone(&oo7::Error::DBus(DBusError::Deleted)));
    }
}
