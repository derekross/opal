use std::path::PathBuf;

const APP: &str = "opal";

/// Never fall back to a shared directory like /tmp: without a home the
/// daemon refuses to start rather than put keys-adjacent data there.
fn join(base: Option<PathBuf>, what: &str) -> PathBuf {
    base.unwrap_or_else(|| panic!("no {what} directory (is HOME set?)"))
        .join(APP)
}

pub fn config_dir() -> PathBuf {
    join(dirs::config_dir(), "config")
}

pub fn data_dir() -> PathBuf {
    join(dirs::data_dir(), "data")
}

pub fn cache_dir() -> PathBuf {
    join(dirs::cache_dir(), "cache")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .expect("XDG_RUNTIME_DIR is not set")
        .join("opal.sock")
}
