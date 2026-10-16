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
    ///
    /// A remembered answer is the app's answer for this method and kind, not
    /// just for this one request, so it also answers that app's other waiting
    /// prompts of the same shape. They are one decision: the rule it saves
    /// already covers them, and a client that connects to several relays at
    /// once (or reconnects and re-challenges every relay) otherwise arrives as
    /// a burst of prompts for the same thing.
    pub fn answer(&self, id: &str, answer: PromptAnswer) -> bool {
        let (tx, closed) = {
            let mut pending = lock(&self.pending);
            let Some((prompt, tx)) = pending.remove(id) else {
                return false;
            };
            let mut closed = vec![id.to_string()];
            if answer.remember != Remember::Once {
                let same: Vec<String> = pending
                    .iter()
                    .filter(|(_, (p, _))| {
                        p.request.connection_id == prompt.request.connection_id
                            && p.request.method == prompt.request.method
                            && p.request.kind == prompt.request.kind
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for other in same {
                    if let Some((_, tx)) = pending.remove(&other) {
                        let _ = tx.send(answer);
                        closed.push(other);
                    }
                }
            }
            (tx, closed)
        };
        for id in closed {
            let _ = self.events.send(PromptEvent::Closed { id });
        }
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

    /// Dismiss every prompt one app has waiting (it was revoked).
    pub fn dismiss_for(&self, connection_id: &str) -> usize {
        self.dismiss_where(|p| p.request.connection_id == connection_id)
    }

    /// Dismiss every prompt (the signer stopped answering).
    pub fn dismiss_all(&self) -> usize {
        self.dismiss_where(|_| true)
    }

    fn dismiss_where(&self, f: impl Fn(&Prompt) -> bool) -> usize {
        let ids: Vec<String> = lock(&self.pending)
            .values()
            .filter(|(p, _)| f(p))
            .map(|(p, _)| p.id.clone())
            .collect();
        ids.iter().filter(|id| self.dismiss(id)).count()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use nostr_sdk::prelude::Keys;
    use opal_core::config::Policy;

    use crate::protocol::Method;

    fn request(app: &str, kind: u16) -> ApprovalRequest {
        ApprovalRequest {
            connection_id: app.into(),
            app_name: "Peridot".into(),
            app_url: None,
            app_image: None,
            account: Keys::generate().public_key(),
            policy: Policy::Basic,
            method: Method::SignEvent,
            kind: Some(kind),
            event: None,
            counterparty: None,
            payload_len: None,
        }
    }

    /// The asks run in their own tasks, so wait for them to be waiting.
    async fn wait_for_pending(hub: &PromptHub, n: usize) {
        for _ in 0..200 {
            if hub.pending().len() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("{} prompts waiting, wanted {n}", hub.pending().len());
    }

    fn ask(
        hub: &Arc<PromptHub>,
        app: &str,
        kind: u16,
    ) -> tokio::task::JoinHandle<Option<PromptAnswer>> {
        let hub = hub.clone();
        let app = app.to_string();
        tokio::spawn(async move { hub.ask(request(&app, kind)).await })
    }

    const REMEMBERED: PromptAnswer = PromptAnswer {
        allow: true,
        remember: Remember::OneHour,
    };

    /// A queued prompt nobody answers would hang the test instead of failing.
    async fn answered(t: tokio::task::JoinHandle<Option<PromptAnswer>>) -> PromptAnswer {
        tokio::time::timeout(Duration::from_secs(2), t)
            .await
            .expect("answered, not asked again")
            .unwrap()
            .expect("an answer came back")
    }

    #[tokio::test]
    async fn a_remembered_answer_answers_the_apps_queued_prompts() {
        let hub = Arc::new(PromptHub::default());
        let relay_a = ask(&hub, "peridot", 22242);
        let relay_b = ask(&hub, "peridot", 22242);
        let other_app = ask(&hub, "anagram", 22242);
        let blossom = ask(&hub, "peridot", 24242);
        wait_for_pending(&hub, 4).await;

        let id = hub
            .pending()
            .iter()
            .find(|p| p.request.kind == Some(22242) && p.request.connection_id == "peridot")
            .expect("a relay login")
            .id
            .clone();
        assert!(hub.answer(&id, REMEMBERED));

        // The same app's other relay logins are the same answer.
        for t in [relay_a, relay_b] {
            let answer = answered(t).await;
            assert!(answer.allow);
            assert_eq!(answer.remember, Remember::OneHour);
        }
        // Another kind, and another app, are different decisions.
        assert_eq!(hub.pending().len(), 2);
        assert!(!other_app.is_finished());
        assert!(!blossom.is_finished());
        assert!(
            hub.pending()
                .iter()
                .any(|p| p.request.connection_id == "anagram")
        );
        assert!(hub.pending().iter().any(|p| p.request.kind == Some(24242)));
    }

    #[tokio::test]
    async fn once_answers_only_that_request() {
        let hub = Arc::new(PromptHub::default());
        let first = ask(&hub, "peridot", 22242);
        let second = ask(&hub, "peridot", 22242);
        wait_for_pending(&hub, 2).await;

        let id = hub.pending()[0].id.clone();
        assert!(hub.answer(
            &id,
            PromptAnswer {
                allow: true,
                remember: Remember::Once,
            }
        ));
        assert!(first.await.unwrap().is_some());
        assert!(!second.is_finished(), "the other one still asks");
        assert_eq!(hub.pending().len(), 1);
    }
}
