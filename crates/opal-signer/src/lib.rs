//! NIP-46 remote signer ("bunker").

pub mod approver;
pub mod connection;
pub mod perms;
pub mod protocol;
pub mod server;
pub mod uri;

pub use approver::{AllowAll, ApprovalRequest, Approver, Decision, DenyAll};
pub use server::{Signer, SignerError, SignerEvent, SignerSettings};
pub use uri::NostrConnectUri;
