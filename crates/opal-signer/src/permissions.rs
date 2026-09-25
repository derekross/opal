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
/// mutes 10000) or delete (5) always ask, and so do HTTP (27235) and Blossom
/// (24242) authorizations, which work as login tokens for other services.
/// Stricter than Amber on purpose.
pub const BASIC_KINDS: &[u16] = &[
    1,     // short note
    6,     // repost
    7,     // reaction
    16,    // generic repost
    1111,  // comment
    9734,  // zap request
    22242, // relay auth (a relay challenge; short-lived)
];

/// Kinds a client can never get blanket, long-lived permission for: they
/// overwrite or destroy state, move money, or act as credentials. Requests
/// for them via `perms`/`nip:<n>` are ignored and remembered answers are
/// capped at one hour.
pub const SENSITIVE_KINDS: &[u16] = &[
    0,     // profile
    3,     // follow list
    5,     // deletion
    62,    // request to vanish
    9735,  // zap receipt
    10000, // mute list
    10002, // relay list
    10050, // DM relays
    13194, // wallet info
    17375, // cashu wallet
    22242, // relay auth
    23194, // wallet request
    24242, // blossom auth
    27235, // http auth
];

/// Automatic approvals (policy or saved rule) only for events dated within
/// this many seconds of now; anything else asks, so an app can't pre-mint
/// tokens or backdate events behind your back.
pub const AUTO_MAX_SKEW: u64 = 600;

pub fn is_sensitive(method: &Method, kind: Option<u16>) -> bool {
    match method {
        Method::SignEvent => kind.is_none_or(|k| SENSITIVE_KINDS.contains(&k)),
        _ => false,
    }
}

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
    created_at: Option<u64>,
    now: u64,
) -> Evaluation {
    // Oddly dated events always need a person to look at them.
    if created_at.is_some_and(|t| t.abs_diff(now) > AUTO_MAX_SKEW) {
        let denied = rules
            .iter()
            .any(|r| r.active(now) && r.matches(method, kind) && !r.allow);
        return if denied {
            Evaluation::Deny(Source::Rule)
        } else {
            Evaluation::Ask
        };
    }
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
                // Never pre-approve: signing anything (bare `sign_event`),
                // sensitive kinds, or decrypting every conversation.
                if is_sensitive(method, *kind)
                    || matches!(
                        method,
                        Method::Other(_)
                            | Method::Connect
                            | Method::Nip04Decrypt
                            | Method::Nip44Decrypt
                    )
                {
                    continue;
                }
                out.push((method.clone(), *kind));
            }
            PermSpec::Nip { nip } => {
                for k in kinds::kinds_of_nip(*nip) {
                    if !SENSITIVE_KINDS.contains(&k) {
                        out.push((Method::SignEvent, Some(k)));
                    }
                }
            }
        }
    }
    let mut seen = Vec::new();
    out.retain(|p| {
        let new = !seen.contains(p);
        seen.push(p.clone());
        new
    });
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
            evaluate(
                Policy::FullTrust,
                &rules,
                &Method::SignEvent,
                Some(1),
                None,
                10
            ),
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
            evaluate(
                Policy::Manual,
                &rules,
                &Method::SignEvent,
                Some(1),
                None,
                10
            ),
            Evaluation::Deny(Source::Rule)
        );
    }

    #[test]
    fn kind_specific_rules_only_match_their_kind() {
        let rules = [rule(Method::SignEvent, Some(1), true, None)];
        assert_eq!(
            evaluate(
                Policy::Manual,
                &rules,
                &Method::SignEvent,
                Some(1),
                None,
                10
            ),
            Evaluation::Allow(Source::Rule)
        );
        assert_eq!(
            evaluate(
                Policy::Manual,
                &rules,
                &Method::SignEvent,
                Some(7),
                None,
                10
            ),
            Evaluation::Ask
        );
    }

    #[test]
    fn expired_rules_are_ignored() {
        let rules = [rule(Method::Nip44Decrypt, None, true, Some(100))];
        assert_eq!(
            evaluate(
                Policy::Manual,
                &rules,
                &Method::Nip44Decrypt,
                None,
                None,
                99
            ),
            Evaluation::Allow(Source::Rule)
        );
        assert_eq!(
            evaluate(
                Policy::Manual,
                &rules,
                &Method::Nip44Decrypt,
                None,
                None,
                100
            ),
            Evaluation::Ask
        );
    }

    #[test]
    fn basic_policy() {
        let e = |m: Method, k| evaluate(Policy::Basic, &[], &m, k, None, 0);
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
            evaluate(Policy::Manual, &[], &Method::SignEvent, Some(1), None, 0),
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
            ],
            "DM relay list (10050) is sensitive"
        );
    }

    #[test]
    fn requested_perms_never_grant_dangerous_things() {
        let perms = crate::perms::parse_perms(
            "sign_event:0,sign_event:3,sign_event:5,sign_event:27235,nip44_decrypt,nip04_decrypt,nip:1,nip:2,nip:9,nip:62,nip:98",
        );
        assert_eq!(
            expand_perms(&perms),
            vec![],
            "nothing sensitive, no blanket decrypt"
        );
    }

    #[test]
    fn oddly_dated_events_always_ask() {
        let trusted = [rule(Method::SignEvent, Some(1), true, None)];
        // Within 10 minutes: automatic as usual.
        assert_eq!(
            evaluate(
                Policy::Basic,
                &[],
                &Method::SignEvent,
                Some(1),
                Some(1000),
                1300
            ),
            Evaluation::Allow(Source::BasicPolicy)
        );
        // An hour in the future: ask, even with full trust or a saved rule.
        for policy in [Policy::Basic, Policy::FullTrust] {
            assert_eq!(
                evaluate(
                    policy,
                    &trusted,
                    &Method::SignEvent,
                    Some(1),
                    Some(1000 + 3600),
                    1000
                ),
                Evaluation::Ask
            );
        }
        // Deny rules still deny.
        let denied = [rule(Method::SignEvent, Some(1), false, None)];
        assert_eq!(
            evaluate(
                Policy::Basic,
                &denied,
                &Method::SignEvent,
                Some(1),
                Some(0),
                5000
            ),
            Evaluation::Deny(Source::Rule)
        );
    }

    #[test]
    fn auth_tokens_ask_under_basic() {
        for k in [24242, 27235] {
            assert_eq!(
                evaluate(Policy::Basic, &[], &Method::SignEvent, Some(k), None, 0),
                Evaluation::Ask
            );
        }
    }
}
