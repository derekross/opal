//! Statuses (NIP-38) and scrobbles: what you're listening to, a status you
//! set, and automatic ones (calendar, away, focus).

pub mod engine;
pub mod events;
pub mod general;
pub mod mpris;
pub mod music;
pub mod sources;
pub mod store;

pub use engine::{StatusCommand, StatusEngine, StatusEvent, StatusParams, StatusSigner};
pub use store::StatusStore;
