//! Shared building blocks for Opal: configuration, key storage and the
//! unlockable vault that holds account keys in memory.

pub mod accounts;
pub mod config;
pub mod db;
pub mod error;
pub mod identity;
pub mod import;
pub mod ipc;
pub mod keystore;
pub mod paths;
pub mod vault;

pub use error::{Error, Result};
pub use nostr;
