//! Secret storage backed by the Secret Service (gnome-keyring, KWallet, …).
//!
//! Account keys are only ever written here as NIP-49 `ncryptsec` strings, so a
//! copied keyring file is useless without the Opal passphrase.

use std::collections::{BTreeMap, HashMap};

use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::Result;

const APPLICATION: &str = "opal";

/// What an item in the store holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    /// A user account, secret = `ncryptsec1…`.
    Account,
    /// A per-connection NIP-46 transport key, secret = hex secret key.
    ConnKey,
}

impl ItemKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::ConnKey => "conn-key",
        }
    }
}

pub enum SecretStore {
    Keyring(oo7::Keyring),
    /// In-process store for tests.
    Memory(Mutex<BTreeMap<(ItemKind, String), Zeroizing<String>>>),
}

impl SecretStore {
    pub async fn keyring() -> Result<Self> {
        Ok(Self::Keyring(oo7::Keyring::new().await?))
    }

    pub fn memory() -> Self {
        Self::Memory(Mutex::new(BTreeMap::new()))
    }

    fn attributes(kind: ItemKind, id: &str) -> HashMap<&'static str, String> {
        HashMap::from([
            ("application", APPLICATION.to_string()),
            ("kind", kind.as_str().to_string()),
            ("id", id.to_string()),
        ])
    }

    pub async fn put(&self, kind: ItemKind, id: &str, label: &str, secret: &str) -> Result<()> {
        match self {
            Self::Keyring(k) => {
                k.create_item(label, &Self::attributes(kind, id), secret, true)
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
            Self::Keyring(k) => {
                let items = k.search_items(&Self::attributes(kind, id)).await?;
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
            Self::Keyring(k) => {
                let query = HashMap::from([
                    ("application", APPLICATION.to_string()),
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
            Self::Keyring(k) => k.delete(&Self::attributes(kind, id)).await?,
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
