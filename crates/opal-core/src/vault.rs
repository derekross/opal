//! The vault: account keys encrypted at rest (NIP-49 in the keyring),
//! decrypted into memory only while unlocked.
//!
//! Every account shares one Opal passphrase. The passphrase itself is never
//! kept; unlocking decrypts each `ncryptsec` and holds the resulting keys.
//!
//! All key-derivation work (unlock, add, verify, change) is serialized, so
//! parallel requests can't exhaust memory, and failed attempts back off.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nostr::key::{Keys, PublicKey, SecretKey};
use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::nips::nip49::{EncryptedSecretKey, KeySecurity};
use tokio::sync::{Mutex, RwLock, watch};
use zeroize::Zeroizing;

use crate::keystore::{ItemKind, SecretStore};
use crate::{Error, Result};

/// scrypt cost for newly written keys (2^18 ≈ 256 MiB, well under a second).
/// It's also the most nostr clients accept when importing an ncryptsec.
pub const DEFAULT_LOG_N: u8 = 18;
/// Longest wait between failed passphrase attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Attempts {
    failures: u32,
    next_allowed: Option<Instant>,
}

pub struct Vault {
    store: SecretStore,
    log_n: u8,
    keys: RwLock<Option<HashMap<PublicKey, Keys>>>,
    unlocked: watch::Sender<bool>,
    /// One key-derivation / keyring rewrite at a time.
    ops: Mutex<()>,
    attempts: std::sync::Mutex<Attempts>,
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
            ops: Mutex::new(()),
            attempts: std::sync::Mutex::new(Attempts::default()),
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

    fn check_backoff(&self) -> Result<()> {
        let a = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        match a.next_allowed {
            Some(t) if Instant::now() < t => Err(Error::Invalid(format!(
                "too many wrong passphrases; try again in {}s",
                t.saturating_duration_since(Instant::now()).as_secs().max(1)
            ))),
            _ => Ok(()),
        }
    }

    fn record_attempt(&self, ok: bool) {
        let mut a = self.attempts.lock().unwrap_or_else(|p| p.into_inner());
        if ok {
            *a = Attempts::default();
        } else {
            a.failures += 1;
            // A typo costs nothing; repeated failures wait 1s, 2s, 4s… up to 30s.
            if a.failures >= 2 {
                let wait = Duration::from_secs(1u64 << (a.failures - 2).min(5)).min(MAX_BACKOFF);
                a.next_allowed = Some(Instant::now() + wait);
            }
        }
    }

    /// Check a passphrase against the stored keys without unlocking.
    pub async fn verify_passphrase(&self, passphrase: &str) -> Result<()> {
        self.check_backoff()?;
        let _guard = self.ops.lock().await;
        let stored = self.store.list(ItemKind::Account).await?;
        let Some((_, first)) = stored.into_iter().next() else {
            return Ok(()); // nothing to protect yet
        };
        let passphrase = Zeroizing::new(passphrase.to_string());
        let ok = tokio::task::spawn_blocking(move || decrypt(&first, &passphrase).is_ok())
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?;
        self.record_attempt(ok);
        if ok {
            Ok(())
        } else {
            Err(Error::BadPassphrase)
        }
    }

    /// Decrypt the stored keys. Items that don't open with this passphrase
    /// (e.g. something else planted in the keyring) are skipped, as long as
    /// at least one does.
    pub async fn unlock(&self, passphrase: &str) -> Result<()> {
        self.check_backoff()?;
        let _guard = self.ops.lock().await;
        let stored = self.store.list(ItemKind::Account).await?;
        let passphrase = Zeroizing::new(passphrase.to_string());
        let log_n = self.log_n;
        let pass2 = passphrase.clone();
        let (keys, failed, upgrades) = tokio::task::spawn_blocking(move || {
            let mut out = HashMap::new();
            let mut failed = 0usize;
            let mut upgrades = Vec::new();
            for (_, ncryptsec) in &stored {
                match decrypt_full(ncryptsec, &pass2) {
                    Ok((keys, enc)) => {
                        if enc.log_n() < log_n {
                            upgrades.push((keys.clone(), enc.key_security()));
                        }
                        out.insert(keys.public_key(), keys);
                    }
                    Err(_) => failed += 1,
                }
            }
            (out, failed, upgrades)
        })
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?;
        if keys.is_empty() && failed > 0 {
            self.record_attempt(false);
            return Err(Error::BadPassphrase);
        }
        self.record_attempt(true);
        if failed > 0 {
            tracing::warn!("{failed} keyring item(s) didn't open with this passphrase");
        }
        // Re-encrypt keys written with a weaker cost setting.
        for (keys, security) in upgrades {
            let pk = keys.public_key();
            let secret = keys.secret_key().clone();
            let p = passphrase.clone();
            let enc =
                tokio::task::spawn_blocking(move || encrypt(&secret, &p, log_n, security)).await;
            if let Ok(Ok(ncryptsec)) = enc {
                let label = format!("Opal Nostr key ({})", short_npub(&pk));
                if let Err(e) = self
                    .store
                    .put(ItemKind::Account, &pk.to_hex(), &label, &ncryptsec)
                    .await
                {
                    tracing::warn!("upgrading key encryption failed: {e}");
                }
            }
        }
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

    /// Store a new, freshly generated account (see [`Self::add_account_with`]).
    pub async fn add_account(&self, keys: Keys, passphrase: &str) -> Result<PublicKey> {
        self.add_account_with(keys, passphrase, KeySecurity::Medium)
            .await
    }

    /// Store a new account. The passphrase must match the existing accounts;
    /// the first account sets it. `security` is NIP-49's key-security byte:
    /// `Medium` for keys generated here, `Unknown` for imported ones.
    pub async fn add_account_with(
        &self,
        keys: Keys,
        passphrase: &str,
        security: KeySecurity,
    ) -> Result<PublicKey> {
        self.check_backoff()?;
        let _guard = self.ops.lock().await;
        let pk = keys.public_key();
        let existing = self.store.list(ItemKind::Account).await?;
        if existing.iter().any(|(id, _)| *id == pk.to_hex()) {
            return Err(Error::AccountExists);
        }
        let passphrase = Zeroizing::new(passphrase.to_string());
        let log_n = self.log_n;
        let secret = keys.secret_key().clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Zeroizing<String>> {
            if let Some((_, other)) = existing.first() {
                decrypt(other, &passphrase)?;
            }
            encrypt(&secret, &passphrase, log_n, security)
        })
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?;
        self.record_attempt(!matches!(result, Err(Error::BadPassphrase)));
        let ncryptsec = result?;

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
        let _guard = self.ops.lock().await;
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

    /// Re-encrypt every account under a new passphrase. All or nothing: if
    /// writing any item fails, the ones already written are put back.
    pub async fn change_passphrase(&self, old: &str, new: &str) -> Result<()> {
        self.check_backoff()?;
        let _guard = self.ops.lock().await;
        let stored = self.store.list(ItemKind::Account).await?;
        let old = Zeroizing::new(old.to_string());
        let new = Zeroizing::new(new.to_string());
        let log_n = self.log_n;
        let originals = stored.clone();
        type Rewrapped = Vec<(PublicKey, Zeroizing<String>)>;
        let result = tokio::task::spawn_blocking(move || -> Result<Rewrapped> {
            stored
                .iter()
                .map(|(_, ncryptsec)| {
                    let (keys, enc) = decrypt_full(ncryptsec, &old)?;
                    let again = encrypt(keys.secret_key(), &new, log_n, enc.key_security())?;
                    Ok((keys.public_key(), again))
                })
                .collect()
        })
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?;
        self.record_attempt(!matches!(result, Err(Error::BadPassphrase)));
        let rewrapped = result?;

        let mut written: Vec<PublicKey> = Vec::new();
        for (pk, ncryptsec) in &rewrapped {
            let label = format!("Opal Nostr key ({})", short_npub(pk));
            if let Err(e) = self
                .store
                .put(ItemKind::Account, &pk.to_hex(), &label, ncryptsec)
                .await
            {
                // Put back what was already changed so one passphrase still
                // opens everything.
                for done in &written {
                    if let Some((id, orig)) = originals.iter().find(|(id, _)| *id == done.to_hex())
                    {
                        let label = format!("Opal Nostr key ({})", short_npub(done));
                        let _ = self.store.put(ItemKind::Account, id, &label, orig).await;
                    }
                }
                return Err(e);
            }
            written.push(*pk);
        }
        Ok(())
    }
}

fn encrypt(
    secret: &SecretKey,
    passphrase: &str,
    log_n: u8,
    security: KeySecurity,
) -> Result<Zeroizing<String>> {
    let enc = EncryptedSecretKey::new(secret, passphrase, log_n, security).map_err(Error::nostr)?;
    Ok(Zeroizing::new(enc.to_bech32().map_err(Error::nostr)?))
}

fn decrypt(ncryptsec: &str, passphrase: &str) -> Result<Keys> {
    decrypt_full(ncryptsec, passphrase).map(|(k, _)| k)
}

fn decrypt_full(ncryptsec: &str, passphrase: &str) -> Result<(Keys, EncryptedSecretKey)> {
    let enc = EncryptedSecretKey::from_bech32(ncryptsec).map_err(Error::nostr)?;
    let secret = enc.decrypt(passphrase).map_err(|_| Error::BadPassphrase)?;
    Ok((Keys::new(secret), enc))
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
    async fn repeated_wrong_passphrases_back_off() {
        let v = vault();
        v.add_account(Keys::generate(), "right").await.unwrap();
        assert!(matches!(v.unlock("wrong").await, Err(Error::BadPassphrase)));
        assert!(matches!(v.unlock("wrong").await, Err(Error::BadPassphrase)));
        // Now even the right one has to wait.
        assert!(matches!(v.unlock("right").await, Err(Error::Invalid(_))));
        tokio::time::sleep(Duration::from_millis(1100)).await;
        v.unlock("right").await.unwrap();
    }

    #[tokio::test]
    async fn a_planted_item_does_not_block_unlocking() {
        let v = vault();
        let pk = v.add_account(Keys::generate(), "pw").await.unwrap();
        // Something else writes an account item under another passphrase.
        let other = Vault::with_log_n(SecretStore::memory(), 4);
        let bogus = Keys::generate();
        other.add_account(bogus.clone(), "attacker").await.unwrap();
        let planted = other.export_ncryptsec(&bogus.public_key()).await.unwrap();
        v.store()
            .put(
                ItemKind::Account,
                &bogus.public_key().to_hex(),
                "x",
                &planted,
            )
            .await
            .unwrap();
        v.unlock("pw").await.unwrap();
        assert!(v.keys(&pk).await.is_ok());
        assert!(v.keys(&bogus.public_key()).await.is_err());
    }

    #[tokio::test]
    async fn imported_keys_keep_their_security_byte_and_weak_ones_upgrade() {
        let v = Vault::with_log_n(SecretStore::memory(), 6);
        let keys = Keys::generate();
        // Written with a lower cost than the vault uses now.
        let weak = EncryptedSecretKey::new(keys.secret_key(), "pw", 4, KeySecurity::Unknown)
            .unwrap()
            .to_bech32()
            .unwrap();
        v.store()
            .put(ItemKind::Account, &keys.public_key().to_hex(), "x", &weak)
            .await
            .unwrap();
        v.unlock("pw").await.unwrap();
        let now = v.export_ncryptsec(&keys.public_key()).await.unwrap();
        let enc = EncryptedSecretKey::from_bech32(&now).unwrap();
        assert_eq!(enc.log_n(), 6, "upgraded to the vault's cost");
        assert_eq!(
            enc.key_security(),
            KeySecurity::Unknown,
            "security byte kept"
        );
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
