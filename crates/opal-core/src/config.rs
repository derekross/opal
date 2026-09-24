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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub identity: Identity,
    pub modules: Modules,
    pub signer: SignerConfig,
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
