//! The vault: account keys encrypted at rest (NIP-49 in the keyring),
//! decrypted into memory only while unlocked.
//!
//! Every account shares one Opal passphrase. The passphrase itself is never
//! kept; unlocking decrypts each `ncryptsec` and holds the resulting keys.

use std::collections::HashMap;
use std::time::Duration;

use nostr::key::{Keys, PublicKey, SecretKey};
use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::nips::nip49::{EncryptedSecretKey, KeySecurity};
use tokio::sync::{RwLock, watch};
use zeroize::Zeroizing;

use crate::keystore::{ItemKind, SecretStore};
use crate::{Error, Result};

/// scrypt cost for newly written keys (2^16 ≈ 64 MiB, ~100 ms).
pub const DEFAULT_LOG_N: u8 = 16;

pub struct Vault {
    store: SecretStore,
    log_n: u8,
    keys: RwLock<Option<HashMap<PublicKey, Keys>>>,
    unlocked: watch::Sender<bool>,
}

impl Vault {
    pub fn new(store: SecretStore) -> Self {
        Self::with_log_n(store, DEFAULT_LOG_N)
    }

    /// Lower `log_n` is only meant for tests.
    pub fn with_log_n(store: SecretStore, log_n: u8) -> Self {
        Self {
            store,
            log_n,
            keys: RwLock::new(None),
            unlocked: watch::Sender::new(false),
        }
    }

    pub fn store(&self) -> &SecretStore {
        &self.store
    }

    pub async fn accounts(&self) -> Result<Vec<PublicKey>> {
        self.store
            .list(ItemKind::Account)
            .await?
            .into_iter()
            .map(|(id, _)| PublicKey::from_hex(&id).map_err(Error::nostr))
            .collect()
    }

    pub fn is_unlocked(&self) -> bool {
        *self.unlocked.borrow()
    }

    /// Watch lock state changes (`true` = unlocked).
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.unlocked.subscribe()
    }

    pub async fn unlock(&self, passphrase: &str) -> Result<()> {
        let stored = self.store.list(ItemKind::Account).await?;
        let passphrase = Zeroizing::new(passphrase.to_string());
        let keys = tokio::task::spawn_blocking(move || -> Result<HashMap<PublicKey, Keys>> {
            let mut out = HashMap::new();
            for (_, ncryptsec) in stored {
                let keys = decrypt(&ncryptsec, &passphrase)?;
                out.insert(keys.public_key(), keys);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Invalid(e.to_string()))??;
        *self.keys.write().await = Some(keys);
        self.unlocked.send_replace(true);
        Ok(())
    }

    pub async fn lock(&self) {
        // Dropping `Keys` wipes the secret keys.
        *self.keys.write().await = None;
        self.unlocked.send_replace(false);
    }

    pub async fn keys(&self, account: &PublicKey) -> Result<Keys> {
        match self.keys.read().await.as_ref() {
            None => Err(Error::Locked),
            Some(map) => map.get(account).cloned().ok_or(Error::AccountNotFound),
        }
    }

    /// Wait until the vault is unlocked, or fail with [`Error::Locked`].
    pub async fn wait_unlocked(&self, timeout: Duration) -> Result<()> {
        let mut rx = self.subscribe();
        tokio::time::timeout(timeout, rx.wait_for(|u| *u))
            .await
            .map_err(|_| Error::Locked)?
            .map_err(|_| Error::Locked)?;
        Ok(())
    }

    /// Store a new account. The passphrase must match the existing accounts;
    /// the first account sets it.
    pub async fn add_account(&self, keys: Keys, passphrase: &str) -> Result<PublicKey> {
        let pk = keys.public_key();
        let existing = self.store.list(ItemKind::Account).await?;
        if existing.iter().any(|(id, _)| *id == pk.to_hex()) {
            return Err(Error::AccountExists);
        }
        let passphrase = Zeroizing::new(passphrase.to_string());
        let log_n = self.log_n;
        let secret = keys.secret_key().clone();
        let ncryptsec = tokio::task::spawn_blocking(move || -> Result<Zeroizing<String>> {
            if let Some((_, other)) = existing.first() {
                decrypt(other, &passphrase)?;
            }
            encrypt(&secret, &passphrase, log_n)
        })
        .await
        .map_err(|e| Error::Invalid(e.to_string()))??;

        let label = format!("Opal Nostr key ({})", short_npub(&pk));
        self.store
            .put(ItemKind::Account, &pk.to_hex(), &label, &ncryptsec)
            .await?;
        if let Some(map) = self.keys.write().await.as_mut() {
            map.insert(pk, keys);
        }
        Ok(pk)
    }

    pub async fn remove_account(&self, account: &PublicKey) -> Result<()> {
        self.store
            .delete(ItemKind::Account, &account.to_hex())
            .await?;
        if let Some(map) = self.keys.write().await.as_mut() {
            map.remove(account);
        }
        Ok(())
    }

    /// The stored `ncryptsec` for backups. It is protected by the Opal passphrase.
    pub async fn export_ncryptsec(&self, account: &PublicKey) -> Result<Zeroizing<String>> {
        self.store
            .get(ItemKind::Account, &account.to_hex())
            .await?
            .ok_or(Error::AccountNotFound)
    }

    /// Re-encrypt every account under a new passphrase.
    pub async fn change_passphrase(&self, old: &str, new: &str) -> Result<()> {
        let stored = self.store.list(ItemKind::Account).await?;
        let old = Zeroizing::new(old.to_string());
        let new = Zeroizing::new(new.to_string());
        let log_n = self.log_n;
        let rewrapped =
            tokio::task::spawn_blocking(move || -> Result<Vec<(PublicKey, Zeroizing<String>)>> {
                stored
                    .iter()
                    .map(|(_, ncryptsec)| {
                        let keys = decrypt(ncryptsec, &old)?;
                        Ok((keys.public_key(), encrypt(keys.secret_key(), &new, log_n)?))
                    })
                    .collect()
            })
            .await
            .map_err(|e| Error::Invalid(e.to_string()))??;
        for (pk, ncryptsec) in rewrapped {
            let label = format!("Opal Nostr key ({})", short_npub(&pk));
            self.store
                .put(ItemKind::Account, &pk.to_hex(), &label, &ncryptsec)
                .await?;
        }
        Ok(())
    }
}

fn encrypt(secret: &SecretKey, passphrase: &str, log_n: u8) -> Result<Zeroizing<String>> {
    let enc = EncryptedSecretKey::new(secret, passphrase, log_n, KeySecurity::Medium)
        .map_err(Error::nostr)?;
    Ok(Zeroizing::new(enc.to_bech32().map_err(Error::nostr)?))
}

fn decrypt(ncryptsec: &str, passphrase: &str) -> Result<Keys> {
    let enc = EncryptedSecretKey::from_bech32(ncryptsec).map_err(Error::nostr)?;
    let secret = enc.decrypt(passphrase).map_err(|_| Error::BadPassphrase)?;
    Ok(Keys::new(secret))
}

pub fn short_npub(pk: &PublicKey) -> String {
    let npub = pk.to_bech32().unwrap_or_default();
    if npub.len() > 16 {
        format!("{}…{}", &npub[..10], &npub[npub.len() - 4..])
    } else {
        npub
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        Vault::with_log_n(SecretStore::memory(), 4)
    }

    #[tokio::test]
    async fn lifecycle() {
        let v = vault();
        let a = Keys::generate();
        let pk = v.add_account(a.clone(), "pass").await.unwrap();
        assert!(matches!(v.keys(&pk).await, Err(Error::Locked)));

        assert!(matches!(v.unlock("wrong").await, Err(Error::BadPassphrase)));
        v.unlock("pass").await.unwrap();
        assert_eq!(v.keys(&pk).await.unwrap().public_key(), pk);

        // A second account must use the same passphrase.
        assert!(matches!(
            v.add_account(Keys::generate(), "other").await,
            Err(Error::BadPassphrase)
        ));
        let b = v.add_account(Keys::generate(), "pass").await.unwrap();
        assert!(
            v.keys(&b).await.is_ok(),
            "added while unlocked is usable at once"
        );
        assert!(matches!(
            v.add_account(a, "pass").await,
            Err(Error::AccountExists)
        ));

        v.lock().await;
        assert!(matches!(v.keys(&pk).await, Err(Error::Locked)));
        assert_eq!(v.accounts().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn stored_secret_is_ncryptsec() {
        let v = vault();
        let pk = v.add_account(Keys::generate(), "pass").await.unwrap();
        let stored = v.export_ncryptsec(&pk).await.unwrap();
        assert!(stored.starts_with("ncryptsec1"));
    }

    #[tokio::test]
    async fn change_passphrase() {
        let v = vault();
        v.add_account(Keys::generate(), "old").await.unwrap();
        v.change_passphrase("old", "new").await.unwrap();
        assert!(v.unlock("old").await.is_err());
        v.unlock("new").await.unwrap();
    }

    #[tokio::test]
    async fn wait_unlocked_times_out_then_succeeds() {
        let v = std::sync::Arc::new(vault());
        v.add_account(Keys::generate(), "p").await.unwrap();
        assert!(v.wait_unlocked(Duration::from_millis(20)).await.is_err());
        let v2 = v.clone();
        let waiter = tokio::spawn(async move { v2.wait_unlocked(Duration::from_secs(5)).await });
        v.unlock("p").await.unwrap();
        waiter.await.unwrap().unwrap();
    }
}
