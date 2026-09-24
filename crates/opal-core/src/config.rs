use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, paths};

/// Relays the signer listens on when an app does not bring its own.
pub const DEFAULT_SIGNER_RELAYS: &[&str] = &[
    "wss://relay.nsec.app",
    "wss://relay.nip46.com",
    "wss://bucket.coracle.social",
    "wss://nrs.primal.net",
];

/// Relays used to look up profiles (kind 0) and relay lists (kind 10002).
pub const DEFAULT_PROFILE_RELAYS: &[&str] = &[
    "wss://purplepag.es",
    "wss://user.kindpag.es",
    "wss://profiles.nostr1.com",
    "wss://indexer.coracle.social",
];

/// Well-connected relays for discovering relay lists and profiles.
pub const DEFAULT_BOOTSTRAP_RELAYS: &[&str] = &[
    "wss://purplepag.es",
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub identity: Identity,
    pub modules: Modules,
    pub signer: SignerConfig,
    pub notifications: NotificationsConfig,
    pub status: StatusConfig,
}

/// Where the rest of Opal gets its identity from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityMode {
    /// Only an npub is known; nothing can be signed.
    ReadOnly,
    /// Keys live in the local vault (the signer module).
    #[default]
    Local,
    /// Signing is delegated to an external NIP-46 bunker.
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Identity {
    pub mode: IdentityMode,
    /// npub used in read-only mode.
    pub npub: Option<String>,
    /// The NIP-05 address that npub was looked up from, for display.
    pub nip05: Option<String>,
    /// bunker:// URI used in external mode.
    pub bunker_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Modules {
    pub signer: bool,
    pub notifications: bool,
    pub status: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            signer: true,
            notifications: false,
            status: false,
        }
    }
}

/// How much a newly connected app may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    /// Common, low-risk actions are approved automatically.
    #[default]
    Basic,
    /// Every permission is asked for.
    Manual,
    /// Everything is approved.
    FullTrust,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SignerConfig {
    pub relays: Vec<String>,
    pub profile_relays: Vec<String>,
    /// Minutes of inactivity before the vault locks itself; `None` = never.
    pub auto_lock_minutes: Option<u32>,
    pub lock_on_screen_lock: bool,
    pub default_policy: Policy,
    /// Seconds a request waits for an unlock or an approval before failing.
    pub pending_timeout_secs: u64,
    pub privacy_mode: bool,
}

impl Default for SignerConfig {
    fn default() -> Self {
        Self {
            relays: DEFAULT_SIGNER_RELAYS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            profile_relays: DEFAULT_PROFILE_RELAYS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            auto_lock_minutes: Some(60),
            lock_on_screen_lock: true,
            default_policy: Policy::Basic,
            pending_timeout_secs: 120,
            privacy_mode: false,
        }
    }
}

/// Which kinds of notification to show.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationTypes {
    pub replies: bool,
    pub mentions: bool,
    pub reposts: bool,
    pub reactions: bool,
    pub zaps: bool,
    /// NIP-17 direct messages (need the signer, unlocked).
    pub dms: bool,
}

impl Default for NotificationTypes {
    fn default() -> Self {
        Self {
            replies: true,
            mentions: true,
            reposts: true,
            reactions: true,
            zaps: true,
            dms: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    pub types: NotificationTypes,
    /// Web client that opens a notification: primal, jumble, coracle, nostrudel, ditto.
    pub client: String,
    /// Pop desktop notifications for new events (the panel always lists them).
    pub desktop: bool,
    /// Show message text in desktop notifications for DMs.
    pub dm_previews: bool,
    /// Zaps below this many sats don't pop a desktop notification.
    pub min_zap_sats: u64,
    /// Extra hex pubkeys to hide, on top of your NIP-51 mute list.
    pub blocked: Vec<String>,
    /// Relays used to find your relay list and profiles.
    pub bootstrap_relays: Vec<String>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            types: NotificationTypes::default(),
            client: "primal".into(),
            desktop: true,
            dm_previews: false,
            min_zap_sats: 0,
            blocked: Vec::new(),
            bootstrap_relays: DEFAULT_BOOTSTRAP_RELAYS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// NIP-38 statuses and scrobbling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusConfig {
    /// Publish what's playing (d=music).
    pub music: bool,
    /// Link on music statuses: auto (the player's own link, else a search),
    /// youtube-music, spotify, or none.
    pub music_link: String,
    /// Only these players (MPRIS base names like "spotify", "chromium");
    /// empty = all.
    pub players: Vec<String>,
    /// Never these players.
    pub players_blocked: Vec<String>,
    /// Keep a local history of what you listened to.
    pub scrobble: bool,
    /// Also publish each play as a kind 1073 scrobble event.
    pub publish_scrobbles: bool,
    /// "In a meeting" while a khal event is running.
    pub auto_calendar: bool,
    /// Show the event title instead of just "In a meeting".
    pub calendar_titles: bool,
    pub calendar_text: String,
    /// "Away" while the screen is locked.
    pub auto_away: bool,
    pub away_text: String,
    /// "Focusing" while notifications are silenced (Do Not Disturb).
    pub auto_focus: bool,
    pub focus_text: String,
    /// Extra relays to publish to, on top of your NIP-65 write relays.
    pub relays: Vec<String>,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            music: true,
            music_link: "auto".into(),
            players: Vec::new(),
            players_blocked: Vec::new(),
            scrobble: true,
            publish_scrobbles: false,
            auto_calendar: false,
            calendar_titles: false,
            calendar_text: "In a meeting".into(),
            auto_away: false,
            away_text: "Away".into(),
            auto_focus: false,
            focus_text: "Focusing".into(),
            relays: Vec::new(),
        }
    }
}

/// Where statuses go when you have no NIP-65 relay list yet.
pub const DEFAULT_PUBLISH_RELAYS: &[&str] = &[
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
    "wss://nos.lol",
    "wss://relay.damus.io",
];

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from(&paths::config_file())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| Error::Config(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&paths::config_file())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[modules]\nstatus = true\n").unwrap();
        assert!(cfg.modules.signer);
        assert!(cfg.modules.status);
        assert_eq!(cfg.signer.relays.len(), DEFAULT_SIGNER_RELAYS.len());
    }

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("opal-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.identity.mode = IdentityMode::External;
        cfg.save_to(&path).unwrap();
        let back = Config::load_from(&path).unwrap();
        assert_eq!(back.identity.mode, IdentityMode::External);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
