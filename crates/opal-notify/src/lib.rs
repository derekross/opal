//! Nostr notifications: replies, mentions, reposts, reactions, zaps and
//! NIP-17 direct messages for your account, kept in the Opal database.

pub mod classify;
pub mod engine;
pub mod links;
pub mod store;

pub use classify::{NotifType, Notification, classify};
pub use engine::{NotifyEngine, NotifyEvent, NotifyParams};
pub use store::{NotifyStore, StoredNotification};
