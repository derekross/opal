//! The live part: finds your relays (NIP-65), your mute list (NIP-51, private
//! entries too when unlocked), subscribes to events that tag you, stores them
//! as notifications and fills in profiles and the notes they refer to.
//!
//! Decrypted private mutes are cached in the login keyring, so they apply
//! after a restart even while Opal is locked.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nostr_sdk::prelude::*;
use opal_core::accounts::Profile;
use opal_core::config::NotificationsConfig;
use opal_core::keystore::ItemKind;
use opal_core::vault::Vault;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::classify::{NotifType, Notification, classify};
use crate::store::NotifyStore;

const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// How far back to catch up on start (at most).
const MAX_CATCH_UP_SECS: u64 = 7 * 86_400;
/// Gift wraps are backdated up to two days (NIP-59).
const WRAP_BACKDATE_SECS: u64 = 2 * 86_400;
const REFRESH_EVERY: Duration = Duration::from_secs(30 * 60);
const PROFILE_TTL_SECS: u64 = 86_400;
const FEED_SUB: &str = "opal-notify";
/// Returned by [`NotifyEngine::set_muted`] when the key is needed and Opal is
/// locked. The API matches on "passphrase" to ask for it.
pub const MUTE_NEEDS_UNLOCK: &str = "unlock Opal with your passphrase to change your mute list";
/// Returned by [`NotifyHandle::set_muted`] when your relays couldn't confirm
/// the current list, so changing it could lose entries.
pub const MUTE_UNSURE: &str = "couldn't confirm your current mute list with your relays";
/// The engine stopped before answering: the change may or may not be saved.
pub const MUTE_INTERRUPTED: &str =
    "notifications restarted while your mute list was being saved; check Settings to see if it was";
/// Like [`MUTE_UNSURE`], when no relay list (kind 10002) was found at all.
pub const MUTE_NO_RELAY_LIST: &str = "no relay list (NIP-65) was found for your account";
const DM_SUB: &str = "opal-dms";

pub struct NotifyParams {
    pub account: PublicKey,
    pub config: NotificationsConfig,
    pub store: NotifyStore,
    /// Present when the account's key is local: enables private mutes and DMs.
    pub vault: Option<Arc<Vault>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotifyEvent {
    /// A new notification. `fresh` = it happened just now (worth a popup),
    /// as opposed to catching up on history.
    New {
        notification: Notification,
        fresh: bool,
    },
    /// Profiles or referenced notes arrived; lists should be re-read.
    Updated,
    Status {
        status: NotifyStatus,
    },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NotifyStatus {
    pub account: String,
    pub read_relays: Vec<String>,
    pub dm_relays: Vec<String>,
    pub muted: usize,
    pub synced: bool,
    pub dms: bool,
    pub error: Option<String>,
}

/// One entry of your NIP-51 mute list.
#[derive(Debug, Clone, Serialize)]
pub struct MutedEntry {
    pub pubkey: String,
    /// In the encrypted part of the list (only you can see it).
    pub private: bool,
}

enum Command {
    SetMuted {
        target: PublicKey,
        mute: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Mutes {
        reply: oneshot::Sender<Vec<MutedEntry>>,
    },
    /// Opal's own block list changed.
    SetBlocked(Vec<String>),
}

pub struct NotifyEngine {
    task: JoinHandle<()>,
    commands: mpsc::UnboundedSender<Command>,
    has_key: bool,
    client: Client,
    events: broadcast::Sender<NotifyEvent>,
    status: Arc<RwLock<NotifyStatus>>,
    account: PublicKey,
}

impl NotifyEngine {
    pub fn start(params: NotifyParams) -> Self {
        let client = Client::default();
        let (events, _) = broadcast::channel(256);
        let status = Arc::new(RwLock::new(NotifyStatus {
            account: params.account.to_hex(),
            ..Default::default()
        }));
        let account = params.account;
        let has_key = params.vault.is_some();
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let run = Runner {
            me: params.account,
            me_hex: params.account.to_hex(),
            cfg: params.config,
            store: params.store,
            vault: params.vault,
            client: client.clone(),
            events: events.clone(),
            status: status.clone(),
            muted: HashSet::new(),
            started: Timestamp::now().as_secs(),
            mute_event: None,
            private_mutes: None,
        };
        let task = tokio::spawn(run.run(commands_rx));
        Self {
            task,
            commands,
            has_key,
            client,
            events,
            status,
            account,
        }
    }

    pub fn account(&self) -> PublicKey {
        self.account
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NotifyEvent> {
        self.events.subscribe()
    }

    pub async fn status(&self) -> NotifyStatus {
        self.status.read().await.clone()
    }

    /// For changing mutes without holding on to the engine.
    pub fn handle(&self) -> NotifyHandle {
        NotifyHandle {
            commands: self.commands.clone(),
            has_key: self.has_key,
        }
    }

    pub async fn stop(self) {
        self.task.abort();
        self.client.shutdown().await;
    }
}

#[derive(Clone)]
pub struct NotifyHandle {
    commands: mpsc::UnboundedSender<Command>,
    has_key: bool,
}

impl NotifyHandle {
    /// The engine was given the vault, so it can change your mute list
    /// (once unlocked).
    pub fn has_key(&self) -> bool {
        self.has_key
    }

    /// Opal's own block list, without restarting the engine.
    pub fn set_blocked(&self, blocked: Vec<String>) {
        let _ = self.commands.send(Command::SetBlocked(blocked));
    }

    /// Add `target` to your mute list as a private (encrypted) entry, or
    /// remove it from both parts. Fetches the newest list first and keeps
    /// everything else in it. Needs the key.
    pub async fn set_muted(&self, target: PublicKey, mute: bool) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetMuted {
                target,
                mute,
                reply,
            })
            .map_err(|_| "notifications aren't running".to_string())?;
        rx.await.map_err(|_| MUTE_INTERRUPTED.to_string())?
    }

    /// The people on your mute list that Opal can read.
    pub async fn mutes(&self) -> Vec<MutedEntry> {
        let (reply, rx) = oneshot::channel();
        if self.commands.send(Command::Mutes { reply }).is_err() {
            return vec![];
        }
        rx.await.unwrap_or_default()
    }
}

struct Runner {
    me: PublicKey,
    me_hex: String,
    cfg: NotificationsConfig,
    store: NotifyStore,
    vault: Option<Arc<Vault>>,
    client: Client,
    events: broadcast::Sender<NotifyEvent>,
    status: Arc<RwLock<NotifyStatus>>,
    muted: HashSet<String>,
    started: u64,
    /// The mute list in effect (newest seen) and its private entries, kept
    /// so a failed fetch or a locked vault doesn't un-mute anyone.
    mute_event: Option<Event>,
    private_mutes: Option<(EventId, HashSet<String>)>,
}

/// The user's relay lists and mutes.
#[derive(Default)]
struct Lists {
    read: Vec<RelayUrl>,
    write: Vec<RelayUrl>,
    dm: Vec<RelayUrl>,
    mute_event: Option<Event>,
}

impl Runner {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        // First time for this account: the history we are about to catch up
        // on is not "new", so don't count it as unread.
        if self.store.last_seen(&self.me_hex).unwrap_or(0) == 0
            && self.store.last_read(&self.me_hex).unwrap_or(0) == 0
        {
            let _ = self
                .store
                .mark_read(&self.me_hex, self.started.saturating_sub(1));
        }
        let mut notifications = self.client.notifications();
        let bootstrap = parse_relays(&self.cfg.bootstrap_relays);
        for r in &bootstrap {
            let _ = self.client.add_relay(r).await;
        }
        self.client.connect().and_wait(CONNECT_WAIT).await;

        let lists = self.fetch_lists(&bootstrap).await.unwrap_or_default();
        let read = if lists.read.is_empty() {
            bootstrap.clone()
        } else {
            lists.read.clone()
        };
        let dm = if lists.dm.is_empty() {
            read.clone()
        } else {
            lists.dm.clone()
        };
        self.load_mute_cache().await;
        self.apply_mutes(lists.mute_event.as_ref()).await;
        for r in read.iter().chain(dm.iter()) {
            let _ = self.client.add_relay(r).await;
        }
        self.client.connect().and_wait(CONNECT_WAIT).await;

        let dms = self.cfg.types.dms && self.vault.is_some();
        {
            let mut st = self.status.write().await;
            st.read_relays = read.iter().map(|r| r.to_string()).collect();
            st.dm_relays = if dms {
                dm.iter().map(|r| r.to_string()).collect()
            } else {
                vec![]
            };
            st.muted = self.muted.len();
            st.dms = dms;
        }
        self.subscribe(&read, &dm, dms).await;
        self.emit_status(true).await;

        let (want_tx, want_rx) = mpsc::unbounded_channel::<Want>();
        tokio::spawn(backfill(
            self.client.clone(),
            self.store.clone(),
            self.events.clone(),
            want_rx,
        ));
        // Our own profile helps the UI; ask for it once.
        let _ = want_tx.send(Want::Profile(self.me_hex.clone()));

        if dms {
            self.process_pending_wraps(&want_tx).await;
        }
        let mut unlocked = self.vault.as_ref().map(|v| v.subscribe());
        let mut refresh = tokio::time::interval(REFRESH_EVERY);
        refresh.tick().await;

        loop {
            tokio::select! {
                n = notifications.next() => {
                    let Some(n) = n else { break };
                    if let ClientNotification::Event { relay_url, event, .. } = n {
                        self.handle(*event, relay_url, &want_tx).await;
                    }
                }
                changed = async {
                    match unlocked.as_mut() {
                        Some(rx) => rx.changed().await.is_ok(),
                        None => std::future::pending().await,
                    }
                } => {
                    if !changed { unlocked = None; continue; }
                    let is_unlocked = unlocked.as_ref().is_some_and(|rx| *rx.borrow());
                    if is_unlocked {
                        // Private mutes and waiting DMs need the key.
                        if let Some(lists) = self.fetch_lists(&bootstrap).await {
                            self.apply_mutes(lists.mute_event.as_ref()).await;
                        } else {
                            self.apply_mutes(None).await;
                        }
                        if dms {
                            self.process_pending_wraps(&want_tx).await;
                        }
                    }
                }
                Some(cmd) = commands.recv() => match cmd {
                    Command::SetMuted { target, mute, reply } => {
                        let r = self.set_muted(target, mute, &bootstrap).await;
                        let _ = reply.send(r);
                        self.emit_status(true).await;
                    }
                    Command::SetBlocked(blocked) => {
                        if self.cfg.blocked == blocked {
                            continue;
                        }
                        self.cfg.blocked = blocked;
                        self.apply_mutes(None).await;
                        self.emit_status(true).await;
                    }
                    Command::Mutes { reply } => {
                        let entries = self.mute_entries();
                        // Muted people never notify, so their profiles are
                        // only fetched for this list.
                        for e in &entries {
                            let _ = want_tx.send(Want::Profile(e.pubkey.clone()));
                        }
                        let _ = reply.send(entries);
                    }
                },
                _ = refresh.tick() => {
                    // A failed fetch keeps the mutes we have.
                    if let Some(lists) = self.fetch_lists(&bootstrap).await {
                        self.apply_mutes(lists.mute_event.as_ref()).await;
                    }
                    let _ = self.store.prune_orphans(&self.me_hex);
                    self.emit_status(true).await;
                }
            }
        }
    }

    async fn fetch_lists(&self, bootstrap: &[RelayUrl]) -> Option<Lists> {
        let filter = Filter::new().author(self.me).kinds([
            Kind::RelayList,
            Kind::Custom(10050),
            Kind::MuteList,
        ]);
        let targets: Vec<(RelayUrl, Vec<Filter>)> = bootstrap
            .iter()
            .map(|r| (r.clone(), vec![filter.clone()]))
            .collect();
        let events = match self
            .client
            .fetch_events(targets)
            .timeout(FETCH_TIMEOUT)
            .await
        {
            Ok(e) => e,
            Err(e) => {
                self.status.write().await.error = Some(format!("couldn't reach relays: {e}"));
                return None;
            }
        };
        self.status.write().await.error = None;
        let newest = |kind: Kind| {
            events
                .iter()
                .filter(|e| e.kind == kind && e.pubkey == self.me)
                .max_by_key(|e| e.created_at)
                .cloned()
        };
        let mut lists = Lists::default();
        let write = &mut lists.write;
        if let Some(ev) = newest(Kind::RelayList) {
            for (url, meta) in nip65::extract_relay_list(&ev) {
                if meta.is_none() || meta == Some(RelayMetadata::Read) {
                    lists.read.push(url.clone());
                }
                if meta.is_none() || meta == Some(RelayMetadata::Write) {
                    write.push(url);
                }
            }
        }
        if let Some(ev) = newest(Kind::Custom(10050)) {
            for t in ev.tags.iter() {
                let s = t.as_slice();
                if s.first().map(String::as_str) == Some("relay")
                    && let Some(url) = s.get(1).and_then(|u| RelayUrl::parse(u).ok())
                {
                    lists.dm.push(url);
                }
            }
        }
        lists.mute_event = newest(Kind::MuteList);
        // Your mute list lives on your own (write) relays too.
        let write = lists.write.clone();
        if !write.is_empty() {
            for r in &write {
                let _ = self.client.add_relay(r).await;
            }
            self.client.connect().and_wait(Duration::from_secs(3)).await;
            let f = Filter::new().author(self.me).kind(Kind::MuteList).limit(1);
            let targets: Vec<(RelayUrl, Vec<Filter>)> =
                write.iter().map(|r| (r.clone(), vec![f.clone()])).collect();
            if let Ok(found) = self
                .client
                .fetch_events(targets)
                .timeout(FETCH_TIMEOUT)
                .await
                && let Some(ev) = found
                    .iter()
                    .filter(|e| e.pubkey == self.me && e.kind == Kind::MuteList)
                    .max_by_key(|e| e.created_at)
                && lists
                    .mute_event
                    .as_ref()
                    .is_none_or(|m| ev.created_at > m.created_at)
            {
                lists.mute_event = Some(ev.clone());
            }
        }
        Some(lists)
    }

    /// Public `p` tags, plus the private ones when the key is available,
    /// plus Opal's own block list.
    /// Apply the newest mute list seen (a missing or older one never
    /// replaces it), its private entries when the key is available (the last
    /// decrypted set otherwise), plus Opal's own block list.
    async fn apply_mutes(&mut self, ev: Option<&Event>) {
        if let Some(ev) = ev
            && self
                .mute_event
                .as_ref()
                .is_none_or(|m| ev.created_at >= m.created_at)
        {
            self.mute_event = Some(ev.clone());
        }
        let mut muted: HashSet<String> = self
            .cfg
            .blocked
            .iter()
            .filter_map(|b| PublicKey::parse(b).ok())
            .map(|pk| pk.to_hex())
            .collect();
        if let Some(ev) = self.mute_event.clone() {
            muted.extend(p_tags(ev.tags.iter().map(|t| t.as_slice().to_vec())));
            if ev.content.is_empty() {
                // No private part (any more): nothing private is muted.
                if self
                    .private_mutes
                    .as_ref()
                    .is_none_or(|(id, _)| *id != ev.id)
                {
                    self.private_mutes = Some((ev.id, HashSet::new()));
                    self.save_mute_cache().await;
                }
            } else {
                let cached = self
                    .private_mutes
                    .as_ref()
                    .filter(|(id, _)| *id == ev.id)
                    .map(|(_, set)| set.clone());
                let private = match cached {
                    Some(set) => Some(set),
                    None => match self.keys().await {
                        Some(keys) => {
                            let set: Option<HashSet<String>> =
                                decrypt_private(&keys, &self.me, &ev.content)
                                    .map(|tags| p_tags(tags.into_iter()).into_iter().collect());
                            if let Some(set) = &set {
                                self.private_mutes = Some((ev.id, set.clone()));
                                self.save_mute_cache().await;
                            }
                            set
                        }
                        // Locked and never decrypted: keep what we had.
                        None => self.private_mutes.as_ref().map(|(_, s)| s.clone()),
                    },
                };
                if let Some(p) = private {
                    muted.extend(p);
                }
            }
        }
        let newly: Vec<String> = muted.difference(&self.muted).cloned().collect();
        if !newly.is_empty() && self.store.remove_authors(&self.me_hex, &newly).unwrap_or(0) > 0 {
            let _ = self.events.send(NotifyEvent::Updated);
        }
        self.muted = muted;
        self.status.write().await.muted = self.muted.len();
    }

    /// The private mutes decrypted before the last restart.
    async fn load_mute_cache(&mut self) {
        let Some(vault) = &self.vault else { return };
        let Ok(Some(json)) = vault.store().get(ItemKind::MuteCache, &self.me_hex).await else {
            return;
        };
        if let Ok(c) = serde_json::from_str::<MuteCache>(&json)
            && let Ok(id) = EventId::from_hex(&c.event)
        {
            let set = p_tags(c.pubkeys.into_iter().map(|pk| vec!["p".into(), pk]));
            self.private_mutes = Some((id, set.into_iter().collect()));
        }
    }

    async fn save_mute_cache(&self) {
        let (Some(vault), Some((id, set))) = (&self.vault, &self.private_mutes) else {
            return;
        };
        let mut pubkeys: Vec<String> = set.iter().cloned().collect();
        pubkeys.sort();
        let cache = MuteCache {
            event: id.to_hex(),
            pubkeys,
        };
        let Ok(json) = serde_json::to_string(&cache) else {
            return;
        };
        let label = format!("Opal private mutes ({})", &self.me_hex[..8]);
        if let Err(e) = vault
            .store()
            .put(ItemKind::MuteCache, &self.me_hex, &label, &json)
            .await
        {
            tracing::warn!("couldn't cache private mutes: {e}");
        }
    }

    /// See [`NotifyEngine::set_muted`]. Refuses to publish rather than risk
    /// dropping entries it couldn't read.
    async fn set_muted(
        &mut self,
        target: PublicKey,
        mute: bool,
        bootstrap: &[RelayUrl],
    ) -> Result<(), String> {
        let keys = self.keys().await.ok_or(MUTE_NEEDS_UNLOCK)?;
        // Its own client: answers are judged per relay, and waiting for a
        // connection means only the relays asked.
        let edit = Client::default();
        let r = self.edit_mutes(&edit, &keys, target, mute, bootstrap).await;
        edit.shutdown().await;
        let ev = r?;
        if let Some(ev) = ev {
            self.apply_mutes(Some(&ev)).await;
        }
        Ok(())
    }

    /// The published list, or None when nothing needed to change.
    async fn edit_mutes(
        &self,
        edit: &Client,
        keys: &Keys,
        target: PublicKey,
        mute: bool,
        bootstrap: &[RelayUrl],
    ) -> Result<Option<Event>, String> {
        let found = fetch_for_edit(edit, self.me, bootstrap).await?;
        let current = match (found.mute_event, self.mute_event.clone()) {
            (Some(a), Some(b)) => Some(if a.created_at >= b.created_at { a } else { b }),
            (a, b) => a.or(b),
        };
        // Nothing found, yet Opal has decrypted a list before: it exists somewhere.
        if current.is_none() && self.private_mutes.is_some() {
            return Err(MUTE_UNSURE.into());
        }
        if let Some(ev) = &current
            && ev.created_at.as_secs() > Timestamp::now().as_secs() + 600
        {
            return Err(
                "your mute list is dated in the future, so relays would refuse a newer one".into(),
            );
        }
        let mut public: Vec<Tag> = vec![];
        let mut private: Vec<Vec<String>> = vec![];
        // Keep NIP-04 for lists written that way, so clients that only read
        // NIP-04 don't lose the private part.
        let mut legacy = false;
        if let Some(ev) = &current {
            public = ev.tags.iter().cloned().collect();
            legacy = ev.content.contains("?iv=");
            if !ev.content.is_empty() {
                private = decrypt_private(keys, &self.me, &ev.content).ok_or(
                    "couldn't read the private part of your mute list, so it was left unchanged",
                )?;
            }
        }
        let hex = target.to_hex();
        let is_target =
            |t: &[String]| t.first().map(String::as_str) == Some("p") && t.get(1) == Some(&hex);
        let listed =
            public.iter().any(|t| is_target(t.as_slice())) || private.iter().any(|t| is_target(t));
        if mute == listed {
            return Ok(None);
        }
        if mute {
            private.push(vec!["p".into(), hex]);
        } else {
            public.retain(|t| !is_target(t.as_slice()));
            private.retain(|t| !is_target(t));
        }
        let content = if private.is_empty() {
            String::new()
        } else {
            let json = serde_json::to_string(&private).map_err(|e| e.to_string())?;
            if legacy {
                nip04::encrypt(keys.secret_key(), &self.me, json).map_err(|e| e.to_string())?
            } else {
                nip44::encrypt(keys.secret_key(), &self.me, json, nip44::Version::V2)
                    .map_err(|e| e.to_string())?
            }
        };
        // Strictly newer than the list it replaces, or relays keep the old one.
        let mut created = Timestamp::now();
        if let Some(old) = &current
            && created <= old.created_at
        {
            created = Timestamp::from(old.created_at.as_secs() + 1);
        }
        let ev = EventBuilder::new(Kind::MuteList, content)
            .tags(public)
            .custom_created_at(created)
            .finalize(keys)
            .map_err(|e| e.to_string())?;
        let mut relays: Vec<RelayUrl> = found.write;
        for r in bootstrap {
            if !relays.contains(r) {
                relays.push(r.clone());
            }
        }
        for r in &relays {
            let _ = edit.add_relay(r).await;
        }
        edit.connect().and_wait(CONNECT_WAIT).await;
        match edit.send_event(&ev).to(relays).await {
            Ok(out) if !out.success.is_empty() => {}
            Ok(out) => {
                return Err(format!(
                    "no relay accepted your mute list: {}",
                    out.failed.values().next().cloned().unwrap_or_default()
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
        Ok(Some(ev))
    }

    fn mute_entries(&self) -> Vec<MutedEntry> {
        let Some(ev) = &self.mute_event else {
            return vec![];
        };
        let mut out: Vec<MutedEntry> = p_tags(ev.tags.iter().map(|t| t.as_slice().to_vec()))
            .into_iter()
            .map(|pubkey| MutedEntry {
                pubkey,
                private: false,
            })
            .collect();
        if let Some((_, set)) = &self.private_mutes {
            let mut private: Vec<&String> = set.iter().collect();
            private.sort();
            for pk in private {
                if !out.iter().any(|e| &e.pubkey == pk) {
                    out.push(MutedEntry {
                        pubkey: pk.clone(),
                        private: true,
                    });
                }
            }
        }
        out
    }

    async fn keys(&self) -> Option<Keys> {
        let vault = self.vault.as_ref()?;
        vault.keys(&self.me).await.ok()
    }

    async fn subscribe(&self, read: &[RelayUrl], dm: &[RelayUrl], dms: bool) {
        let t = &self.cfg.types;
        let mut kinds = Vec::new();
        if t.replies || t.mentions {
            kinds.extend([Kind::TextNote, Kind::Comment]);
        }
        if t.reposts {
            kinds.extend([Kind::Repost, Kind::GenericRepost]);
        }
        if t.reactions {
            kinds.push(Kind::Reaction);
        }
        if t.zaps {
            kinds.push(Kind::ZapReceipt);
        }
        let now = Timestamp::now().as_secs();
        let last_seen = self.store.last_seen(&self.me_hex).unwrap_or(0);
        let since = last_seen
            .saturating_sub(600)
            .max(now.saturating_sub(MAX_CATCH_UP_SECS));
        if !kinds.is_empty() {
            let feed = Filter::new()
                .kinds(kinds)
                .pubkey(self.me)
                .since(Timestamp::from(since))
                .limit(200);
            let targets: Vec<(RelayUrl, Vec<Filter>)> = read
                .iter()
                .map(|r| (r.clone(), vec![feed.clone()]))
                .collect();
            if let Err(e) = self
                .client
                .subscribe(targets)
                .with_id(SubscriptionId::new(FEED_SUB))
                .await
            {
                self.status.write().await.error = Some(e.to_string());
            }
        }
        if dms {
            let wraps = Filter::new()
                .kind(Kind::GiftWrap)
                .pubkey(self.me)
                .since(Timestamp::from(since.saturating_sub(WRAP_BACKDATE_SECS)))
                .limit(200);
            let targets: Vec<(RelayUrl, Vec<Filter>)> = dm
                .iter()
                .map(|r| (r.clone(), vec![wraps.clone()]))
                .collect();
            let _ = self
                .client
                .subscribe(targets)
                .with_id(SubscriptionId::new(DM_SUB))
                .await;
        }
    }

    async fn emit_status(&self, synced: bool) {
        let status = {
            let mut st = self.status.write().await;
            st.synced = synced;
            st.clone()
        };
        let _ = self.events.send(NotifyEvent::Status { status });
    }

    fn type_enabled(&self, t: NotifType) -> bool {
        let ty = &self.cfg.types;
        match t {
            NotifType::Reply => ty.replies,
            NotifType::Mention => ty.mentions,
            NotifType::Repost => ty.reposts,
            NotifType::Reaction => ty.reactions,
            NotifType::Zap => ty.zaps,
            NotifType::Dm => ty.dms,
        }
    }

    async fn handle(&mut self, event: Event, relay: RelayUrl, want: &mpsc::UnboundedSender<Want>) {
        if event.kind == Kind::GiftWrap {
            self.handle_wrap(&event, Timestamp::now().as_secs(), want)
                .await;
            return;
        }
        if event.verify().is_err() {
            return;
        }
        let now = Timestamp::now().as_secs();
        // Far-future events would sit on top and stay unread forever.
        if event.created_at.as_secs() > now.saturating_add(600) {
            return;
        }
        let Some(mut n) = classify(&event, &self.me) else {
            return;
        };
        if self.muted.contains(&event.pubkey.to_hex()) {
            return;
        }
        if self.muted.contains(&n.author) || !self.type_enabled(n.ntype) {
            return;
        }
        n.relay = Some(relay.to_string());
        if !self.store.insert(&self.me_hex, &n, now).unwrap_or(false) {
            return;
        }
        if n.created_at <= now.saturating_add(60) {
            let _ = self.store.set_last_seen(&self.me_hex, n.created_at);
        }
        let _ = want.send(Want::Profile(n.author.clone()));
        if let Some(r) = &n.ref_id {
            let _ = want.send(Want::Event(r.clone()));
        }
        let fresh = n.created_at.saturating_add(120) >= self.started;
        let _ = self.events.send(NotifyEvent::New {
            notification: n,
            fresh,
        });
    }

    async fn handle_wrap(
        &mut self,
        event: &Event,
        received: u64,
        want: &mpsc::UnboundedSender<Want>,
    ) {
        let Some(keys) = self.keys().await else {
            // Locked: keep it until the key is available.
            let _ = self.store.add_pending_wrap(
                &self.me_hex,
                &event.id.to_hex(),
                &event.as_json(),
                received,
            );
            return;
        };
        let Ok(gift) = UnwrappedGift::from_gift_wrap(&keys, event) else {
            return;
        };
        let rumor = gift.rumor;
        if gift.sender == self.me || !matches!(rumor.kind.as_u16(), 14 | 15) {
            return;
        }
        let sender = gift.sender.to_hex();
        if self.muted.contains(&sender) {
            return;
        }
        let detail = if self.cfg.dm_previews {
            let text: String = rumor
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            text.chars().take(280).collect()
        } else {
            String::new()
        };
        let n = Notification {
            id: event.id.to_hex(),
            ntype: NotifType::Dm,
            kind: rumor.kind.as_u16(),
            author: sender.clone(),
            // The rumor's time is chosen by the sender and hidden from relays;
            // never let it be later than when the message actually arrived.
            created_at: rumor.created_at.as_secs().min(received),
            detail,
            ref_id: None,
            sats: None,
            media: None,
            relay: None,
        };
        if !self
            .store
            .insert(&self.me_hex, &n, received)
            .unwrap_or(false)
        {
            return;
        }
        let _ = want.send(Want::Profile(sender));
        let fresh = received.saturating_add(120) >= self.started
            && n.created_at.saturating_add(600) >= self.started;
        let _ = self.events.send(NotifyEvent::New {
            notification: n,
            fresh,
        });
    }

    async fn process_pending_wraps(&mut self, want: &mpsc::UnboundedSender<Want>) {
        if self.keys().await.is_none() {
            return;
        }
        for (json, received) in self
            .store
            .take_pending_wraps(&self.me_hex)
            .unwrap_or_default()
        {
            if let Ok(ev) = Event::from_json(&json) {
                self.handle_wrap(&ev, received, want).await;
            }
        }
    }
}

/// The newest mute list, read carefully enough to publish over. Your
/// newest relay list must be found (following it to its own relays), and
/// most of its write relays must finish answering.
async fn fetch_for_edit(
    client: &Client,
    me: PublicKey,
    bootstrap: &[RelayUrl],
) -> Result<Found, String> {
    let filter = Filter::new()
        .author(me)
        .kinds([Kind::RelayList, Kind::MuteList]);
    let newest = |answers: &[(RelayUrl, Vec<Event>)], kind: Kind| {
        answers
            .iter()
            .flat_map(|(_, evs)| evs)
            .filter(|e| e.kind == kind)
            .max_by_key(|e| e.created_at)
            .cloned()
    };
    let mut asked: Vec<RelayUrl> = bootstrap.to_vec();
    let mut answers = fetch_each(client, me, bootstrap, &filter).await;
    let mut relay_list: Option<Event> = None;
    // A newer relay list may only be on the relays an older one names.
    for _ in 0..3 {
        let Some(list) = newest(&answers, Kind::RelayList) else {
            break;
        };
        if relay_list.as_ref().is_some_and(|l| l.id == list.id) {
            break;
        }
        let mut fresh: Vec<RelayUrl> = nip65::extract_relay_list(&list)
            .map(|(url, _)| url.clone())
            .filter(|u| !asked.contains(u))
            .collect();
        fresh.sort();
        fresh.dedup();
        relay_list = Some(list);
        if fresh.is_empty() {
            break;
        }
        asked.extend(fresh.iter().cloned());
        answers.extend(fetch_each(client, me, &fresh, &filter).await);
    }
    let Some(relay_list) = relay_list else {
        // Only "there is none" when some relay actually answered.
        return Err(if answers.is_empty() {
            MUTE_UNSURE
        } else {
            MUTE_NO_RELAY_LIST
        }
        .into());
    };
    if newest(&answers, Kind::RelayList).is_some_and(|l| l.id != relay_list.id) {
        return Err(MUTE_UNSURE.into());
    }
    let mut write: Vec<RelayUrl> = nip65::extract_relay_list(&relay_list)
        .filter(|(_, meta)| meta.is_none() || *meta == Some(RelayMetadata::Write))
        .map(|(url, _)| url.clone())
        .collect();
    // The same relay listed twice is still one relay.
    write.sort();
    write.dedup();
    let answered = write
        .iter()
        .filter(|w| answers.iter().any(|(url, _)| url == *w))
        .count();
    if write.is_empty() || answered * 2 <= write.len() {
        return Err(MUTE_UNSURE.into());
    }
    Ok(Found {
        mute_event: newest(&answers, Kind::MuteList),
        write,
    })
}

/// Ask each relay on its own. A relay is returned only if it sent EOSE in
/// time, with only your own events. The SDK's fetch can't tell: it ends the
/// same way on EOSE, a disconnect or an unrecognised CLOSED.
async fn fetch_each(
    client: &Client,
    me: PublicKey,
    relays: &[RelayUrl],
    filter: &Filter,
) -> Vec<(RelayUrl, Vec<Event>)> {
    let asks = relays.iter().map(|url| async move {
        let evs = tokio::time::timeout(
            CONNECT_WAIT + FETCH_TIMEOUT,
            ask_until_eose(client, url, filter.clone()),
        )
        .await
        .ok()??;
        let evs = evs
            .into_iter()
            .filter(|e| e.pubkey == me && e.verify().is_ok())
            .collect();
        Some((url.clone(), evs))
    });
    futures::future::join_all(asks)
        .await
        .into_iter()
        .flatten()
        .collect()
}

async fn ask_until_eose(client: &Client, url: &RelayUrl, filter: Filter) -> Option<Vec<Event>> {
    client.add_relay(url).await.ok()?;
    let relay = client.relay(url).await.ok()??;
    relay.try_connect().timeout(CONNECT_WAIT).await.ok()?;
    let mut notes = relay.notifications();
    let id = SubscriptionId::generate();
    relay
        .send_msg(ClientMessage::req(id.clone(), vec![filter]))
        .await
        .ok()?;
    let mut evs = Vec::new();
    let done = loop {
        match notes.next().await {
            Some(RelayNotification::Message { message }) => match *message {
                RelayMessage::Event {
                    subscription_id,
                    event,
                } if *subscription_id == id => evs.push(event.into_owned()),
                RelayMessage::EndOfStoredEvents(sid) if *sid == id => break true,
                RelayMessage::Closed {
                    subscription_id, ..
                } if *subscription_id == id => break false,
                _ => {}
            },
            Some(RelayNotification::RelayStatus { status }) if !status.is_connected() => {
                break false;
            }
            Some(_) => {}
            None => break false,
        }
    };
    let _ = relay.send_msg(ClientMessage::close(id)).await;
    done.then_some(evs)
}

/// See [`Runner::fetch_for_edit`].
struct Found {
    mute_event: Option<Event>,
    write: Vec<RelayUrl>,
}

/// What the keyring holds for [`ItemKind::MuteCache`].
#[derive(Serialize, Deserialize)]
struct MuteCache {
    /// The mute list event the entries were decrypted from.
    event: String,
    pubkeys: Vec<String>,
}

/// The private tags of a NIP-51 list (NIP-44, or NIP-04 in older lists).
fn decrypt_private(keys: &Keys, me: &PublicKey, content: &str) -> Option<Vec<Vec<String>>> {
    let plain = if content.contains("?iv=") {
        nip04::decrypt(keys.secret_key(), me, content).ok()
    } else {
        nip44::decrypt(keys.secret_key(), me, content).ok()
    }?;
    serde_json::from_str(&plain).ok()
}

fn p_tags(tags: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    tags.filter(|t| t.first().map(String::as_str) == Some("p"))
        .filter_map(|t| t.get(1).and_then(|h| PublicKey::from_hex(h).ok()))
        .map(|pk| pk.to_hex())
        .collect()
}

fn parse_relays(list: &[String]) -> Vec<RelayUrl> {
    list.iter()
        .filter_map(|r| RelayUrl::parse(r).ok())
        .collect()
}

enum Want {
    Profile(String),
    Event(String),
}

/// Batches profile and referenced-note lookups.
async fn backfill(
    client: Client,
    store: NotifyStore,
    events: broadcast::Sender<NotifyEvent>,
    mut rx: mpsc::UnboundedReceiver<Want>,
) {
    loop {
        let Some(first) = rx.recv().await else { return };
        // Collect whatever else arrives in the next moment.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let mut wants = vec![first];
        while let Ok(w) = rx.try_recv() {
            wants.push(w);
        }
        let now = Timestamp::now().as_secs();
        let mut authors = HashSet::new();
        let mut ids = HashSet::new();
        for w in wants {
            match w {
                Want::Profile(pk) => {
                    let stale = store
                        .profile(&pk)
                        .ok()
                        .flatten()
                        .is_none_or(|(_, at)| now.saturating_sub(at) > PROFILE_TTL_SECS);
                    if stale && let Ok(pk) = PublicKey::from_hex(&pk) {
                        authors.insert(pk);
                    }
                }
                Want::Event(id) => {
                    if !store.has_ref_event(&id).unwrap_or(true)
                        && let Ok(id) = EventId::from_hex(&id)
                    {
                        ids.insert(id);
                    }
                }
            }
        }
        let mut filters = Vec::new();
        // Relays refuse filters with too many authors (a long mute list).
        let listed: Vec<PublicKey> = authors.iter().copied().collect();
        for chunk in listed.chunks(100) {
            filters.push(
                Filter::new()
                    .kind(Kind::Metadata)
                    .authors(chunk.iter().copied()),
            );
        }
        if !ids.is_empty() {
            filters.push(Filter::new().ids(ids));
        }
        if filters.is_empty() {
            continue;
        }
        let Ok(found) = client.fetch_events(filters).timeout(FETCH_TIMEOUT).await else {
            continue;
        };
        let mut changed = false;
        let mut profiles: Vec<&Event> = found.iter().filter(|e| e.kind == Kind::Metadata).collect();
        profiles.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        let mut done = HashSet::new();
        for ev in profiles {
            if done.insert(ev.pubkey) {
                let _ = store.put_profile(&ev.pubkey.to_hex(), &parse_profile(&ev.content), now);
                changed = true;
            }
        }
        // Authors with no profile anywhere: don't ask again for a day.
        for pk in authors.iter().filter(|pk| !done.contains(*pk)) {
            let _ = store.put_profile(&pk.to_hex(), &Profile::default(), now);
        }
        for ev in found.iter().filter(|e| e.kind != Kind::Metadata) {
            let content = if ev.kind == Kind::Repost || ev.kind == Kind::GenericRepost {
                String::new()
            } else {
                ev.content.clone()
            };
            let _ = store.put_ref_event(
                &ev.id.to_hex(),
                &ev.pubkey.to_hex(),
                ev.kind.as_u16(),
                &content,
            );
            changed = true;
        }
        if changed {
            let _ = events.send(NotifyEvent::Updated);
        }
    }
}

pub fn parse_profile(content: &str) -> Profile {
    let v: serde_json::Value = serde_json::from_str(content).unwrap_or_default();
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    Profile {
        name: s("name").map(|v| v.chars().take(64).collect()),
        display_name: s("display_name")
            .or_else(|| s("displayName"))
            .map(|v| v.chars().take(64).collect()),
        picture: s("picture").filter(|p| p.starts_with("https://") && p.len() < 2048),
        nip05: s("nip05").map(|v| v.chars().take(100).collect()),
        about: None,
    }
}
