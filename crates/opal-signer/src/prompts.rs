//! Requests waiting for the user. The UI lists them, shows each one, and
//! answers; a request that is dropped (timed out) disappears on its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nostr_sdk::prelude::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot};

use crate::approver::ApprovalRequest;
use crate::kinds;
use crate::permissions::Remember;

#[derive(Debug, Clone, Serialize)]
pub struct Prompt {
    pub id: String,
    /// Arrival order; stable even for prompts from the same second.
    pub seq: u64,
    pub created_at: u64,
    pub kind_label: Option<String>,
    /// Sensitive requests (credentials, overwrites…) can't be remembered long.
    pub sensitive: bool,
    #[serde(flatten)]
    pub request: ApprovalRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PromptAnswer {
    pub allow: bool,
    pub remember: Remember,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptEvent {
    Opened { prompt: Box<Prompt> },
    Closed { id: String },
}

type Pending = HashMap<String, (Prompt, oneshot::Sender<PromptAnswer>)>;

pub struct PromptHub {
    pending: Arc<Mutex<Pending>>,
    events: broadcast::Sender<PromptEvent>,
    next_seq: std::sync::atomic::AtomicU64,
}

impl Default for PromptHub {
    fn default() -> Self {
        Self {
            pending: Arc::default(),
            events: broadcast::channel(64).0,
            next_seq: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

/// Removes the prompt if the waiting request goes away first.
struct Guard {
    id: String,
    pending: Arc<Mutex<Pending>>,
    events: broadcast::Sender<PromptEvent>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let removed = lock(&self.pending).remove(&self.id).is_some();
        if removed {
            let _ = self.events.send(PromptEvent::Closed {
                id: self.id.clone(),
            });
        }
    }
}

fn lock(m: &Mutex<Pending>) -> std::sync::MutexGuard<'_, Pending> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl PromptHub {
    pub fn subscribe(&self) -> broadcast::Receiver<PromptEvent> {
        self.events.subscribe()
    }

    /// Wait for the user's answer. `None` if the prompt was dismissed.
    pub async fn ask(&self, request: ApprovalRequest) -> Option<PromptAnswer> {
        let id = crate::server::random_hex(8);
        let prompt = Prompt {
            id: id.clone(),
            seq: self
                .next_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            created_at: Timestamp::now().as_secs(),
            kind_label: request.kind.map(kinds::label),
            sensitive: crate::permissions::is_sensitive(&request.method, request.kind),
            request,
        };
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id.clone(), (prompt.clone(), tx));
        let _guard = Guard {
            id,
            pending: self.pending.clone(),
            events: self.events.clone(),
        };
        let _ = self.events.send(PromptEvent::Opened {
            prompt: Box::new(prompt),
        });
        rx.await.ok()
    }

    /// Answer a prompt. Returns `false` if it no longer exists.
    pub fn answer(&self, id: &str, answer: PromptAnswer) -> bool {
        let Some((_, tx)) = lock(&self.pending).remove(id) else {
            return false;
        };
        let _ = self.events.send(PromptEvent::Closed { id: id.to_string() });
        tx.send(answer).is_ok()
    }

    /// Dismiss a prompt without answering (counts as a denial).
    pub fn dismiss(&self, id: &str) -> bool {
        let removed = lock(&self.pending).remove(id).is_some();
        if removed {
            let _ = self.events.send(PromptEvent::Closed { id: id.to_string() });
        }
        removed
    }

    pub fn pending(&self) -> Vec<Prompt> {
        let mut v: Vec<Prompt> = lock(&self.pending)
            .values()
            .map(|(p, _)| p.clone())
            .collect();
        v.sort_by_key(|p| p.seq);
        v
    }

    /// How many prompts one app has waiting.
    pub fn pending_for(&self, connection_id: &str) -> usize {
        lock(&self.pending)
            .values()
            .filter(|(p, _)| p.request.connection_id == connection_id)
            .count()
    }
}
