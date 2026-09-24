use std::path::PathBuf;

const APP: &str = "opal";

fn join(base: Option<PathBuf>, fallback: &str) -> PathBuf {
    base.unwrap_or_else(|| PathBuf::from(fallback)).join(APP)
}

pub fn config_dir() -> PathBuf {
    join(dirs::config_dir(), "/tmp")
}

pub fn data_dir() -> PathBuf {
    join(dirs::data_dir(), "/tmp")
}

pub fn cache_dir() -> PathBuf {
    join(dirs::cache_dir(), "/tmp")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("opal.sock")
}
