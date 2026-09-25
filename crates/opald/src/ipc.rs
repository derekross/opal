//! Opal on the control socket (served by opal-kit): requests go to
//! [`api::dispatch`], `subscribe` gets the full status plus live events.

use std::sync::Arc;

use futures::future::BoxFuture;
use opal_core::ipc::IpcEvent;
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
        Box::pin(async move { api::dispatch(&self, &method, params).await })
    }

    fn snapshot(self: Arc<Self>) -> BoxFuture<'static, Value> {
        Box::pin(async move { self.status().await })
    }

    fn events(&self) -> broadcast::Receiver<IpcEvent> {
        self.events.subscribe()
    }
}
