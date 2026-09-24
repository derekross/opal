//! Per-app permission rules and how a request is judged against them.
//!
//! Order: full trust → an unexpired deny rule → an unexpired allow rule →
//! the Basic policy's safe list → ask the user.

use std::fmt;

use opal_core::config::Policy;
use serde::{Deserialize, Serialize};

use crate::kinds;
use crate::perms::PermSpec;
use crate::protocol::Method;

/// Kinds the Basic policy signs without asking: everyday, additive actions.
/// Kinds that overwrite state (profile 0, follows 3, relay list 10002,
/// mutes 10000) or delete (5) always ask. Stricter than Amber on purpose.
pub const BASIC_KINDS: &[u16] = &[
    1,     // short note
    6,     // repost
    7,     // reaction
    16,    // generic repost
    1111,  // comment
    9734,  // zap request
    22242, // relay auth
    24242, // blossom auth
    27235, // http auth
];

/// How long an answer to a prompt is remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Remember {
    #[serde(rename = "once")]
    Once,
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "1d")]
    OneDay,
    #[serde(rename = "1w")]
    OneWeek,
    #[serde(rename = "always")]
    Always,
}

impl Remember {
    /// `None` = don't store a rule; `Some(None)` = forever; `Some(Some(t))` = until t.
    pub fn until(self, now: u64) -> Option<Option<u64>> {
        let secs = match self {
            Self::Once => return None,
            Self::Always => return Some(None),
            Self::FiveMinutes => 5 * 60,
            Self::OneHour => 3600,
            Self::OneDay => 86_400,
            Self::OneWeek => 7 * 86_400,
        };
        Some(Some(now + secs))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rule {
    pub app_id: String,
    pub method: Method,
    /// `None` = every kind (for `sign_event`) or not applicable.
    pub kind: Option<u16>,
    pub allow: bool,
    /// Unix time the rule stops applying; `None` = forever.
    pub until: Option<u64>,
    pub created_at: u64,
}

impl Rule {
    fn active(&self, now: u64) -> bool {
        self.until.is_none_or(|t| t > now)
    }

    fn matches(&self, method: &Method, kind: Option<u16>) -> bool {
        self.method == *method && (self.kind.is_none() || self.kind == kind)
    }
}

/// Why a request was allowed or denied (shown in the activity log).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    FullTrust,
    Rule,
    BasicPolicy,
    User,
    Timeout,
    Locked,
    /// Blanket approvers used in tests and debugging.
    Automatic,
    Error,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FullTrust => "full trust",
            Self::Rule => "saved rule",
            Self::BasicPolicy => "basic policy",
            Self::User => "you",
            Self::Timeout => "timed out",
            Self::Locked => "locked",
            Self::Automatic => "automatic",
            Self::Error => "error",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evaluation {
    Allow(Source),
    Deny(Source),
    Ask,
}

pub fn evaluate(
    policy: Policy,
    rules: &[Rule],
    method: &Method,
    kind: Option<u16>,
    now: u64,
) -> Evaluation {
    if policy == Policy::FullTrust {
        return Evaluation::Allow(Source::FullTrust);
    }
    let applicable = || {
        rules
            .iter()
            .filter(|r| r.active(now) && r.matches(method, kind))
    };
    if applicable().any(|r| !r.allow) {
        return Evaluation::Deny(Source::Rule);
    }
    if applicable().any(|r| r.allow) {
        return Evaluation::Allow(Source::Rule);
    }
    if policy == Policy::Basic && basic_allows(method, kind) {
        return Evaluation::Allow(Source::BasicPolicy);
    }
    Evaluation::Ask
}

fn basic_allows(method: &Method, kind: Option<u16>) -> bool {
    match method {
        Method::GetPublicKey => true,
        Method::SignEvent => kind.is_some_and(|k| BASIC_KINDS.contains(&k)),
        _ => false,
    }
}

/// `(method, kind)` pairs to pre-approve from a client's requested perms.
pub fn expand_perms(perms: &[PermSpec]) -> Vec<(Method, Option<u16>)> {
    let mut out = Vec::new();
    for p in perms {
        match p {
            PermSpec::Method { method, kind } => {
                // A bare `sign_event` would mean "sign anything"; never pre-approve that.
                if *method == Method::SignEvent && kind.is_none() {
                    continue;
                }
                if matches!(method, Method::Other(_) | Method::Connect) {
                    continue;
                }
                out.push((method.clone(), *kind));
            }
            PermSpec::Nip { nip } => {
                for k in kinds::kinds_of_nip(*nip) {
                    out.push((Method::SignEvent, Some(k)));
                }
            }
        }
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(method: Method, kind: Option<u16>, allow: bool, until: Option<u64>) -> Rule {
        Rule {
            app_id: "a".into(),
            method,
            kind,
            allow,
            until,
            created_at: 0,
        }
    }

    #[test]
    fn full_trust_beats_everything() {
        let rules = [rule(Method::SignEvent, None, false, None)];
        assert_eq!(
            evaluate(Policy::FullTrust, &rules, &Method::SignEvent, Some(1), 10),
            Evaluation::Allow(Source::FullTrust)
        );
    }

    #[test]
    fn deny_beats_allow() {
        let rules = [
            rule(Method::SignEvent, Some(1), true, None),
            rule(Method::SignEvent, None, false, None),
        ];
        assert_eq!(
            evaluate(Policy::Manual, &rules, &Method::SignEvent, Some(1), 10),
            Evaluation::Deny(Source::Rule)
        );
    }

    #[test]
    fn kind_specific_rules_only_match_their_kind() {
        let rules = [rule(Method::SignEvent, Some(1), true, None)];
        assert_eq!(
            evaluate(Policy::Manual, &rules, &Method::SignEvent, Some(1), 10),
            Evaluation::Allow(Source::Rule)
        );
        assert_eq!(
            evaluate(Policy::Manual, &rules, &Method::SignEvent, Some(7), 10),
            Evaluation::Ask
        );
    }

    #[test]
    fn expired_rules_are_ignored() {
        let rules = [rule(Method::Nip44Decrypt, None, true, Some(100))];
        assert_eq!(
            evaluate(Policy::Manual, &rules, &Method::Nip44Decrypt, None, 99),
            Evaluation::Allow(Source::Rule)
        );
        assert_eq!(
            evaluate(Policy::Manual, &rules, &Method::Nip44Decrypt, None, 100),
            Evaluation::Ask
        );
    }

    #[test]
    fn basic_policy() {
        let e = |m: Method, k| evaluate(Policy::Basic, &[], &m, k, 0);
        assert_eq!(
            e(Method::GetPublicKey, None),
            Evaluation::Allow(Source::BasicPolicy)
        );
        assert_eq!(
            e(Method::SignEvent, Some(1)),
            Evaluation::Allow(Source::BasicPolicy)
        );
        assert_eq!(
            e(Method::SignEvent, Some(0)),
            Evaluation::Ask,
            "profile overwrite asks"
        );
        assert_eq!(
            e(Method::SignEvent, Some(5)),
            Evaluation::Ask,
            "deletion asks"
        );
        assert_eq!(e(Method::Nip44Decrypt, None), Evaluation::Ask);
        // Manual asks even for the basic kinds.
        assert_eq!(
            evaluate(Policy::Manual, &[], &Method::SignEvent, Some(1), 0),
            Evaluation::Ask
        );
    }

    #[test]
    fn remember_durations() {
        assert_eq!(Remember::Once.until(10), None);
        assert_eq!(Remember::Always.until(10), Some(None));
        assert_eq!(Remember::OneHour.until(10), Some(Some(3610)));
    }

    #[test]
    fn expanding_requested_perms() {
        let perms =
            crate::perms::parse_perms("sign_event,sign_event:1,nip44_encrypt,nip:17,connect");
        let got = expand_perms(&perms);
        assert_eq!(
            got,
            vec![
                (Method::SignEvent, Some(1)),
                (Method::Nip44Encrypt, None),
                (Method::SignEvent, Some(14)),
                (Method::SignEvent, Some(15)),
                (Method::SignEvent, Some(10050)),
            ]
        );
    }
}
