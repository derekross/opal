//! The NIP-46 remote signer: listens for kind 24133 requests on relays,
//! checks them, asks the [`Approver`], and replies.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nostr_sdk::prelude::*;
use opal_core::config::Policy;
use opal_core::vault::Vault;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};

use crate::approver::{ApprovalRequest, Approver, Decision};
use crate::connection::{ConnStore, Connection, ConnectionInfo};
use crate::permissions::{Rule, Source};
use crate::perms::parse_perms;
use crate::protocol::{Method, Request, Response, Transport};
use crate::store::{ActivityEntry, SignerStore, hash_secret};
use crate::uri::{NostrConnectUri, bunker_uri};

/// Requests older or newer than this are ignored (replays, broken clocks).
const MAX_CLOCK_SKEW_SECS: u64 = 300;
const SEEN_CAPACITY: usize = 10_000;
/// Requests handled at the same time (each may wait for a prompt).
const MAX_CONCURRENT_REQUESTS: usize = 32;
/// How long to wait for relays to connect before carrying on anyway.
const RELAY_CONNECT_WAIT: Duration = Duration::from_secs(5);
/// Activity older than this is pruned.
const ACTIVITY_RETENTION_SECS: u64 = 90 * 86_400;

#[derive(Debug, Clone)]
pub struct SignerSettings {
    pub default_relays: Vec<RelayUrl>,
    /// How long a request may wait for an unlock or a decision.
    pub pending_timeout: Duration,
    /// Record requests in the activity log (off = privacy mode).
    pub log_activity: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    #[error(transparent)]
    Core(#[from] opal_core::Error),
    #[error("relay client: {0}")]
    Client(String),
    #[error("unknown account")]
    UnknownAccount,
    #[error("unknown app")]
    UnknownApp,
    #[error("no signer store")]
    NoStore,
}

/// Things the UI wants to hear about.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignerEvent {
    Connected {
        connection: ConnectionInfo,
    },
    Updated {
        connection: ConnectionInfo,
    },
    Disconnected {
        connection_id: String,
    },
    UnlockNeeded {
        connection_id: String,
        app_name: String,
        method: Method,
    },
    Request {
        connection_id: String,
        app_name: String,
        method: Method,
        kind: Option<u16>,
        allowed: bool,
        source: Source,
        reason: Option<String>,
        at: u64,
    },
}

#[derive(Clone)]
pub struct Signer {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    client: Client,
    pub(crate) vault: Arc<Vault>,
    pub(crate) conns: ConnStore,
    pub(crate) store: Option<SignerStore>,
    approver: Arc<dyn Approver>,
    settings: SignerSettings,
    seen: Mutex<Seen>,
    events: broadcast::Sender<SignerEvent>,
    sub_id: SubscriptionId,
    unlock_nags: Mutex<std::collections::HashMap<String, u64>>,
    /// Caps how many requests are handled at once.
    busy: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct Seen {
    set: HashSet<EventId>,
    order: VecDeque<EventId>,
}

impl Seen {
    /// Returns `false` if the id was already seen.
    fn insert(&mut self, id: EventId) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > SEEN_CAPACITY
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        true
    }
}

/// The `sign_event` parameter: an event template.
#[derive(Deserialize)]
struct EventTemplate {
    kind: u16,
    #[serde(default)]
    content: String,
    #[serde(default)]
    tags: Vec<Vec<String>>,
    created_at: Option<u64>,
}

/// Optional 4th `connect` parameter. Display only, never trusted.
#[derive(Deserialize, Default)]
struct ClientMetadata {
    name: Option<String>,
    url: Option<String>,
    image: Option<String>,
}

type Reply = Result<String, String>;

/// What a request wants done with the account key, once its parameters
/// have been checked. Shared by NIP-46 apps and local apps.
#[derive(Debug, Clone)]
pub enum Op {
    GetPublicKey,
    SignEvent(UnsignedEvent),
    /// NIP-04/NIP-44 encrypt or decrypt `text` with `with`.
    Cipher {
        method: Method,
        with: PublicKey,
        text: String,
    },
}

impl Op {
    pub fn method(&self) -> Method {
        match self {
            Self::GetPublicKey => Method::GetPublicKey,
            Self::SignEvent(_) => Method::SignEvent,
            Self::Cipher { method, .. } => method.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Outcome {
    PublicKey(PublicKey),
    Event(Event),
    Text(String),
}

/// Who is asking: the part of an [`ApprovalRequest`] that comes from the
/// app itself.
#[derive(Debug, Clone)]
pub(crate) struct Requester {
    pub connection_id: String,
    pub app_name: String,
    pub app_url: Option<String>,
    pub app_image: Option<String>,
    pub account: PublicKey,
    pub policy: Policy,
}

impl Requester {
    fn nip46(c: &Connection) -> Self {
        Self {
            connection_id: c.id.clone(),
            app_name: c.display_name(),
            app_url: c.url.clone(),
            app_image: c.image.clone(),
            account: c.account,
            policy: c.policy,
        }
    }
}

/// What to do when the vault is locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WhenLocked {
    /// Tell the UI once a minute and wait for an unlock (remote apps).
    WaitAndNag,
    /// Refuse at once, unlogged (local daemons that retry on their own).
    FailFast,
}

impl Signer {
    /// A signer that keeps connections in memory only (tests, dev tools).
    pub fn new(vault: Arc<Vault>, approver: Arc<dyn Approver>, settings: SignerSettings) -> Self {
        Self::build(vault, approver, settings, ConnStore::memory(), None)
    }

    /// A signer that saves apps, rules and activity in `store`.
    pub async fn with_store(
        vault: Arc<Vault>,
        approver: Arc<dyn Approver>,
        settings: SignerSettings,
        store: SignerStore,
    ) -> Result<Self, SignerError> {
        let conns = ConnStore::load(store.clone(), vault.clone()).await?;
        Ok(Self::build(vault, approver, settings, conns, Some(store)))
    }

    fn build(
        vault: Arc<Vault>,
        approver: Arc<dyn Approver>,
        settings: SignerSettings,
        conns: ConnStore,
        store: Option<SignerStore>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(Inner {
                client: Client::default(),
                vault,
                conns,
                store,
                approver,
                settings,
                seen: Mutex::new(Seen::default()),
                events,
                sub_id: SubscriptionId::new("opal-nip46"),
                unlock_nags: Mutex::new(std::collections::HashMap::new()),
                busy: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            }),
        }
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<SignerEvent> {
        self.inner.events.subscribe()
    }

    pub fn store(&self) -> Option<&SignerStore> {
        self.inner.store.as_ref()
    }

    /// Connect to relays and start answering requests.
    pub async fn start(&self) -> Result<tokio::task::JoinHandle<()>, SignerError> {
        let inner = self.inner.clone();
        let mut notifications = inner.client.notifications();
        let mut relays = inner.settings.default_relays.clone();
        for r in inner.conns.relays().await {
            if !relays.contains(&r) {
                relays.push(r);
            }
        }
        inner.ensure_relays(&relays).await?;
        inner.resubscribe().await?;
        Ok(tokio::spawn(async move {
            while let Some(n) = notifications.next().await {
                if let ClientNotification::Event {
                    relay_url, event, ..
                } = n
                    && event.kind == Kind::NostrConnect
                {
                    // Over the limit, requests wait here instead of piling up tasks.
                    let Ok(permit) = inner.busy.clone().acquire_owned().await else {
                        break;
                    };
                    let inner = inner.clone();
                    tokio::spawn(async move {
                        inner.handle_event(*event, relay_url).await;
                        drop(permit);
                    });
                }
            }
        }))
    }

    /// Disconnect from all relays (kill switch) or reconnect.
    pub async fn set_online(&self, online: bool) {
        if online {
            self.inner.client.connect().await;
            if let Err(e) = self.inner.resubscribe().await {
                tracing::warn!("resubscribing failed: {e}");
            }
        } else {
            self.inner.client.disconnect().await;
        }
    }

    pub async fn shutdown(&self) {
        self.inner.client.shutdown().await;
    }

    pub async fn connections(&self) -> Vec<ConnectionInfo> {
        self.inner
            .conns
            .all()
            .await
            .iter()
            .map(Connection::info)
            .collect()
    }

    pub async fn connection(&self, id: &str) -> Option<ConnectionInfo> {
        self.inner.conns.get(id).await.map(|c| c.info())
    }

    /// Signer-initiated flow: returns the new connection and its `bunker://` URI.
    pub async fn create_bunker(
        &self,
        account: PublicKey,
        name: Option<String>,
        relays: Option<Vec<RelayUrl>>,
        policy: Policy,
        unused_ttl: Option<Duration>,
    ) -> Result<(ConnectionInfo, String), SignerError> {
        let inner = &self.inner;
        inner.check_account(&account).await?;
        let relays = relays
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| inner.settings.default_relays.clone());
        let secret = random_hex(16);
        let now = Timestamp::now();
        let conn = Connection {
            id: random_hex(16),
            account,
            transport: Keys::generate(),
            client: None,
            secret_hash: Some(hash_secret(&secret)),
            name: name.filter(|n| !n.trim().is_empty()),
            url: None,
            image: None,
            relays: relays.clone(),
            policy,
            requested_perms: Vec::new(),
            created_at: now,
            last_used: None,
            expires_unused_at: unused_ttl.map(|d| now + d),
        };
        let uri = bunker_uri(&conn.transport.public_key(), &relays, Some(&secret));
        let info = conn.info();
        inner.ensure_relays(&relays).await?;
        inner.conns.insert(conn).await?;
        inner.resubscribe().await?;
        Ok((info, uri))
    }

    /// Client-initiated flow: accept a `nostrconnect://` URI the user approved.
    /// `grant` lists `(method, kind)` pairs to allow from now on.
    pub async fn accept_nostrconnect(
        &self,
        uri: &NostrConnectUri,
        account: PublicKey,
        policy: Policy,
        grant: &[(Method, Option<u16>)],
    ) -> Result<ConnectionInfo, SignerError> {
        let inner = &self.inner;
        inner.check_account(&account).await?;
        let now = Timestamp::now();
        let conn = Connection {
            id: random_hex(16),
            account,
            transport: Keys::generate(),
            client: Some(uri.client),
            secret_hash: None,
            name: uri
                .name
                .as_deref()
                .map(clean_label)
                .filter(|s| !s.is_empty()),
            url: uri.url.clone().filter(|u| u.starts_with("https://")),
            image: uri.image.clone().filter(|u| u.starts_with("https://")),
            relays: uri.relays.clone(),
            policy,
            requested_perms: uri.perms.clone(),
            created_at: now,
            last_used: Some(now),
            expires_unused_at: None,
        };
        inner.ensure_relays(&conn.relays).await?;
        inner.conns.insert(conn.clone()).await?;
        if let Some(store) = &inner.store {
            for (method, kind) in grant {
                store.put_rule(&Rule {
                    app_id: conn.id.clone(),
                    method: method.clone(),
                    kind: *kind,
                    allow: true,
                    until: None,
                    created_at: now.as_secs(),
                })?;
            }
        }
        inner.resubscribe().await?;
        // The client learns our key from the author of this response and
        // checks the echoed secret.
        let res = Response::ok(random_hex(16), uri.secret.clone());
        inner
            .reply(&conn, &uri.client, Transport::Nip44, &res, None)
            .await;
        let info = conn.info();
        inner.emit(SignerEvent::Connected {
            connection: info.clone(),
        });
        Ok(info)
    }

    pub async fn remove_connection(&self, id: &str) -> Result<bool, SignerError> {
        let removed = self.inner.conns.remove(id).await.is_some();
        if removed {
            self.inner.resubscribe().await?;
            self.inner.emit(SignerEvent::Disconnected {
                connection_id: id.to_string(),
            });
        }
        Ok(removed)
    }

    /// Change an app's name, policy or relays.
    pub async fn update_connection(
        &self,
        id: &str,
        name: Option<String>,
        policy: Option<Policy>,
        relays: Option<Vec<RelayUrl>>,
    ) -> Result<ConnectionInfo, SignerError> {
        if let Some(r) = &relays {
            self.inner.ensure_relays(r).await?;
        }
        let updated = self
            .inner
            .conns
            .update(id, |c| {
                if let Some(n) = name {
                    c.name = Some(n).filter(|n| !n.trim().is_empty());
                }
                if let Some(p) = policy {
                    c.policy = p;
                }
                if let Some(r) = relays.filter(|r| !r.is_empty()) {
                    c.relays = r;
                }
            })
            .await
            .ok_or(SignerError::UnknownApp)?;
        let info = updated.info();
        self.inner.emit(SignerEvent::Updated {
            connection: info.clone(),
        });
        Ok(info)
    }

    /// Housekeeping: drop unused expired bunker URIs, old request ids and
    /// old activity. Call periodically.
    pub async fn prune(&self) -> Result<(), SignerError> {
        let now = Timestamp::now();
        for c in self.inner.conns.all().await {
            if c.client.is_none() && c.expires_unused_at.is_some_and(|t| t <= now) {
                self.remove_connection(&c.id).await?;
            }
        }
        if let Some(store) = &self.inner.store {
            store.prune_seen(now.as_secs().saturating_sub(2 * MAX_CLOCK_SKEW_SECS))?;
            store.prune_activity(now.as_secs().saturating_sub(ACTIVITY_RETENTION_SECS))?;
        }
        Ok(())
    }
}

impl Inner {
    pub(crate) fn emit(&self, e: SignerEvent) {
        let _ = self.events.send(e);
    }

    /// "Unlock needed" at most once a minute per app.
    async fn unlock_nag_due(&self, conn_id: &str) -> bool {
        let now = Timestamp::now().as_secs();
        let mut last = self.unlock_nags.lock().await;
        match last.get(conn_id) {
            Some(t) if now.saturating_sub(*t) < 60 => false,
            _ => {
                last.insert(conn_id.to_string(), now);
                true
            }
        }
    }

    pub(crate) async fn check_account(&self, account: &PublicKey) -> Result<(), SignerError> {
        if self.vault.accounts().await?.contains(account) {
            Ok(())
        } else {
            Err(SignerError::UnknownAccount)
        }
    }

    async fn ensure_relays(&self, relays: &[RelayUrl]) -> Result<(), SignerError> {
        for r in relays {
            self.client
                .add_relay(r)
                .await
                .map_err(|e| SignerError::Client(e.to_string()))?;
        }
        // Requests are ephemeral: relays don't keep them for late subscribers,
        // so don't report ready before we are actually listening.
        self.client.connect().and_wait(RELAY_CONNECT_WAIT).await;
        Ok(())
    }

    /// Subscribe on each relay only to the apps that use it, so one app's
    /// relay can't link your other apps together, and let go of relays no
    /// app uses any more.
    async fn resubscribe(&self) -> Result<(), SignerError> {
        let conns = self.conns.all().await;
        let since = Timestamp::now() - Duration::from_secs(60);
        let mut per_relay: Vec<(RelayUrl, Vec<PublicKey>)> = Vec::new();
        for c in &conns {
            for r in &c.relays {
                match per_relay.iter_mut().find(|(u, _)| u == r) {
                    Some((_, keys)) => keys.push(c.transport.public_key()),
                    None => per_relay.push((r.clone(), vec![c.transport.public_key()])),
                }
            }
        }
        // Drop relays nothing needs (the default ones stay connected).
        for url in self.client.relays().await.into_keys() {
            let used = per_relay.iter().any(|(u, _)| *u == url)
                || self.settings.default_relays.contains(&url);
            if !used {
                let _ = self.client.remove_relay(&url).force().await;
            }
        }
        let _ = self.client.unsubscribe(&self.sub_id).await;
        if per_relay.is_empty() {
            return Ok(());
        }
        let targets: Vec<(RelayUrl, Vec<Filter>)> = per_relay
            .into_iter()
            .map(|(url, keys)| {
                let f = Filter::new()
                    .kind(Kind::NostrConnect)
                    .pubkeys(keys)
                    .since(since);
                (url, vec![f])
            })
            .collect();
        self.client
            .subscribe(targets)
            .with_id(self.sub_id.clone())
            .await
            .map_err(|e| SignerError::Client(e.to_string()))?;
        Ok(())
    }

    /// `false` if this request was handled before (also across restarts).
    async fn first_time(&self, event: &Event) -> bool {
        if !self.seen.lock().await.insert(event.id) {
            return false;
        }
        match &self.store {
            Some(store) => store
                .mark_seen(&event.id.to_hex(), event.created_at.as_secs())
                .unwrap_or(true),
            None => true,
        }
    }

    async fn handle_event(&self, event: Event, relay: RelayUrl) {
        let now = Timestamp::now().as_secs();
        if event.created_at.as_secs().abs_diff(now) > MAX_CLOCK_SKEW_SECS {
            tracing::debug!(id = %event.id, "dropping stale request");
            return;
        }
        // Route before doing anything expensive: it must be for one of ours.
        let mut conn = None;
        for pk in event.tags.public_keys() {
            if let Some(c) = self.conns.get_by_transport(&pk).await {
                conn = Some(c);
                break;
            }
        }
        let Some(conn) = conn else { return };
        // Only the connected app may talk to a claimed connection; anyone
        // else is dropped without a reply (no probing, no reply spam).
        let client = event.pubkey;
        if conn.client.is_some_and(|c| c != client) {
            return;
        }
        if event.verify().is_err() {
            return;
        }

        let transport = Transport::detect(&event.content);
        let secret = conn.transport.secret_key();
        let plaintext = match transport {
            Transport::Nip44 => nip44::decrypt(secret, &event.pubkey, &event.content).ok(),
            Transport::Nip04 => nip04::decrypt(secret, &event.pubkey, &event.content).ok(),
        };
        let Some(req) = plaintext.and_then(|p| serde_json::from_str::<Request>(&p).ok()) else {
            tracing::debug!(id = %event.id, "undecryptable or malformed request");
            return;
        };
        // An unclaimed bunker link only understands `connect`.
        if conn.client.is_none() && req.method != Method::Connect {
            return;
        }
        if !self.first_time(&event).await {
            return;
        }

        let res = match self.process(&conn, &client, &req).await {
            Ok(result) => Response::ok(&req.id, result),
            Err(error) => Response::err(&req.id, error),
        };
        // Reply on the connection's relays (and the one it came in on, if
        // that is one of them); never somewhere a stranger chose.
        let via = conn.relays.contains(&relay).then_some(relay);
        self.reply(&conn, &client, transport, &res, via).await;

        if req.method == Method::Logout && res.error.is_none() {
            self.conns.remove(&conn.id).await;
            let _ = self.resubscribe().await;
            self.emit(SignerEvent::Disconnected {
                connection_id: conn.id.clone(),
            });
        }
    }

    async fn process(&self, conn: &Connection, client: &PublicKey, req: &Request) -> Reply {
        if req.method == Method::Connect {
            return self.connect(conn, client, req).await;
        }
        if conn.client.as_ref() != Some(client) {
            return Err("unauthorized: connect first".into());
        }
        let conn = self
            .conns
            .update(&conn.id, |c| c.last_used = Some(Timestamp::now()))
            .await
            .ok_or("connection removed")?;

        match &req.method {
            Method::Ping => Ok("pong".into()),
            Method::SwitchRelays => {
                let relays: Vec<String> = conn.relays.iter().map(|r| r.to_string()).collect();
                Ok(serde_json::to_string(&relays).unwrap_or_else(|_| "null".into()))
            }
            Method::Logout => Ok("ack".into()),
            Method::GetPublicKey
            | Method::SignEvent
            | Method::Nip04Encrypt
            | Method::Nip04Decrypt
            | Method::Nip44Encrypt
            | Method::Nip44Decrypt => self.guarded(&conn, req).await,
            Method::Connect => unreachable!(),
            Method::Other(m) => Err(format!("unsupported method: {m}")),
        }
    }

    async fn connect(&self, conn: &Connection, client: &PublicKey, req: &Request) -> Reply {
        if conn.client.as_ref() == Some(client) {
            return Ok("ack".into());
        }
        let given = req.param(1).unwrap_or_default().to_string();
        let meta: ClientMetadata = req
            .param(3)
            .and_then(|m| serde_json::from_str(m).ok())
            .unwrap_or_default();
        let perms = parse_perms(req.param(2).unwrap_or_default());
        let now = Timestamp::now();
        // Check and claim in one step, so two clients racing with the same
        // secret can't both get in.
        let mut refused = None;
        let updated = self
            .conns
            .update(&conn.id, |c| {
                if c.client.is_some() {
                    refused = Some("already connected");
                    return;
                }
                if c.expires_unused_at.is_some_and(|t| t <= now) {
                    refused = Some("this login link has expired");
                    return;
                }
                if !c.secret_matches(&given) {
                    refused = Some("invalid secret");
                    return;
                }
                c.client = Some(*client);
                c.secret_hash = None;
                c.expires_unused_at = None;
                c.last_used = Some(now);
                c.requested_perms = perms;
                if c.name.is_none() {
                    c.name = meta.name.map(|n| clean_label(&n)).filter(|s| !s.is_empty());
                }
                c.url = c
                    .url
                    .take()
                    .or(meta.url.filter(|u| u.starts_with("https://")));
                c.image = c
                    .image
                    .take()
                    .or(meta.image.filter(|u| u.starts_with("https://")));
            })
            .await
            .ok_or("connection removed")?;
        if let Some(why) = refused {
            return Err(why.into());
        }
        self.emit(SignerEvent::Connected {
            connection: updated.info(),
        });
        Ok("ack".into())
    }

    /// Methods that use the account key: need an unlocked vault and approval.
    async fn guarded(&self, conn: &Connection, req: &Request) -> Reply {
        let op = match req.method {
            Method::SignEvent => {
                let t: EventTemplate = req
                    .param(0)
                    .and_then(|p| serde_json::from_str(p).ok())
                    .ok_or("invalid event template")?;
                let tags = t
                    .tags
                    .into_iter()
                    .map(Tag::parse)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("invalid tag: {e}"))?;
                let created_at = t
                    .created_at
                    .map(Timestamp::from)
                    .unwrap_or_else(Timestamp::now);
                Op::SignEvent(UnsignedEvent::new(
                    conn.account,
                    created_at,
                    Kind::from(t.kind),
                    tags,
                    t.content,
                ))
            }
            Method::GetPublicKey => Op::GetPublicKey,
            _ => {
                let pk = req
                    .param(0)
                    .and_then(|p| PublicKey::parse(p).ok())
                    .ok_or("invalid public key")?;
                Op::Cipher {
                    method: req.method.clone(),
                    with: pk,
                    text: req.param(1).ok_or("missing text")?.to_string(),
                }
            }
        };
        match self
            .authorize(Requester::nip46(conn), op, WhenLocked::WaitAndNag)
            .await?
        {
            Outcome::PublicKey(pk) => Ok(pk.to_hex()),
            Outcome::Event(e) => Ok(e.as_json()),
            Outcome::Text(t) => Ok(t),
        }
    }

    /// The shared path for anything that uses the account key: wait for an
    /// unlock (or not), ask the approver, do the work, log it. Every request
    /// ends in [`Inner::finish`] except a fail-fast refusal while locked.
    pub(crate) async fn authorize(
        &self,
        who: Requester,
        op: Op,
        when_locked: WhenLocked,
    ) -> Result<Outcome, String> {
        let mut approval = ApprovalRequest {
            connection_id: who.connection_id,
            app_name: who.app_name,
            app_url: who.app_url,
            app_image: who.app_image,
            account: who.account,
            policy: who.policy,
            method: op.method(),
            kind: None,
            event: None,
            counterparty: None,
            payload_len: None,
        };
        match &op {
            Op::SignEvent(ev) => {
                approval.kind = Some(ev.kind.as_u16());
                approval.event = Some(ev.clone());
            }
            Op::Cipher { with, text, .. } => {
                approval.counterparty = Some(*with);
                approval.payload_len = Some(text.len());
            }
            Op::GetPublicKey => {}
        }

        let timeout = self.settings.pending_timeout;
        if !self.vault.is_unlocked() {
            match when_locked {
                WhenLocked::FailFast => return Err("Opal is locked".into()),
                WhenLocked::WaitAndNag => {
                    if self.unlock_nag_due(&approval.connection_id).await {
                        self.emit(SignerEvent::UnlockNeeded {
                            connection_id: approval.connection_id.clone(),
                            app_name: approval.app_name.clone(),
                            method: approval.method.clone(),
                        });
                    }
                    if self.vault.wait_unlocked(timeout).await.is_err() {
                        return self.finish(
                            &approval,
                            Source::Locked,
                            Err("signer is locked".into()),
                        );
                    }
                }
            }
        }

        let decision = tokio::time::timeout(timeout, self.approver.decide(&approval))
            .await
            .unwrap_or_else(|_| {
                Decision::Deny(Source::Timeout, "timed out waiting for approval".into())
            });
        let source = match decision {
            Decision::Allow(s) => s,
            Decision::Deny(s, reason) => return self.finish(&approval, s, Err(reason)),
        };

        let keys = match self.vault.keys(&approval.account).await {
            Ok(k) => k,
            Err(_) => {
                return self.finish(&approval, Source::Error, Err("signer unavailable".into()));
            }
        };
        let sk = keys.secret_key();
        let result: Result<Outcome, String> = match op {
            Op::GetPublicKey => Ok(Outcome::PublicKey(approval.account)),
            Op::SignEvent(unsigned) => keys
                .sign_event(unsigned)
                .map(Outcome::Event)
                .map_err(|e| e.to_string()),
            Op::Cipher { method, with, text } => match method {
                Method::Nip04Encrypt => nip04::encrypt(sk, &with, &text).map_err(|e| e.to_string()),
                Method::Nip04Decrypt => nip04::decrypt(sk, &with, &text).map_err(|e| e.to_string()),
                Method::Nip44Encrypt => {
                    nip44::encrypt(sk, &with, &text, nip44::Version::V2).map_err(|e| e.to_string())
                }
                Method::Nip44Decrypt => nip44::decrypt(sk, &with, &text).map_err(|e| e.to_string()),
                m => Err(format!("unsupported method: {m}")),
            }
            .map(Outcome::Text),
        };
        let source = if result.is_ok() {
            source
        } else {
            Source::Error
        };
        self.finish(&approval, source, result)
    }

    pub(crate) fn finish<T>(
        &self,
        approval: &ApprovalRequest,
        source: Source,
        result: Result<T, String>,
    ) -> Result<T, String> {
        let at = Timestamp::now().as_secs();
        let reason = result.as_ref().err().cloned();
        if self.settings.log_activity
            && let Some(store) = &self.store
        {
            let entry = ActivityEntry {
                id: 0,
                at,
                app_id: approval.connection_id.clone(),
                app_name: approval.app_name.clone(),
                account: approval.account.to_hex(),
                method: approval.method.clone(),
                kind: approval.kind,
                kind_label: None,
                allowed: result.is_ok(),
                source,
                reason: reason.clone(),
            };
            if let Err(e) = store.log_activity(&entry) {
                tracing::warn!("activity log failed: {e}");
            }
        }
        self.emit(SignerEvent::Request {
            connection_id: approval.connection_id.clone(),
            app_name: approval.app_name.clone(),
            method: approval.method.clone(),
            kind: approval.kind,
            allowed: result.is_ok(),
            source,
            reason,
            at,
        });
        result
    }

    async fn reply(
        &self,
        conn: &Connection,
        client: &PublicKey,
        transport: Transport,
        res: &Response,
        via: Option<RelayUrl>,
    ) {
        let json = serde_json::to_string(res).expect("response serializes");
        let secret = conn.transport.secret_key();
        let content = match transport {
            Transport::Nip44 => nip44::encrypt(secret, client, &json, nip44::Version::V2),
            Transport::Nip04 => nip04::encrypt(secret, client, &json),
        };
        let content = match content {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("encrypting reply failed: {e}");
                return;
            }
        };
        let event = match EventBuilder::new(Kind::NostrConnect, content)
            .tag(Tag::public_key(*client))
            .finalize(&conn.transport)
        {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("signing reply failed: {e}");
                return;
            }
        };
        let mut relays = conn.relays.clone();
        if let Some(r) = via
            && !relays.contains(&r)
        {
            relays.push(r);
        }
        if let Err(e) = self.client.send_event(&event).to(relays).await {
            tracing::warn!("sending reply failed: {e}");
        }
    }
}

/// App names come from the app itself: keep them short, single-line, and
/// unable to pass as command-line options.
pub fn clean_label(s: &str) -> String {
    let one_line: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    one_line
        .trim_start_matches('-')
        .trim()
        .chars()
        .take(60)
        .collect()
}

/// `bytes` random bytes as lowercase hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(&mut buf[..]);
    hex::encode(buf)
}
