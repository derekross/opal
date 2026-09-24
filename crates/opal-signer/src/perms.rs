//! The `perms` string clients send: `method[:kind]` entries separated by
//! commas, e.g. `sign_event:1,nip44_encrypt`. Amber also accepts `nip:<n>`
//! meaning "every kind of NIP n"; we keep that as its own variant.

use serde::{Deserialize, Serialize};

use crate::protocol::Method;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermSpec {
    Method { method: Method, kind: Option<u16> },
    Nip { nip: u16 },
}

pub fn parse_perms(s: &str) -> Vec<PermSpec> {
    let mut out: Vec<PermSpec> = Vec::new();
    for item in s.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let (name, arg) = match item.split_once(':') {
            Some((n, a)) => (n.trim(), Some(a.trim())),
            None => (item, None),
        };
        let spec = if name == "nip" {
            match arg.and_then(|a| a.parse().ok()) {
                Some(nip) => PermSpec::Nip { nip },
                None => continue,
            }
        } else {
            let method = Method::from(name);
            let kind = match arg {
                Some(a) => match a.parse() {
                    Ok(k) => Some(k),
                    Err(_) => continue,
                },
                None => None,
            };
            PermSpec::Method { method, kind }
        };
        if !out.contains(&spec) {
            out.push(spec);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mixed_list() {
        let perms =
            parse_perms("sign_event:1, nip44_encrypt,sign_event:1,nip:17,nip:x,sign_event:abc,");
        assert_eq!(
            perms,
            vec![
                PermSpec::Method {
                    method: Method::SignEvent,
                    kind: Some(1)
                },
                PermSpec::Method {
                    method: Method::Nip44Encrypt,
                    kind: None
                },
                PermSpec::Nip { nip: 17 },
            ]
        );
    }
}
