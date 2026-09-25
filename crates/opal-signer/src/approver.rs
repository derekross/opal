//! Deciding whether a request may go ahead.

use std::sync::Arc;

use futures::future::BoxFuture;
use nostr_sdk::prelude::{PublicKey, Timestamp, UnsignedEvent};
use opal_core::config::Policy;
use serde::Serialize;

use crate::permissions::{Evaluation, Rule, Source, evaluate, is_sensitive};
use crate::prompts::PromptHub;
use crate::protocol::Method;
use crate::store::SignerStore;

/// Prompts one app may have waiting at once; more are denied.
const MAX_PENDING_PER_APP: usize = 3;

/// Everything the user (or a rule) needs to decide on a request.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalRequest {
    pub connection_id: String,
    pub app_name: String,
    pub app_url: Option<String>,
    pub app_image: Option<String>,
    pub account: PublicKey,
    pub policy: Policy,
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
    Allow(Source),
    Deny(Source, String),
}

pub trait Approver: Send + Sync {
    fn decide<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, Decision>;
}

/// Approves everything. Only for tests and debugging.
pub struct AllowAll;

impl Approver for AllowAll {
    fn decide<'a>(&'a self, _req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async { Decision::Allow(Source::Automatic) })
    }
}

/// Rejects everything.
pub struct DenyAll;

impl Approver for DenyAll {
    fn decide<'a>(&'a self, _req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async { Decision::Deny(Source::Automatic, "user rejected".into()) })
    }
}

/// The real approver: saved rules and the app's policy first, then a prompt.
/// Answers the user asks to remember become rules.
pub struct PolicyApprover {
    store: SignerStore,
    prompts: Arc<PromptHub>,
}

impl PolicyApprover {
    pub fn new(store: SignerStore, prompts: Arc<PromptHub>) -> Self {
        Self { store, prompts }
    }
}

impl Approver for PolicyApprover {
    fn decide<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        Box::pin(async move {
            let now = Timestamp::now().as_secs();
            let rules = match self.store.rules(&req.connection_id) {
                Ok(r) => r,
                Err(e) => return Decision::Deny(Source::Error, e.to_string()),
            };
            let created_at = req.event.as_ref().map(|e| e.created_at.as_secs());
            match evaluate(req.policy, &rules, &req.method, req.kind, created_at, now) {
                Evaluation::Allow(s) => return Decision::Allow(s),
                Evaluation::Deny(s) => return Decision::Deny(s, "denied by a saved rule".into()),
                Evaluation::Ask => {}
            }
            // One app can't queue up a wall of prompts.
            if self.prompts.pending_for(&req.connection_id) >= MAX_PENDING_PER_APP {
                return Decision::Deny(Source::Automatic, "too many pending requests".into());
            }
            let Some(answer) = self.prompts.ask(req.clone()).await else {
                return Decision::Deny(Source::Timeout, "no answer".into());
            };
            if let Some(mut until) = answer.remember.until(now) {
                // Sensitive kinds are never remembered for more than an hour.
                if is_sensitive(&req.method, req.kind) {
                    until = Some(until.map_or(now + 3600, |t| t.min(now + 3600)));
                }
                let rule = Rule {
                    app_id: req.connection_id.clone(),
                    method: req.method.clone(),
                    kind: req.kind,
                    allow: answer.allow,
                    until,
                    created_at: now,
                };
                if let Err(e) = self.store.put_rule(&rule) {
                    tracing::warn!("saving rule failed: {e}");
                }
            }
            if answer.allow {
                Decision::Allow(Source::User)
            } else {
                Decision::Deny(Source::User, "user rejected".into())
            }
        })
    }
}
