//! Nostr notifications: replies, mentions, reposts, reactions, zaps and
//! NIP-17 direct messages for your account, kept in the Opal database.

pub mod classify;
pub mod engine;
pub mod links;
pub mod store;

pub use classify::{NotifType, Notification, classify};
pub use engine::{
    MUTE_INTERRUPTED, MUTE_NEEDS_UNLOCK, MUTE_NO_RELAY_LIST, MUTE_UNSURE, MutedEntry, NotifyEngine,
    NotifyEvent, NotifyHandle, NotifyParams,
};
pub use store::{NotifyStore, StoredNotification};
