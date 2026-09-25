//! Automatic status inputs: calendar (khal), screen lock, Do Not Disturb.

use std::time::Duration;

use tokio::process::Command;

/// Run a helper with a timeout; a hung one must not stall the status loop.
async fn output(cmd: &mut Command) -> Option<std::process::Output> {
    tokio::time::timeout(Duration::from_secs(3), cmd.kill_on_drop(true).output())
        .await
        .ok()?
        .ok()
}

/// Title of a timed event in progress, from `khal`. All-day events are
/// ignored (they'd make you "in a meeting" all day).
pub async fn current_meeting() -> Option<String> {
    let out = Command::new("khal")
        .args([
            "list", "now", "1m", "--json", "title", "--json", "start", "--json", "end", "--json",
            "all-day",
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_khal(&String::from_utf8_lossy(&out.stdout))
}

pub(crate) fn parse_khal(stdout: &str) -> Option<String> {
    // khal prints one JSON array per day line.
    for line in stdout.lines().filter(|l| l.trim_start().starts_with('[')) {
        let Ok(events) = serde_json::from_str::<Vec<serde_json::Value>>(line) else {
            continue;
        };
        for e in events {
            let all_day = matches!(
                e.get("all-day").and_then(|v| v.as_str()),
                Some("True" | "true")
            ) || e.get("all-day").and_then(|v| v.as_bool()) == Some(true);
            let start = e.get("start").and_then(|v| v.as_str()).unwrap_or("");
            // Timed events have a time part ("2026-09-24 18:00").
            if all_day || !start.contains(':') {
                continue;
            }
            return Some(
                e.get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
        }
    }
    None
}

/// Hyprland keeps an active session lock in `solitaryBlockedBy` (see
/// `omarchy-hyprland-session-locked`).
pub async fn screen_locked() -> bool {
    let Ok(out) = Command::new("hyprctl")
        .args(["-j", "monitors"])
        .output()
        .await
    else {
        return false;
    };
    let Ok(monitors) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return false;
    };
    monitors.as_array().is_some_and(|ms| {
        ms.iter().any(|m| {
            m["solitaryBlockedBy"]
                .as_array()
                .is_some_and(|b| b.iter().any(|r| r == "LOCK"))
        })
    })
}

/// Omarchy's notification Do Not Disturb switch.
pub async fn do_not_disturb() -> bool {
    output(Command::new("omarchy-shell").args(["notifications", "isDnd"]))
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn khal_output() {
        let out = r#"Today, 2026-09-24
[{"title": "Birthday", "start": "2026-09-24", "end": "2026-09-25", "all-day": "True"}, {"title": "Standup", "start": "2026-09-24 10:00", "end": "2026-09-24 10:15", "all-day": "False"}]
"#;
        assert_eq!(parse_khal(out).as_deref(), Some("Standup"));
        assert_eq!(parse_khal("[]\n"), None);
        assert_eq!(parse_khal(""), None);
        assert_eq!(
            parse_khal(
                r#"[{"title": "Holiday", "start": "2026-09-24", "end": "2026-09-25", "all-day": "True"}]"#
            ),
            None
        );
    }
}
