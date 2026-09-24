//! Turning what a person types (npub, nprofile, hex, or a NIP-05 address like
//! `derek@grownostr.org`) into a public key.

use std::time::Duration;

use nostr::key::PublicKey;
use nostr::nips::nip05::{Nip05Address, Nip05Profile};
use nostr::nips::nip19::{FromBech32, Nip19Profile, ToBech32};
use serde::Serialize;

use crate::{Error, Result};

/// NIP-05 documents are small; refuse anything bigger than this.
const MAX_BODY: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Resolved {
    pub pubkey: String,
    pub npub: String,
    /// The NIP-05 address it was found through, if any.
    pub nip05: Option<String>,
    /// Relays the NIP-05 document lists for this key.
    pub relays: Vec<String>,
}

/// What kind of input this is, without any network access.
pub enum Input {
    Key(PublicKey),
    Nip05(Nip05Address),
}

pub fn classify(input: &str) -> Result<Input> {
    let s = input.trim().trim_start_matches("nostr:");
    if s.is_empty() {
        return Err(Error::Invalid("enter an npub or a NIP-05 address".into()));
    }
    if let Ok(pk) = PublicKey::from_bech32(s) {
        return Ok(Input::Key(pk));
    }
    if let Ok(p) = Nip19Profile::from_bech32(s) {
        return Ok(Input::Key(p.public_key));
    }
    if s.len() == 64
        && let Ok(pk) = PublicKey::from_hex(s)
    {
        return Ok(Input::Key(pk));
    }
    // name@domain, or a bare domain meaning `_@domain`.
    let lower = s.to_ascii_lowercase();
    let (name, domain) = match lower.split_once('@') {
        Some((n, d)) => (if n.is_empty() { "_" } else { n }, d),
        None => ("_", lower.as_str()),
    };
    let name_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    let domain_ok = domain.contains('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':'));
    if !name_ok || !domain_ok {
        return Err(Error::Invalid(
            "that isn't an npub or a NIP-05 address (name@domain)".into(),
        ));
    }
    let addr = Nip05Address::parse(&format!("{name}@{domain}")).map_err(Error::nostr)?;
    Ok(Input::Nip05(addr))
}

/// Resolve `input` to a public key, looking up NIP-05 addresses over HTTPS.
pub async fn resolve(input: &str) -> Result<Resolved> {
    match classify(input)? {
        Input::Key(pk) => Ok(resolved(pk, None, vec![])),
        Input::Nip05(addr) => {
            let body = fetch(addr.url().as_str()).await?;
            let profile = Nip05Profile::from_raw_json(&addr, &body).map_err(|_| {
                Error::Invalid(format!("{} isn't listed at {}", addr.name(), addr.domain()))
            })?;
            let shown = if addr.name() == "_" {
                addr.domain().to_string()
            } else {
                format!("{}@{}", addr.name(), addr.domain())
            };
            let relays = profile.relays.iter().map(|r| r.to_string()).collect();
            Ok(resolved(profile.public_key, Some(shown), relays))
        }
    }
}

fn resolved(pk: PublicKey, nip05: Option<String>, relays: Vec<String>) -> Resolved {
    Resolved {
        pubkey: pk.to_hex(),
        npub: pk.to_bech32().unwrap_or_default(),
        nip05,
        relays,
    }
}

async fn fetch(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        // NIP-05: fetchers must ignore redirects.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let net = |e: reqwest::Error| Error::Invalid(format!("couldn't reach that domain: {e}"));
    let mut resp = client.get(url).send().await.map_err(net)?;
    if !resp.status().is_success() {
        return Err(Error::Invalid(format!(
            "that domain answered {} (no NIP-05 there?)",
            resp.status()
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(net)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err(Error::Invalid("the NIP-05 document is too large".into()));
        }
    }
    String::from_utf8(body).map_err(|_| Error::Invalid("the NIP-05 document isn't text".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::key::Keys;

    #[test]
    fn keys_are_recognised_offline() {
        let pk = Keys::generate().public_key();
        let npub = pk.to_bech32().unwrap();
        let nprofile = Nip19Profile::new(pk, []).to_bech32().unwrap();
        for input in [
            npub.clone(),
            format!("nostr:{npub}"),
            nprofile,
            pk.to_hex(),
            format!("  {npub} "),
        ] {
            match classify(&input).unwrap() {
                Input::Key(k) => assert_eq!(k, pk, "{input}"),
                Input::Nip05(_) => panic!("{input} is a key"),
            }
        }
    }

    #[test]
    fn nip05_addresses() {
        let Input::Nip05(a) = classify("DerekRoss@GrowNostr.org").unwrap() else {
            panic!()
        };
        assert_eq!((a.name(), a.domain()), ("derekross", "grownostr.org"));
        assert_eq!(
            a.url().as_str(),
            "https://grownostr.org/.well-known/nostr.json?name=derekross"
        );
        let Input::Nip05(a) = classify("grownostr.org").unwrap() else {
            panic!()
        };
        assert_eq!(a.name(), "_");
        let Input::Nip05(a) = classify("@grownostr.org").unwrap() else {
            panic!()
        };
        assert_eq!(a.name(), "_");
    }

    #[test]
    fn junk_is_rejected() {
        for bad in [
            "",
            "hello",
            "npub1nope",
            "a b@c.d",
            "me@localhost",
            "x@evil.com/../",
        ] {
            assert!(classify(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn nip05_json_is_matched_by_name() {
        let pk = Keys::generate().public_key();
        let Input::Nip05(addr) = classify("bob@example.com").unwrap() else {
            panic!()
        };
        let json = format!(
            r#"{{"names":{{"bob":"{}"}},"relays":{{"{}":["wss://r.example.com"]}}}}"#,
            pk.to_hex(),
            pk.to_hex()
        );
        let p = Nip05Profile::from_raw_json(&addr, &json).unwrap();
        assert_eq!(p.public_key, pk);
        assert_eq!(p.relays.len(), 1);
        let other = r#"{"names":{"alice":"0000000000000000000000000000000000000000000000000000000000000001"}}"#;
        assert!(Nip05Profile::from_raw_json(&addr, other).is_err());
    }

    /// Real lookup; run with `cargo test -p opal-core -- --ignored nip05_live`.
    #[tokio::test]
    #[ignore = "network"]
    async fn nip05_live() {
        let r = resolve("derekross@grownostr.org").await.unwrap();
        assert!(r.npub.starts_with("npub1"));
        assert_eq!(r.nip05.as_deref(), Some("derekross@grownostr.org"));
    }
}
