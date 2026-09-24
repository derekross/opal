use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("the vault is locked")]
    Locked,
    #[error("wrong passphrase")]
    BadPassphrase,
    #[error("account not found")]
    AccountNotFound,
    #[error("account already exists")]
    AccountExists,
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("nostr: {0}")]
    Nostr(String),
    #[error("keyring: {0}")]
    Keyring(Box<oo7::Error>),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    Config(String),
}

impl Error {
    pub fn nostr(e: impl std::fmt::Display) -> Self {
        Self::Nostr(e.to_string())
    }
}

impl From<oo7::Error> for Error {
    fn from(e: oo7::Error) -> Self {
        Self::Keyring(Box::new(e))
    }
}
