//! Wire format of the local control socket (`$XDG_RUNTIME_DIR/opal.sock`).
//!
//! Newline-delimited JSON. Clients send requests; the daemon answers each
//! with a response carrying the same `id`. After a `subscribe` request the
//! daemon also pushes events on that connection.
//!
//! ```text
//! → {"id":1,"method":"unlock","params":{"passphrase":"…"}}
//! ← {"id":1,"result":{"locked":false}}
//! ← {"event":"state","data":{…}}
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEvent {
    pub event: String,
    pub data: Value,
}

/// Anything the daemon may write on a connection.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum IpcMessage {
    Response(IpcResponse),
    Event(IpcEvent),
}
