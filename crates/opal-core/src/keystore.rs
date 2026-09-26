//! Secret storage backed by the Secret Service (gnome-keyring, KWallet, …).
//!
//! Opal account keys are only ever written here as NIP-49 `ncryptsec`
//! strings, so a copied keyring file is useless without the Opal passphrase.
//! Peridot's device identity is the exception: it is meant to work without a
//! passphrase, so it is only as safe as the login keyring itself.

use std::collections::{BTreeMap, HashMap};

use tokio::sync::Mutex;
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
        }
    }
}

pub enum SecretStore {
    /// Items tagged `application = <app>`.
    Keyring(oo7::Keyring, &'static str),
    /// In-process store for tests.
    Memory(Mutex<BTreeMap<(ItemKind, String), Zeroizing<String>>>),
}

impl SecretStore {
    /// Opal's items in the login keyring.
    pub async fn keyring() -> Result<Self> {
        Self::keyring_for(crate::paths::AppDirs::OPAL).await
    }

    /// Another app's items in the login keyring.
    pub async fn keyring_for(app: crate::paths::AppDirs) -> Result<Self> {
        Ok(Self::Keyring(oo7::Keyring::new().await?, app.name))
    }

    pub fn memory() -> Self {
        Self::Memory(Mutex::new(BTreeMap::new()))
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
            Self::Keyring(k, app) => {
                k.create_item(label, &Self::attributes(app, kind, id), secret, true)
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
            Self::Keyring(k, app) => {
                let items = k.search_items(&Self::attributes(app, kind, id)).await?;
                match items.first() {
                    Some(item) => Ok(Some(secret_text(&item.secret().await?))),
                    None => Ok(None),
                }
            }
            Self::Memory(m) => Ok(m.lock().await.get(&(kind, id.to_string())).cloned()),
        }
    }

    /// All `(id, secret)` pairs of one kind.
    pub async fn list(&self, kind: ItemKind) -> Result<Vec<(String, Zeroizing<String>)>> {
        match self {
            Self::Keyring(k, app) => {
                let query = HashMap::from([
                    ("application", app.to_string()),
                    ("kind", kind.as_str().to_string()),
                ]);
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
            Self::Keyring(k, app) => k.delete(&Self::attributes(app, kind, id)).await?,
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
