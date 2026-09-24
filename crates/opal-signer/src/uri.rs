//! `bunker://` and `nostrconnect://` URIs.

use nostr_sdk::prelude::{PublicKey, RelayUrl};
use serde::Deserialize;
use url::Url;

use crate::perms::{PermSpec, parse_perms};

/// A client-initiated connection request (`nostrconnect://`).
#[derive(Debug, Clone)]
pub struct NostrConnectUri {
    pub client: PublicKey,
    pub relays: Vec<RelayUrl>,
    pub secret: String,
    pub perms: Vec<PermSpec>,
    pub name: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
}

/// Older clients put name/url/perms in a JSON `metadata` parameter.
#[derive(Deserialize, Default)]
struct LegacyMetadata {
    name: Option<String>,
    url: Option<String>,
    #[serde(default)]
    icons: Vec<String>,
    #[serde(default)]
    perms: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UriError {
    #[error("not a nostrconnect:// URI")]
    Scheme,
    #[error("invalid client public key")]
    PublicKey,
    #[error("the URI has no relays")]
    NoRelays,
    #[error("the URI has no secret")]
    NoSecret,
    #[error("malformed URI: {0}")]
    Malformed(String),
}

impl NostrConnectUri {
    pub fn parse(input: &str) -> Result<Self, UriError> {
        let input = input.trim();
        let url = Url::parse(input).map_err(|e| UriError::Malformed(e.to_string()))?;
        if url.scheme() != "nostrconnect" {
            return Err(UriError::Scheme);
        }
        let client = url
            .host_str()
            .and_then(|h| PublicKey::parse(h).ok())
            .ok_or(UriError::PublicKey)?;

        let mut relays = Vec::new();
        let mut secret = None;
        let mut perms = Vec::new();
        let (mut name, mut app_url, mut image) = (None, None, None);
        let mut legacy = LegacyMetadata::default();

        for (key, value) in url.query_pairs() {
            let value = value.into_owned();
            match key.as_ref() {
                "relay" => {
                    if let Ok(r) = RelayUrl::parse(&value)
                        && !relays.contains(&r)
                    {
                        relays.push(r);
                    }
                }
                "secret" => secret = Some(value),
                "perms" => perms = parse_perms(&value),
                "name" => name = Some(value),
                "url" => app_url = Some(value),
                "image" => image = Some(value),
                "metadata" => legacy = serde_json::from_str(&value).unwrap_or_default(),
                _ => {}
            }
        }

        if relays.is_empty() {
            return Err(UriError::NoRelays);
        }
        let secret = secret.filter(|s| !s.is_empty()).ok_or(UriError::NoSecret)?;
        if perms.is_empty()
            && let Some(p) = &legacy.perms
        {
            perms = parse_perms(p);
        }

        Ok(Self {
            client,
            relays,
            secret,
            perms,
            name: non_empty(name.or(legacy.name)),
            url: non_empty(app_url.or(legacy.url)),
            image: non_empty(image.or(legacy.icons.into_iter().next())),
        })
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

/// `bunker://<signer-pubkey>?relay=…&secret=…`
pub fn bunker_uri(signer: &PublicKey, relays: &[RelayUrl], secret: Option<&str>) -> String {
    let mut url = Url::parse(&format!("bunker://{}", signer.to_hex())).expect("valid base");
    {
        let mut q = url.query_pairs_mut();
        for r in relays {
            q.append_pair("relay", r.as_str());
        }
        if let Some(s) = secret {
            q.append_pair("secret", s);
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Method;

    const PK: &str = "fa984bd7dbb282f07e16e7ae87b26a2a7b9b90b7246a44771f0cf5ae58018f52";

    #[test]
    fn parses_full_uri() {
        let uri = format!(
            "nostrconnect://{PK}?relay=wss%3A%2F%2Frelay.example.com&relay=wss://r2.example&secret=s3cr3t&perms=sign_event%3A1%2Cnip44_encrypt&name=My%20App&url=https%3A%2F%2Fapp.example&image=https%3A%2F%2Fapp.example%2Ficon.png"
        );
        let p = NostrConnectUri::parse(&uri).unwrap();
        assert_eq!(p.client.to_hex(), PK);
        assert_eq!(p.relays.len(), 2);
        assert_eq!(p.secret, "s3cr3t");
        assert_eq!(p.name.as_deref(), Some("My App"));
        assert_eq!(p.image.as_deref(), Some("https://app.example/icon.png"));
        assert_eq!(
            p.perms[0],
            PermSpec::Method {
                method: Method::SignEvent,
                kind: Some(1)
            }
        );
    }

    #[test]
    fn legacy_metadata() {
        let meta = url::form_urlencoded::byte_serialize(
            br#"{"name":"Old","url":"https://old.example","perms":"nip04_encrypt"}"#,
        )
        .collect::<String>();
        let uri = format!("nostrconnect://{PK}?relay=wss://r.example&secret=x&metadata={meta}");
        let p = NostrConnectUri::parse(&uri).unwrap();
        assert_eq!(p.name.as_deref(), Some("Old"));
        assert_eq!(p.perms.len(), 1);
    }

    #[test]
    fn required_fields() {
        assert_eq!(
            NostrConnectUri::parse(&format!("nostrconnect://{PK}?secret=x")).unwrap_err(),
            UriError::NoRelays
        );
        assert_eq!(
            NostrConnectUri::parse(&format!("nostrconnect://{PK}?relay=wss://r.example"))
                .unwrap_err(),
            UriError::NoSecret
        );
        assert_eq!(
            NostrConnectUri::parse("bunker://abc").unwrap_err(),
            UriError::Scheme
        );
    }

    #[test]
    fn builds_bunker_uri() {
        let pk = PublicKey::parse(PK).unwrap();
        let relays = vec![RelayUrl::parse("wss://relay.example.com").unwrap()];
        let uri = bunker_uri(&pk, &relays, Some("abc"));
        assert_eq!(
            uri,
            format!("bunker://{PK}?relay=wss%3A%2F%2Frelay.example.com&secret=abc")
        );
    }
}
