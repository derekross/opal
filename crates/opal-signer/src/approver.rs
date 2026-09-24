//! Deciding whether a request may go ahead.

use futures::future::BoxFuture;
use nostr_sdk::prelude::{PublicKey, UnsignedEvent};
use serde::Serialize;

use crate::protocol::Method;

/// Everything the user (or a rule) needs to decide on a request.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub connection_id: String,
    pub app_name: String,
    pub account: PublicKey,
    pub method: Method,
    /// Kind of the event to sign, for `sign_event`.
    pub kind: Option<u16>,
    /// The event to sign, for `sign_event`.
    pub event: Option<UnsignedEvent>,
    /// The other party, for encrypt/decrypt.
    pub counterparty: Option<PublicKey>,
    /// Length of the text to encrypt/decrypt; the text itself is not shared.
    pub payload_len: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
}

pub trait Approver: Send + Sync {
    fn decide<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, Decision>;
}

/// Approves everything. Only for tests and `--auto-approve` debugging.
pub struct AllowAll;

impl Approver for AllowAll {
    fn decide<'a>(&'a self, _req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async { Decision::Allow })
    }
}

/// Rejects everything.
pub struct DenyAll;

impl Approver for DenyAll {
    fn decide<'a>(&'a self, _req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async { Decision::Deny("user rejected".into()) })
    }
}
