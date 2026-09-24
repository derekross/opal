//! The live part: finds your relays (NIP-65), your mute list (NIP-51, private
//! entries too when unlocked), subscribes to events that tag you, stores them
//! as notifications and fills in profiles and the notes they refer to.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nostr_sdk::prelude::*;
use opal_core::accounts::Profile;
use opal_core::config::NotificationsConfig;
use opal_core::vault::Vault;
use serde::Serialize;
use tokio::sync::{RwLock, broadcast, mpsc};
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

pub struct NotifyEngine {
    task: JoinHandle<()>,
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
        };
        let task = tokio::spawn(run.run());
        Self {
            task,
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

    pub async fn stop(self) {
        self.task.abort();
        self.client.shutdown().await;
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
}

/// The user's relay lists and mutes.
#[derive(Default)]
struct Lists {
    read: Vec<RelayUrl>,
    dm: Vec<RelayUrl>,
    mute_event: Option<Event>,
}

impl Runner {
    async fn run(mut self) {
        let mut notifications = self.client.notifications();
        let bootstrap = parse_relays(&self.cfg.bootstrap_relays);
        for r in &bootstrap {
            let _ = self.client.add_relay(r).await;
        }
        self.client.connect().and_wait(CONNECT_WAIT).await;

        let lists = self.fetch_lists(&bootstrap).await;
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
                        let lists = self.fetch_lists(&bootstrap).await;
                        self.apply_mutes(lists.mute_event.as_ref()).await;
                        if dms {
                            self.process_pending_wraps(&want_tx).await;
                        }
                    }
                }
                _ = refresh.tick() => {
                    let lists = self.fetch_lists(&bootstrap).await;
                    self.apply_mutes(lists.mute_event.as_ref()).await;
                    self.emit_status(true).await;
                }
            }
        }
    }

    async fn fetch_lists(&self, bootstrap: &[RelayUrl]) -> Lists {
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
                return Lists::default();
            }
        };
        let newest = |kind: Kind| {
            events
                .iter()
                .filter(|e| e.kind == kind && e.pubkey == self.me)
                .max_by_key(|e| e.created_at)
                .cloned()
        };
        let mut lists = Lists::default();
        if let Some(ev) = newest(Kind::RelayList) {
            for (url, meta) in nip65::extract_relay_list(&ev) {
                if meta.is_none() || meta == Some(RelayMetadata::Read) {
                    lists.read.push(url);
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
        lists
    }

    /// Public `p` tags, plus the private ones when the key is available,
    /// plus Opal's own block list.
    async fn apply_mutes(&mut self, ev: Option<&Event>) {
        let mut muted: HashSet<String> = self
            .cfg
            .blocked
            .iter()
            .filter_map(|b| PublicKey::parse(b).ok())
            .map(|pk| pk.to_hex())
            .collect();
        if let Some(ev) = ev {
            muted.extend(p_tags(ev.tags.iter().map(|t| t.as_slice().to_vec())));
            if !ev.content.is_empty()
                && let Some(keys) = self.keys().await
            {
                let plain = if ev.content.contains("?iv=") {
                    nip04::decrypt(keys.secret_key(), &self.me, &ev.content).ok()
                } else {
                    nip44::decrypt(keys.secret_key(), &self.me, &ev.content).ok()
                };
                if let Some(tags) =
                    plain.and_then(|p| serde_json::from_str::<Vec<Vec<String>>>(&p).ok())
                {
                    muted.extend(p_tags(tags.into_iter()));
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
        let Some(mut n) = classify(&event, &self.me) else {
            return;
        };
        if self.muted.contains(&n.author) || !self.type_enabled(n.ntype) {
            return;
        }
        n.relay = Some(relay.to_string());
        let now = Timestamp::now().as_secs();
        if !self.store.insert(&self.me_hex, &n, now).unwrap_or(false) {
            return;
        }
        if n.created_at <= now + 60 {
            let _ = self.store.set_last_seen(&self.me_hex, n.created_at);
        }
        let _ = want.send(Want::Profile(n.author.clone()));
        if let Some(r) = &n.ref_id {
            let _ = want.send(Want::Event(r.clone()));
        }
        let fresh = n.created_at + 120 >= self.started;
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
            created_at: rumor.created_at.as_secs(),
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
        let fresh = received + 120 >= self.started && n.created_at + 600 >= self.started;
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
        if !authors.is_empty() {
            filters.push(Filter::new().kind(Kind::Metadata).authors(authors.clone()));
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
        name: s("name"),
        display_name: s("display_name").or_else(|| s("displayName")),
        picture: s("picture").filter(|p| p.starts_with("https://")),
        nip05: s("nip05"),
        about: None,
    }
}
