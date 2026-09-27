//! Opal on the control socket (served by opal-kit): requests go to
//! [`api::dispatch`], `subscribe` gets the full status plus live events.

use std::sync::Arc;

use futures::future::BoxFuture;
use opal_core::ipc::IpcEvent;
use opal_kit::ipc::Peer;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::api;
use crate::app::App;

impl opal_kit::ipc::Service for App {
    fn dispatch(
        self: Arc<Self>,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, anyhow::Result<Value>> {
        Box::pin(async move { api::dispatch(&self, &Peer::default(), &method, params).await })
    }

    fn dispatch_with_peer(
        self: Arc<Self>,
        peer: Peer,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, anyhow::Result<Value>> {
        Box::pin(async move { api::dispatch(&self, &peer, &method, params).await })
    }

    fn snapshot(self: Arc<Self>) -> BoxFuture<'static, Value> {
        Box::pin(async move { self.status().await })
    }

    fn events(&self) -> broadcast::Receiver<IpcEvent> {
        self.events.subscribe()
    }

    /// A connection that hasn't shown the UI session token gets the state
    /// snapshot and counts (what the bar gem needs), nothing an app asked
    /// to sign, no notification content, no offers.
    fn filter_event(&self, peer: &Peer, event: &IpcEvent) -> Option<IpcEvent> {
        if peer.session.is_privileged() {
            return Some(event.clone());
        }
        match event.event.as_str() {
            "state" | "pending" | "unread" => Some(event.clone()),
            _ => None,
        }
    }
}
