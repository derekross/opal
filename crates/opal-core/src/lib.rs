//! Shared building blocks for Opal: configuration, key storage and the
//! unlockable vault that holds account keys in memory.

pub mod config;
pub mod error;
pub mod import;
pub mod keystore;
pub mod paths;
pub mod vault;

pub use error::{Error, Result};
pub use nostr;
