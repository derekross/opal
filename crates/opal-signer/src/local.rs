//! Local apps: programs on this computer (Peridot) that use the account key
//! over the control socket instead of NIP-46.
//!
//! They pair once, through the same approval dialog as a `nostrconnect://`
//! link, and from then on send a pairing token with every request. The
//! token only says *which* paired app is asking; what it may do is decided
//! by the app's policy, its saved rules and prompts, exactly like a remote
//! app. The daemon also remembers where the pairing came from (the systemd
//! unit, and the executable when that's readable) and refuses other
//! programs that show up with the token. Any process running as the user
//! could still start the real program, so the real protections are the
//! pairing prompt, the rules, and the activity log.

use std::path::{Path, PathBuf};

use nostr_sdk::prelude::{PublicKey, RelayUrl, Timestamp};
use opal_core::config::Policy;
use zeroize::Zeroizing;

use crate::connection::{AppKind, ConnectionInfo};
use crate::permissions::{Rule, is_sensitive};
use crate::protocol::Method;
use crate::server::{Op, Outcome, Requester, Signer, SignerError, SignerEvent, WhenLocked};
use crate::store::{LocalAppRecord, hash_secret};

/// Most kinds an app may declare.
pub const MAX_KINDS: usize = 32;

/// What the user agreed to when pairing a local app.
#[derive(Debug, Clone)]
pub struct LocalPairing {
    pub app_key: String,
    pub name: String,
    pub account: PublicKey,
    pub policy: Policy,
    pub kinds: Vec<u16>,
    pub nip44: bool,
    pub exe: Option<PathBuf>,
    pub unit: Option<String>,
    /// `(method, kind)` pairs to allow from now on; filtered to
    /// [`grantable`], so nothing sensitive slips in.
    pub grant: Vec<(Method, Option<u16>)>,
}

/// `a-z`, `0-9` and `-`, 1 to 32 characters: an id, not a display name.
pub fn valid_app_key(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What a pairing prompt may offer as "always allow": the declared kinds
/// that aren't sensitive, and encrypting (never decrypting) its own data.
pub fn grantable(kinds: &[u16], nip44: bool) -> Vec<(Method, Option<u16>)> {
    let mut out: Vec<_> = kinds
        .iter()
        .filter(|k| !is_sensitive(&Method::SignEvent, Some(**k)))
        .map(|k| (Method::SignEvent, Some(*k)))
        .collect();
    if nip44 {
        out.push((Method::Nip44Encrypt, None));
    }
    out
}

pub fn local_info(a: &LocalAppRecord) -> ConnectionInfo {
    ConnectionInfo {
        id: a.id.clone(),
        kind: AppKind::Local,
        account: a.account.to_hex(),
        signer_pubkey: None,
        client: None,
        connected: true,
        name: Some(a.name.clone()),
        display_name: a.name.clone(),
        url: None,
        image: None,
        relays: Vec::new(),
        policy: a.policy,
        requested_perms: Vec::new(),
        created_at: a.created_at.as_secs(),
        last_used: a.last_used.map(|t| t.as_secs()),
        expires_unused_at: None,
        exe: a.exe.clone(),
        unit: a.unit.clone(),
        kinds: a.kinds.clone(),
        nip44: a.nip44,
    }
}

impl Requester {
    pub(crate) fn local(a: &LocalAppRecord) -> Self {
        Self {
            connection_id: a.id.clone(),
            app_name: a.name.clone(),
            app_url: None,
            app_image: None,
            account: a.account,
            policy: a.policy,
        }
    }
}

impl Signer {
    /// Record a pairing the user approved. Returns the app and its new
    /// token; only the token's hash is kept. Pairing an app key that is
    /// already paired keeps its id, rotates the token, replaces what it
    /// declared and starts its rules over from `grant`.
    pub async fn pair_local_app(
        &self,
        p: LocalPairing,
    ) -> Result<(ConnectionInfo, Zeroizing<String>), SignerError> {
        let inner = &self.inner;
        inner.check_account(&p.account).await?;
        let store = inner.store.as_ref().ok_or(SignerError::NoStore)?;
        let now = Timestamp::now();
        let existing = store.local_app_by_key(&p.app_key)?;
        let token = Zeroizing::new(crate::server::random_hex(32));
        let rec = LocalAppRecord {
            id: existing
                .as_ref()
                .map_or_else(|| crate::server::random_hex(16), |e| e.id.clone()),
            app_key: p.app_key,
            name: p.name,
            account: p.account,
            policy: p.policy,
            kinds: p.kinds,
            nip44: p.nip44,
            exe: p.exe.map(|e| e.to_string_lossy().into_owned()),
            unit: p.unit,
            token_hash: hash_secret(&token),
            created_at: existing.as_ref().map_or(now, |e| e.created_at),
            last_used: None,
        };
        if existing.is_some() {
            store.clear_rules(&rec.id)?;
        }
        store.save_local_app(&rec)?;
        let allowed = grantable(&rec.kinds, rec.nip44);
        for (method, kind) in p.grant.iter().filter(|g| allowed.contains(g)) {
            store.put_rule(&Rule {
                app_id: rec.id.clone(),
                method: method.clone(),
                kind: *kind,
                allow: true,
                until: None,
                created_at: now.as_secs(),
            })?;
        }
        let info = local_info(&rec);
        inner.emit(if existing.is_some() {
            SignerEvent::Updated {
                connection: info.clone(),
            }
        } else {
            SignerEvent::Connected {
                connection: info.clone(),
            }
        });
        Ok((info, token))
    }

    /// The paired app a token belongs to, if any.
    pub fn local_app_by_token(&self, token: &str) -> Option<LocalAppRecord> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.inner
            .store
            .as_ref()?
            .local_app_by_token_hash(&hash_secret(token))
            .ok()
            .flatten()
    }

    /// The token is only good from where the pairing was made: the same
    /// systemd unit, and the same executable where that could be read.
    pub fn check_local_peer(
        a: &LocalAppRecord,
        peer_exe: Option<&Path>,
        peer_unit: Option<&str>,
    ) -> Result<(), String> {
        let exe = peer_exe.map(|p| p.to_string_lossy().into_owned());
        if exe == a.exe && peer_unit == a.unit.as_deref() {
            Ok(())
        } else {
            Err(format!(
                "paired with a different program ({}); pair again",
                a.exe.as_deref().or(a.unit.as_deref()).unwrap_or("unknown")
            ))
        }
    }

    /// One request from a paired app, through the same policy, rules and
    /// prompts as a remote app. Refuses at once while locked.
    pub async fn local_request(&self, a: &LocalAppRecord, op: Op) -> Result<Outcome, String> {
        match &op {
            Op::SignEvent(ev) => {
                let kind = ev.kind.as_u16();
                if !a.kinds.contains(&kind) {
                    return Err(format!(
                        "{} didn't declare kind {kind} when it paired",
                        a.name
                    ));
                }
                if ev.pubkey != a.account {
                    return Err("the event isn't for that account".into());
                }
            }
            Op::Cipher { method, with, .. } => {
                if !a.nip44 || !matches!(method, Method::Nip44Encrypt | Method::Nip44Decrypt) {
                    return Err(format!(
                        "{} didn't ask for encryption when it paired",
                        a.name
                    ));
                }
                if *with != a.account {
                    return Err("local apps only encrypt to their own account".into());
                }
            }
            Op::GetPublicKey => {}
        }
        if let Some(store) = &self.inner.store {
            let _ = store.touch_app(&a.id, Timestamp::now());
        }
        self.inner
            .authorize(Requester::local(a), op, WhenLocked::FailFast)
            .await
    }

    /// Every app, remote and local, newest first.
    pub async fn all_apps(&self) -> Vec<ConnectionInfo> {
        let mut v = self.connections().await;
        if let Some(store) = &self.inner.store
            && let Ok(local) = store.local_apps()
        {
            v.extend(local.iter().map(local_info));
        }
        v.sort_by_key(|c| std::cmp::Reverse(c.created_at));
        v
    }

    pub async fn app_info(&self, id: &str) -> Option<ConnectionInfo> {
        if let Some(c) = self.connection(id).await {
            return Some(c);
        }
        self.inner
            .store
            .as_ref()?
            .local_app(id)
            .ok()
            .flatten()
            .as_ref()
            .map(local_info)
    }

    /// Change any app's name or policy (and, for remote apps, relays).
    pub async fn update_app(
        &self,
        id: &str,
        name: Option<String>,
        policy: Option<Policy>,
        relays: Option<Vec<RelayUrl>>,
    ) -> Result<ConnectionInfo, SignerError> {
        if self.inner.conns.get(id).await.is_some() {
            return self.update_connection(id, name, policy, relays).await;
        }
        let store = self.inner.store.as_ref().ok_or(SignerError::NoStore)?;
        let name = name.filter(|n| !n.trim().is_empty());
        let rec = store
            .update_local_app(id, name.as_deref(), policy)?
            .ok_or(SignerError::UnknownApp)?;
        let info = local_info(&rec);
        self.inner.emit(SignerEvent::Updated {
            connection: info.clone(),
        });
        Ok(info)
    }

    /// Remove any app; its rules go with it. A removed local app's token
    /// stops working at once.
    pub async fn remove_app(&self, id: &str) -> Result<bool, SignerError> {
        if self.remove_connection(id).await? {
            return Ok(true);
        }
        let Some(store) = &self.inner.store else {
            return Ok(false);
        };
        if store.local_app(id)?.is_none() {
            return Ok(false);
        }
        store.delete_app(id)?;
        self.inner.emit(SignerEvent::Disconnected {
            connection_id: id.to_string(),
        });
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_keys_are_plain_ids() {
        assert!(valid_app_key("peridot"));
        assert!(valid_app_key("my-app-2"));
        assert!(!valid_app_key(""));
        assert!(!valid_app_key("Peridot"));
        assert!(!valid_app_key("a b"));
        assert!(!valid_app_key(&"x".repeat(33)));
    }

    #[test]
    fn grantable_skips_sensitive_kinds_and_decrypt() {
        let g = grantable(&[30078, 22242, 24242, 1], true);
        assert_eq!(
            g,
            vec![
                (Method::SignEvent, Some(30078)),
                (Method::SignEvent, Some(1)),
                (Method::Nip44Encrypt, None),
            ]
        );
        assert!(grantable(&[5], false).is_empty());
    }

    #[test]
    fn peer_must_match_exactly() {
        let mut a = LocalAppRecord {
            id: "x".into(),
            app_key: "peridot".into(),
            name: "Peridot".into(),
            account: nostr_sdk::prelude::Keys::generate().public_key(),
            policy: Policy::Basic,
            kinds: vec![],
            nip44: false,
            exe: Some("/usr/bin/peridotd".into()),
            unit: Some("peridot.service".into()),
            token_hash: String::new(),
            created_at: Timestamp::from(1),
            last_used: None,
        };
        let exe = Some(Path::new("/usr/bin/peridotd"));
        assert!(Signer::check_local_peer(&a, exe, Some("peridot.service")).is_ok());
        let err =
            Signer::check_local_peer(&a, Some(Path::new("/tmp/evil")), Some("peridot.service"))
                .unwrap_err();
        assert!(err.starts_with("paired with a different program"), "{err}");
        assert!(Signer::check_local_peer(&a, exe, Some("evil.service")).is_err());
        assert!(Signer::check_local_peer(&a, exe, None).is_err());
        assert!(Signer::check_local_peer(&a, None, Some("peridot.service")).is_err());
        // The usual case under systemd: no executable, only the unit.
        a.exe = None;
        assert!(Signer::check_local_peer(&a, None, Some("peridot.service")).is_ok());
        assert!(Signer::check_local_peer(&a, exe, Some("peridot.service")).is_err());
        let err = Signer::check_local_peer(&a, None, Some("other.service")).unwrap_err();
        assert!(err.contains("peridot.service"), "{err}");
    }
}
