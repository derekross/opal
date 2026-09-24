//! NIP-46 remote signer ("bunker").

pub mod approver;
pub mod connection;
pub mod kinds;
pub mod permissions;
pub mod perms;
pub mod prompts;
pub mod protocol;
pub mod server;
pub mod store;
pub mod uri;

pub use approver::{AllowAll, ApprovalRequest, Approver, Decision, DenyAll, PolicyApprover};
pub use prompts::{Prompt, PromptAnswer, PromptEvent, PromptHub};
pub use server::{Signer, SignerError, SignerEvent, SignerSettings};
pub use store::SignerStore;
pub use uri::NostrConnectUri;
