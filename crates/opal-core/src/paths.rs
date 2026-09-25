use std::path::PathBuf;

/// Where an app built on these crates keeps its files. Opal and Peridot each
/// get their own config, data, cache and socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppDirs {
    pub name: &'static str,
}

impl AppDirs {
    pub const OPAL: AppDirs = AppDirs { name: "opal" };
    pub const PERIDOT: AppDirs = AppDirs { name: "peridot" };

    /// Never fall back to a shared directory like /tmp: without a home the
    /// daemon refuses to start rather than put keys-adjacent data there.
    fn join(self, base: Option<PathBuf>, what: &str) -> PathBuf {
        base.unwrap_or_else(|| panic!("no {what} directory (is HOME set?)"))
            .join(self.name)
    }

    pub fn config_dir(self) -> PathBuf {
        self.join(dirs::config_dir(), "config")
    }

    pub fn data_dir(self) -> PathBuf {
        self.join(dirs::data_dir(), "data")
    }

    pub fn cache_dir(self) -> PathBuf {
        self.join(dirs::cache_dir(), "cache")
    }

    pub fn config_file(self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    pub fn db_file(self) -> PathBuf {
        self.data_dir().join(format!("{}.db", self.name))
    }

    pub fn socket_path(self) -> PathBuf {
        dirs::runtime_dir()
            .expect("XDG_RUNTIME_DIR is not set")
            .join(format!("{}.sock", self.name))
    }
}

pub fn config_dir() -> PathBuf {
    AppDirs::OPAL.config_dir()
}

pub fn data_dir() -> PathBuf {
    AppDirs::OPAL.data_dir()
}

pub fn cache_dir() -> PathBuf {
    AppDirs::OPAL.cache_dir()
}

pub fn config_file() -> PathBuf {
    AppDirs::OPAL.config_file()
}

pub fn socket_path() -> PathBuf {
    AppDirs::OPAL.socket_path()
}
