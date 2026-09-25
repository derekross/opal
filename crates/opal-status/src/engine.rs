//! Runs the status module: watches players and automatic sources, decides
//! what to publish, signs through a [`StatusSigner`] and sends to your
//! write relays. While the signer is locked the latest status per `d` tag
//! (and any scrobbles) wait and go out on unlock.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nostr_sdk::prelude::*;
use opal_core::config::{DEFAULT_PUBLISH_RELAYS, StatusConfig};
use serde::Serialize;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::events;
use crate::general::{self, AutoInputs, AutoSettings, Desired, Manual};
use crate::mpris::{self, Track};
use crate::music::{Action, MusicTracker, PlayerFilter, link_for};
use crate::store::{Play, StatusStore};

const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// Automatic statuses expire after this and are refreshed while they hold.
const AUTO_TTL: u64 = 30 * 60;
const AUTO_REFRESH_BEFORE: u64 = 10 * 60;
const MAX_PENDING_SCROBBLES: usize = 200;

/// Signs events as the user.
pub use opal_kit::signer::{EventSigner as StatusSigner, SignError};

pub struct StatusParams {
    pub account: PublicKey,
    pub config: StatusConfig,
    pub store: StatusStore,
    pub signer: Arc<dyn StatusSigner>,
    /// Relays for finding the user's relay list.
    pub bootstrap_relays: Vec<String>,
    /// Tells the engine when signing may work again (vault unlocked).
    pub signer_ready: Option<watch::Receiver<bool>>,
}

pub enum StatusCommand {
    SetManual(Option<Manual>),
    /// Clear statuses and stop.
    Stop(oneshot::Sender<()>),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Snapshot {
    pub now_playing: Option<Track>,
    /// What the music status currently says (None = nothing published).
    pub music: Option<String>,
    pub general: Option<Desired>,
    pub relays: Vec<String>,
    /// `d` tags / scrobbles waiting for the signer.
    pub waiting: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatusEvent {
    Changed { snapshot: Snapshot },
    Scrobbled { play: Play },
}

pub struct StatusEngine {
    task: JoinHandle<()>,
    commands: mpsc::Sender<StatusCommand>,
    events: broadcast::Sender<StatusEvent>,
    snapshot: Arc<RwLock<Snapshot>>,
    account: PublicKey,
}

impl StatusEngine {
    pub fn start(p: StatusParams) -> Self {
        let (commands, rx) = mpsc::channel(16);
        let (events, _) = broadcast::channel(64);
        let snapshot = Arc::new(RwLock::new(Snapshot::default()));
        let account = p.account;
        let runner = Runner {
            account_hex: p.account.to_hex(),
            me: p.account,
            client: Client::default(),
            cfg: p.config,
            store: p.store,
            signer: p.signer,
            bootstrap: p.bootstrap_relays,
            relays: Vec::new(),
            events: events.clone(),
            snapshot: snapshot.clone(),
            pending_status: HashMap::new(),
            pending_scrobbles: Vec::new(),
            published_general: None,
        };
        let task = tokio::spawn(runner.run(rx, p.signer_ready));
        Self {
            task,
            commands,
            events,
            snapshot,
            account,
        }
    }

    pub fn account(&self) -> PublicKey {
        self.account
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StatusEvent> {
        self.events.subscribe()
    }

    pub async fn snapshot(&self) -> Snapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn set_manual(&self, m: Option<Manual>) {
        let _ = self.commands.send(StatusCommand::SetManual(m)).await;
    }

    /// Clear published statuses (best effort, a few seconds) and stop.
    pub async fn stop(self) {
        let (tx, rx) = oneshot::channel();
        if self.commands.send(StatusCommand::Stop(tx)).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(6), rx).await;
        }
        self.task.abort();
    }
}

struct Runner {
    me: PublicKey,
    account_hex: String,
    client: Client,
    cfg: StatusConfig,
    store: StatusStore,
    signer: Arc<dyn StatusSigner>,
    bootstrap: Vec<String>,
    relays: Vec<RelayUrl>,
    events: broadcast::Sender<StatusEvent>,
    snapshot: Arc<RwLock<Snapshot>>,
    /// Latest unsigned status per `d` tag, waiting for the signer.
    pending_status: HashMap<String, (EventBuilder, Option<u64>)>,
    pending_scrobbles: Vec<(i64, EventBuilder)>,
    /// (content, link, expires_at) of the general status on relays.
    published_general: Option<(String, Option<String>, u64)>,
}

impl Runner {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<StatusCommand>,
        mut ready: Option<watch::Receiver<bool>>,
    ) {
        self.setup_relays().await;

        let (tracks_tx, mut tracks_rx) = watch::channel(Vec::<Track>::new());
        let watcher = if self.cfg.music || self.cfg.scrobble {
            Some(tokio::spawn(mpris::watch_players(tracks_tx)))
        } else {
            drop(tracks_tx);
            None
        };
        let mut music = MusicTracker::new();
        let filter = PlayerFilter {
            allow: self.cfg.players.clone(),
            block: self.cfg.players_blocked.clone(),
        };
        let mut manual = self.store.manual().ok().flatten();
        let mut auto = AutoInputs::default();
        let settings = AutoSettings {
            calendar: self.cfg.auto_calendar,
            calendar_titles: self.cfg.calendar_titles,
            calendar_text: self.cfg.calendar_text.clone(),
            away: self.cfg.auto_away,
            away_text: self.cfg.away_text.clone(),
            focus: self.cfg.auto_focus,
            focus_text: self.cfg.focus_text.clone(),
        };

        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut seconds: u64 = 0;
        loop {
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    Some(StatusCommand::SetManual(m)) => {
                        let _ = self.store.set_manual(m.as_ref());
                        manual = m;
                        self.update_general(manual.as_ref(), &auto, &settings, true).await;
                    }
                    Some(StatusCommand::Stop(done)) => {
                        self.shutdown(&music).await;
                        let _ = done.send(());
                        break;
                    }
                    None => break,
                },
                changed = async {
                    match ready.as_mut() {
                        Some(rx) => rx.changed().await.is_ok(),
                        None => std::future::pending().await,
                    }
                } => {
                    if !changed { ready = None; continue; }
                    if ready.as_ref().is_some_and(|r| *r.borrow()) {
                        self.flush_pending().await;
                    }
                }
                _ = tracks_rx.changed(), if watcher.is_some() => {
                    let tracks = tracks_rx.borrow().clone();
                    self.step_music(&mut music, &tracks, &filter).await;
                }
                _ = tick.tick() => {
                    seconds += 1;
                    if watcher.is_some() {
                        let tracks = tracks_rx.borrow().clone();
                        self.step_music(&mut music, &tracks, &filter).await;
                    }
                    let mut changed = false;
                    if settings.away && seconds % 5 == 1 {
                        let away = crate::sources::screen_locked().await;
                        changed |= away != auto.away;
                        auto.away = away;
                    }
                    if settings.focus && seconds % 20 == 1 {
                        let focus = crate::sources::do_not_disturb().await;
                        changed |= focus != auto.focus;
                        auto.focus = focus;
                    }
                    if settings.calendar && seconds % 60 == 1 {
                        let meeting = crate::sources::current_meeting().await;
                        changed |= meeting != auto.meeting;
                        auto.meeting = meeting;
                    }
                    // Anything waiting for a signer that doesn't say when it's
                    // ready (an external one) is retried every minute.
                    if seconds.is_multiple_of(60)
                        && (!self.pending_status.is_empty() || !self.pending_scrobbles.is_empty())
                    {
                        self.flush_pending().await;
                    }
                    // Expired manual statuses and auto refreshes are checked every 30s.
                    if changed || seconds.is_multiple_of(30) {
                        self.update_general(manual.as_ref(), &auto, &settings, changed).await;
                    }
                }
            }
        }
        if let Some(w) = watcher {
            w.abort();
        }
        self.client.shutdown().await;
    }

    async fn setup_relays(&mut self) {
        let bootstrap = opal_kit::relays::parse_urls(&self.bootstrap);
        let mut write =
            opal_kit::relays::fetch_relay_list(&self.client, &bootstrap, self.me, FETCH_TIMEOUT)
                .await
                .map(|l| l.write)
                .unwrap_or_default();
        for r in opal_kit::relays::parse_urls(&self.cfg.relays) {
            if !write.contains(&r) {
                write.push(r);
            }
        }
        // Well-known relays only when there's nothing else to go on.
        if write.is_empty() {
            write = DEFAULT_PUBLISH_RELAYS
                .iter()
                .filter_map(|r| RelayUrl::parse(r).ok())
                .collect();
        }
        for r in &write {
            let _ = self.client.add_relay(r).await;
        }
        self.client.connect().and_wait(Duration::from_secs(5)).await;
        self.relays = write;
        self.snapshot.write().await.relays = self.relays.iter().map(|r| r.to_string()).collect();
        self.emit().await;
    }

    async fn step_music(
        &mut self,
        music: &mut MusicTracker,
        tracks: &[Track],
        filter: &PlayerFilter,
    ) {
        let now_unix = Timestamp::now().as_secs();
        let actions = music.update(
            tracks,
            filter,
            &self.cfg.music_link,
            Instant::now(),
            now_unix,
        );
        let playing = tracks
            .iter()
            .find(|t| t.playing && filter.accepts(&t.player))
            .cloned();
        let playing_changed = {
            let snap = self.snapshot.read().await;
            snap.now_playing.as_ref().map(Track::key) != playing.as_ref().map(Track::key)
        };
        if playing_changed {
            self.snapshot.write().await.now_playing = playing.clone();
        }
        let notify = playing_changed || !actions.is_empty();
        for a in actions {
            match a {
                Action::Publish {
                    content,
                    link,
                    expires_at,
                } => {
                    if !self.cfg.music {
                        continue;
                    }
                    let b = events::status("music", &content, link.as_deref(), Some(expires_at));
                    if self.publish_status("music", b, Some(expires_at)).await {
                        self.snapshot.write().await.music = Some(content);
                    } else {
                        music.forget_published();
                    }
                }
                Action::Clear => {
                    if !self.cfg.music {
                        continue;
                    }
                    let now = Timestamp::now().as_secs();
                    self.publish_status("music", events::clear("music", now), None)
                        .await;
                    self.snapshot.write().await.music = None;
                }
                Action::Scrobble { track, played_at } => self.scrobble(&track, played_at).await,
            }
        }
        if notify {
            self.emit().await;
        }
    }

    async fn scrobble(&mut self, track: &Track, played_at: u64) {
        if !self.cfg.scrobble {
            return;
        }
        let link = link_for(track, &self.cfg.music_link);
        let mut play = Play {
            id: 0,
            played_at,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration: track.length.map(|d| d.as_secs()),
            player: track.player.clone(),
            link: link.clone(),
            event_id: None,
        };
        let Ok(id) = self.store.add_play(&self.account_hex, &play) else {
            return;
        };
        play.id = id;
        if self.cfg.publish_scrobbles {
            let b = events::scrobble(track, link.as_deref(), played_at);
            match self.sign(b.clone()).await {
                Ok(ev) => {
                    if self.send(&ev).await {
                        let _ = self.store.set_event_id(id, &ev.id.to_hex());
                        play.event_id = Some(ev.id.to_hex());
                    }
                }
                Err(SignError::Unavailable(_)) => {
                    if self.pending_scrobbles.len() < MAX_PENDING_SCROBBLES {
                        self.pending_scrobbles.push((id, b));
                    }
                }
                Err(SignError::Failed(e)) => self.set_error(e).await,
            }
        }
        let _ = self.events.send(StatusEvent::Scrobbled { play });
    }

    async fn update_general(
        &mut self,
        manual: Option<&Manual>,
        auto: &AutoInputs,
        s: &AutoSettings,
        force: bool,
    ) {
        let now = Timestamp::now().as_secs();
        let desired = general::desired(manual, auto, s, now);
        match (&desired, &self.published_general) {
            (None, None) => {}
            (None, Some(_)) => {
                if self
                    .publish_status("general", events::clear("general", now), None)
                    .await
                {
                    self.published_general = None;
                }
            }
            (Some(d), current) => {
                let expires = d.expires_at.unwrap_or(now + AUTO_TTL);
                let same = current
                    .as_ref()
                    .is_some_and(|(c, l, _)| *c == d.content && *l == d.link);
                // Auto statuses carry a short expiry; refresh before it lapses.
                let due = current.as_ref().is_some_and(|(_, _, exp)| {
                    d.expires_at.is_none() && exp.saturating_sub(now) < AUTO_REFRESH_BEFORE
                });
                if !same || due || (force && !same) {
                    let b = events::status("general", &d.content, d.link.as_deref(), Some(expires));
                    if self.publish_status("general", b, Some(expires)).await {
                        self.published_general = Some((d.content.clone(), d.link.clone(), expires));
                    }
                }
            }
        }
        let changed = self.snapshot.read().await.general != desired;
        if changed {
            self.snapshot.write().await.general = desired;
            self.emit().await;
        }
    }

    /// Sign and send a status; if the signer isn't available keep the
    /// latest one for this `d` tag. Returns whether it went out now.
    async fn publish_status(&mut self, d: &str, b: EventBuilder, expires: Option<u64>) -> bool {
        match self.sign(b.clone()).await {
            Ok(ev) => {
                self.pending_status.remove(d);
                self.send(&ev).await
            }
            Err(SignError::Unavailable(_)) => {
                self.pending_status.insert(d.to_string(), (b, expires));
                self.update_waiting().await;
                // Treat as done so we don't retry every second; the pending
                // copy goes out on unlock.
                true
            }
            Err(SignError::Failed(e)) => {
                self.set_error(e).await;
                false
            }
        }
    }

    async fn flush_pending(&mut self) {
        let now = Timestamp::now().as_secs();
        let statuses: Vec<_> = self.pending_status.drain().collect();
        for (d, (b, expires)) in statuses {
            if expires.is_some_and(|e| e <= now) {
                continue; // the song is over
            }
            if let Ok(ev) = self.sign(b.clone()).await {
                self.send(&ev).await;
            } else {
                self.pending_status.insert(d, (b, expires));
            }
        }
        let scrobbles: Vec<_> = std::mem::take(&mut self.pending_scrobbles);
        for (id, b) in scrobbles {
            match self.sign(b.clone()).await {
                Ok(ev) => {
                    if self.send(&ev).await {
                        let _ = self.store.set_event_id(id, &ev.id.to_hex());
                    }
                }
                Err(_) => self.pending_scrobbles.push((id, b)),
            }
        }
        self.update_waiting().await;
        self.emit().await;
    }

    async fn shutdown(&mut self, music: &MusicTracker) {
        let now = Timestamp::now().as_secs();
        if self.cfg.music
            && music.published_key().is_some()
            && let Ok(ev) = self.sign(events::clear("music", now)).await
        {
            self.send(&ev).await;
        }
        // Automatic statuses shouldn't outlive the module; a manual one stays.
        if self.published_general.is_some()
            && self.store.manual().ok().flatten().is_none()
            && let Ok(ev) = self.sign(events::clear("general", now)).await
        {
            self.send(&ev).await;
        }
    }

    /// Sign, but never wait long: an external signer may be slow or away,
    /// and this loop also drives everything else.
    async fn sign(&self, b: EventBuilder) -> Result<Event, SignError> {
        opal_kit::signer::sign_within(
            self.signer.as_ref(),
            b.finalize_unsigned(self.me),
            opal_kit::signer::SIGN_TIMEOUT,
        )
        .await
    }

    async fn send(&self, ev: &Event) -> bool {
        match opal_kit::relays::publish(&self.client, ev, &self.relays).await {
            Ok(_) => true,
            Err(e) => {
                self.set_error(e).await;
                false
            }
        }
    }

    async fn set_error(&self, e: String) {
        tracing::warn!("status: {e}");
        self.snapshot.write().await.last_error = Some(e);
    }

    async fn update_waiting(&self) {
        self.snapshot.write().await.waiting =
            self.pending_status.len() + self.pending_scrobbles.len();
    }

    async fn emit(&self) {
        let snapshot = self.snapshot.read().await.clone();
        let _ = self.events.send(StatusEvent::Changed { snapshot });
    }
}
