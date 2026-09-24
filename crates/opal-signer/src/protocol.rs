//! NIP-46 message payloads (the decrypted `content` of kind 24133 events).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum Method {
    Connect,
    GetPublicKey,
    SignEvent,
    Nip04Encrypt,
    Nip04Decrypt,
    Nip44Encrypt,
    Nip44Decrypt,
    Ping,
    SwitchRelays,
    Logout,
    Other(String),
}

impl Method {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Connect => "connect",
            Self::GetPublicKey => "get_public_key",
            Self::SignEvent => "sign_event",
            Self::Nip04Encrypt => "nip04_encrypt",
            Self::Nip04Decrypt => "nip04_decrypt",
            Self::Nip44Encrypt => "nip44_encrypt",
            Self::Nip44Decrypt => "nip44_decrypt",
            Self::Ping => "ping",
            Self::SwitchRelays => "switch_relays",
            Self::Logout => "logout",
            Self::Other(s) => s,
        }
    }

    pub fn is_encryption(&self) -> bool {
        matches!(
            self,
            Self::Nip04Encrypt | Self::Nip04Decrypt | Self::Nip44Encrypt | Self::Nip44Decrypt
        )
    }
}

impl From<&str> for Method {
    fn from(s: &str) -> Self {
        match s {
            "connect" => Self::Connect,
            "get_public_key" => Self::GetPublicKey,
            "sign_event" => Self::SignEvent,
            "nip04_encrypt" => Self::Nip04Encrypt,
            "nip04_decrypt" => Self::Nip04Decrypt,
            "nip44_encrypt" => Self::Nip44Encrypt,
            "nip44_decrypt" => Self::Nip44Decrypt,
            "ping" => Self::Ping,
            "switch_relays" => Self::SwitchRelays,
            "logout" => Self::Logout,
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<String> for Method {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Method> for String {
    fn from(m: Method) -> Self {
        m.as_str().to_string()
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub id: String,
    pub method: Method,
    #[serde(default, deserialize_with = "string_params")]
    pub params: Vec<String>,
}

impl Request {
    pub fn param(&self, i: usize) -> Option<&str> {
        self.params.get(i).map(String::as_str)
    }
}

/// Params are specified as strings; be lenient with clients that send
/// numbers or nulls.
fn string_params<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values: Option<Vec<Value>> = Option::deserialize(d)?;
    Ok(values
        .unwrap_or_default()
        .into_iter()
        .map(|v| match v {
            Value::String(s) => s,
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response {
    pub id: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: impl Into<String>, result: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            result: result.into(),
            error: None,
        }
    }

    /// `result` is set to `"error"` because some clients (rust-nostr) parse
    /// `result` before looking at `error`.
    pub fn err(id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            result: "error".into(),
            error: Some(error.into()),
        }
    }
}

/// The encryption a client used; replies must use the same one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Nip44,
    Nip04,
}

impl Transport {
    /// NIP-04 payloads are `base64?iv=base64`; NIP-44 payloads never contain `?`.
    pub fn detect(content: &str) -> Self {
        if content.contains("?iv=") {
            Self::Nip04
        } else {
            Self::Nip44
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_request_with_odd_params() {
        let r: Request =
            serde_json::from_str(r#"{"id":"1","method":"sign_event","params":["{}",5,null]}"#)
                .unwrap();
        assert_eq!(r.method, Method::SignEvent);
        assert_eq!(r.params, vec!["{}", "5", ""]);

        let r: Request = serde_json::from_str(r#"{"id":"2","method":"frobnicate"}"#).unwrap();
        assert_eq!(r.method, Method::Other("frobnicate".into()));
        assert!(r.params.is_empty());
    }

    #[test]
    fn serialize_response() {
        let ok = serde_json::to_string(&Response::ok("1", "pong")).unwrap();
        assert_eq!(ok, r#"{"id":"1","result":"pong"}"#);
        let err = serde_json::to_string(&Response::err("1", "no")).unwrap();
        assert_eq!(err, r#"{"id":"1","result":"error","error":"no"}"#);
    }

    #[test]
    fn detect_transport() {
        assert_eq!(Transport::detect("abc?iv=def"), Transport::Nip04);
        assert_eq!(Transport::detect("AgAAAA=="), Transport::Nip44);
    }
}
