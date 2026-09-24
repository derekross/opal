//! The general status (NIP-38 `d=general`): a manual status you set, or an
//! automatic one (away, calendar, focus). Manual always wins.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manual {
    pub text: String,
    pub link: Option<String>,
    /// Unix time it stops applying; `None` = until cleared.
    pub expires_at: Option<u64>,
}

/// What the automatic sources currently say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoInputs {
    /// Title of a running calendar event.
    pub meeting: Option<String>,
    pub away: bool,
    pub focus: bool,
}

/// Texts and switches from settings.
#[derive(Debug, Clone)]
pub struct AutoSettings {
    pub calendar: bool,
    pub calendar_titles: bool,
    pub calendar_text: String,
    pub away: bool,
    pub away_text: String,
    pub focus: bool,
    pub focus_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Desired {
    pub content: String,
    pub link: Option<String>,
    pub expires_at: Option<u64>,
    /// manual, away, calendar or focus
    pub source: &'static str,
}

pub fn desired(
    manual: Option<&Manual>,
    auto: &AutoInputs,
    s: &AutoSettings,
    now: u64,
) -> Option<Desired> {
    if let Some(m) =
        manual.filter(|m| m.expires_at.is_none_or(|t| t > now) && !m.text.trim().is_empty())
    {
        return Some(Desired {
            content: m.text.trim().to_string(),
            link: m.link.clone(),
            expires_at: m.expires_at,
            source: "manual",
        });
    }
    if s.away && auto.away {
        return Some(auto_status(&s.away_text, "away"));
    }
    if s.calendar
        && let Some(title) = &auto.meeting
    {
        let text = if s.calendar_titles && !title.trim().is_empty() {
            format!("{}: {}", s.calendar_text, title.trim())
        } else {
            s.calendar_text.clone()
        };
        return Some(auto_status(&text, "calendar"));
    }
    if s.focus && auto.focus {
        return Some(auto_status(&s.focus_text, "focus"));
    }
    None
}

fn auto_status(text: &str, source: &'static str) -> Desired {
    Desired {
        content: text.to_string(),
        link: None,
        // The engine gives automatic statuses a short expiry and refreshes it
        // while they hold, so a crashed computer doesn't leave "Away" up forever.
        expires_at: None,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> AutoSettings {
        AutoSettings {
            calendar: true,
            calendar_titles: false,
            calendar_text: "In a meeting".into(),
            away: true,
            away_text: "Away".into(),
            focus: true,
            focus_text: "Focusing".into(),
        }
    }

    #[test]
    fn priority_manual_away_calendar_focus() {
        let s = settings();
        let all = AutoInputs {
            meeting: Some("Standup".into()),
            away: true,
            focus: true,
        };
        let manual = Manual {
            text: "Hacking on Opal".into(),
            link: None,
            expires_at: None,
        };
        assert_eq!(
            desired(Some(&manual), &all, &s, 0).unwrap().source,
            "manual"
        );
        assert_eq!(desired(None, &all, &s, 0).unwrap().content, "Away");
        let no_away = AutoInputs {
            away: false,
            ..all.clone()
        };
        assert_eq!(
            desired(None, &no_away, &s, 0).unwrap().content,
            "In a meeting"
        );
        let focus_only = AutoInputs {
            focus: true,
            ..Default::default()
        };
        assert_eq!(
            desired(None, &focus_only, &s, 0).unwrap().content,
            "Focusing"
        );
        assert_eq!(desired(None, &AutoInputs::default(), &s, 0), None);
    }

    #[test]
    fn switches_and_titles() {
        let mut s = settings();
        s.away = false;
        s.calendar_titles = true;
        let inputs = AutoInputs {
            meeting: Some("Soapbox sync".into()),
            away: true,
            focus: false,
        };
        assert_eq!(
            desired(None, &inputs, &s, 0).unwrap().content,
            "In a meeting: Soapbox sync"
        );
        s.calendar = false;
        assert_eq!(desired(None, &inputs, &s, 0), None);
    }

    #[test]
    fn expired_manual_falls_through() {
        let s = settings();
        let m = Manual {
            text: "Lunch".into(),
            link: None,
            expires_at: Some(100),
        };
        let focus = AutoInputs {
            focus: true,
            ..Default::default()
        };
        assert_eq!(desired(Some(&m), &focus, &s, 99).unwrap().source, "manual");
        assert_eq!(desired(Some(&m), &focus, &s, 100).unwrap().source, "focus");
    }
}
