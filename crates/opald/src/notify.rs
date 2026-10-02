//! Desktop notifications. The panel and approval dialog are the main UI; these
//! make sure a waiting request isn't missed while you're elsewhere. They go
//! straight to the notification server over D-Bus, with the hints Omarchy's
//! `omarchy-notification-send` sets (glyph, image, what a click runs), and
//! never through a helper's command line: any user can read those in /proc,
//! and the text can be a private message.
//!
//! Everything shown comes partly from strangers (names, note text), so it is
//! sanitized: no leading dashes (they'd be read as options), no markup, no
//! control characters, limited length. Popups are rate-limited.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use opal_signer::Prompt;

use crate::app::App;

const GLYPH: &str = "󰇈";
/// At most this many Nostr popups per minute; the rest only land in the Inbox.
const POPUPS_PER_MINUTE: usize = 6;
/// One "waiting for approval" popup per app per this long.
const PROMPT_POPUP_EVERY: Duration = Duration::from_secs(30);

pub async fn prompt_opened(_app: &Arc<App>, prompt: &Prompt) {
    if !due(&prompt.request.connection_id, PROMPT_POPUP_EVERY) {
        return;
    }
    let what = match &prompt.kind_label {
        Some(label) => format!("wants to sign: {label}"),
        None => format!("wants to use {}", prompt.request.method),
    };
    send(&prompt.request.app_name, &what, Click::Approvals, None).await;
}

pub async fn unlock_needed(_app: &Arc<App>, app_name: &str, method: &str) {
    if !due(&format!("unlock:{app_name}"), PROMPT_POPUP_EVERY) {
        return;
    }
    send(
        app_name,
        &format!("is waiting ({method}). Unlock Opal to continue."),
        Click::Approvals,
        None,
    )
    .await;
}

enum Click {
    Approvals,
    Inbox,
    Url(String),
}

/// Open a notification link in the user's session, outside opald's sandbox
/// (a browser started from inside it would inherit its restrictions).
pub fn open_url(url: &str) {
    if !openable(url) {
        return;
    }
    let spawned = std::process::Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--", "xdg-open", url])
        .spawn();
    if let Ok(mut child) = spawned {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Links Opal hands to xdg-open: a web page, or a `nostr:` / `web+nostr:`
/// link to a NIP-19 identifier for the default Nostr app. Nothing else, so
/// a bad link can't make the desktop launch some other handler.
fn openable(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    [crate::handlers::NOSTR, crate::handlers::WEB_NOSTR]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme)?.strip_prefix(':'))
        .is_some_and(|body| {
            !body.is_empty()
                && body
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

async fn send(headline: &str, body: &str, click: Click, image: Option<&std::path::Path>) {
    let headline = clean(headline, 80);
    let body = clean(body, 240);
    if let Err(e) = notify_dbus(&headline, &body, click, image).await {
        tracing::debug!("sending a notification failed: {e}");
    }
}

async fn notify_dbus(
    headline: &str,
    body: &str,
    click: Click,
    image: Option<&std::path::Path>,
) -> zbus::Result<()> {
    use zbus::zvariant::Value;
    static SESSION: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
    let conn = SESSION.get_or_try_init(zbus::Connection::session).await?;
    // What a click runs, as Omarchy's notification server expects it: an
    // argv as JSON, never a shell string.
    let exec: Vec<String> = match click {
        Click::Approvals => vec!["omarchy-shell".into(), "opal".into(), "approvals".into()],
        Click::Inbox => vec![
            "omarchy-shell".into(),
            "opal".into(),
            "notifications".into(),
        ],
        Click::Url(u) => [
            "systemd-run",
            "--user",
            "--quiet",
            "--collect",
            "--",
            "xdg-open",
        ]
        .into_iter()
        .map(String::from)
        .chain([u])
        .collect(),
    };
    let exec = serde_json::to_string(&exec).unwrap_or_default();
    let mut hints: HashMap<&str, Value> = HashMap::new();
    hints.insert("urgency", Value::from(0u8));
    hints.insert("omarchy-glyph", Value::from(GLYPH));
    hints.insert("omarchy-exec-argv", Value::from(exec.as_str()));
    let image = image.map(|p| p.to_string_lossy().into_owned());
    if let Some(img) = &image {
        hints.insert("image-path", Value::from(img.as_str()));
    }
    let actions: Vec<&str> = vec![];
    conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &("Opal", 0u32, "", headline, body, actions, hints, 12_000i32),
    )
    .await?;
    Ok(())
}

/// Make untrusted text safe to hand to a notification: one line, no control
/// characters, no markup, can't start with '-', bounded length.
pub fn clean(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out: String = flat.chars().take(max).collect();
    if flat.chars().count() > max {
        out.push('…');
    }
    let out = out
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    // A leading dash would be an option to a command-line sender; D-Bus
    // takes the text as data, so this is only belt and braces now.
    if out.starts_with('-') {
        format!("\u{2011}{}", out.trim_start_matches('-'))
    } else {
        out
    }
}

/// Simple per-key cooldown.
fn due(key: &str, every: Duration) -> bool {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let mut map = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    map.retain(|_, t| now.duration_since(*t) < Duration::from_secs(3600));
    match map.get(key) {
        Some(t) if now.duration_since(*t) < every => false,
        _ => {
            map.insert(key.to_string(), now);
            true
        }
    }
}

/// Global popup budget for Nostr notifications.
fn popup_allowed() -> bool {
    static RECENT: OnceLock<Mutex<Vec<Instant>>> = OnceLock::new();
    let mut recent = RECENT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    recent.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
    if recent.len() >= POPUPS_PER_MINUTE {
        return false;
    }
    recent.push(now);
    true
}

/// Desktop popup for a new Nostr notification.
pub async fn nostr_popup(app: &Arc<App>, n: opal_notify::Notification) {
    use opal_notify::NotifType;

    let cfg = app.config.read().await.notifications.clone();
    if !cfg.desktop {
        return;
    }
    if n.ntype == NotifType::Zap && n.sats.unwrap_or(0) < cfg.min_zap_sats {
        return;
    }
    if !popup_allowed() {
        return;
    }
    // Give the profile lookup a moment so the popup shows a name and avatar.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let profile = app
        .notify_store
        .profile(&n.author)
        .ok()
        .flatten()
        .map(|(p, _)| p);
    let name = profile
        .as_ref()
        .and_then(|p| p.display_name.clone().or_else(|| p.name.clone()))
        .map(|n| clean(&n, 40))
        .unwrap_or_else(|| short_hex(&n.author));
    let headline = match n.ntype {
        NotifType::Reply => format!("{name} replied"),
        NotifType::Mention => format!("{name} mentioned you"),
        NotifType::Repost => format!("{name} reposted you"),
        NotifType::Reaction => format!("{name} reacted {}", clean(&n.detail, 24)),
        NotifType::Zap => match n.sats {
            Some(s) => format!("{name} zapped you {s} sats"),
            None => format!("{name} zapped you"),
        },
        NotifType::Dm => format!("Message from {name}"),
    };
    let body = match n.ntype {
        NotifType::Reaction | NotifType::Repost => String::new(),
        NotifType::Dm if n.detail.is_empty() => "Open Opal to read it".into(),
        _ => n.detail.clone(),
    };
    let avatar = match profile.as_ref().and_then(|p| p.picture.clone()) {
        Some(url) => cached_avatar(&n.author, &url).await,
        None => None,
    };

    // Replies and mentions open themselves; reactions, reposts and zaps open
    // the note they are about.
    let (target, author, kind) = match (n.ntype, &n.ref_id) {
        (NotifType::Reply | NotifType::Mention, _) | (_, None) => {
            (n.id.clone(), Some(n.author.clone()), Some(n.kind))
        }
        (_, Some(r)) => (r.clone(), None, None),
    };
    let click = if n.ntype == NotifType::Dm {
        Click::Inbox
    } else {
        match opal_notify::links::event_link(
            crate::handlers::opener(&cfg).await,
            &target,
            author.as_deref(),
            kind,
            n.relay.as_deref(),
        ) {
            Some(u) if openable(&u) => Click::Url(u),
            _ => Click::Inbox,
        }
    };
    send(&headline, &body, click, avatar.as_deref()).await;
}

fn short_hex(hex: &str) -> String {
    use nostr_sdk::prelude::{PublicKey, ToBech32};
    PublicKey::from_hex(hex)
        .ok()
        .and_then(|pk| pk.to_bech32().ok())
        .map(|npub| format!("{}…", &npub[..12]))
        .unwrap_or_else(|| hex.chars().take(8).collect())
}

/// Largest avatar we download, and how many we keep.
const AVATAR_MAX_BYTES: &str = "524288";
const AVATAR_CACHE_MAX: usize = 300;

/// Download an avatar once into ~/.cache/opal/avatars (the notification
/// daemon only shows local images). Keyed on pubkey + URL so a changed
/// picture is fetched again; old files are pruned.
async fn cached_avatar(pubkey: &str, url: &str) -> Option<std::path::PathBuf> {
    if !url.starts_with("https://") || !pubkey.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let dir = opal_core::paths::cache_dir().join("avatars");
    let name = format!("{pubkey}-{:016x}", fnv(url));
    let path = dir.join(&name);
    if path.metadata().is_ok_and(|m| m.len() > 0) {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let tmp = dir.join(format!("{name}.part"));
    let fetch = tokio::process::Command::new("curl")
        .args([
            "-fsL",
            "--max-time",
            "6",
            "--max-filesize",
            AVATAR_MAX_BYTES,
            "--max-redirs",
            "3",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-o",
        ])
        .arg(&tmp)
        .arg("--")
        .arg(url)
        .kill_on_drop(true)
        .status();
    let ok = matches!(
        tokio::time::timeout(Duration::from_secs(8), fetch).await,
        Ok(Ok(s)) if s.success()
    );
    if ok && tmp.metadata().is_ok_and(|m| m.len() > 0) && std::fs::rename(&tmp, &path).is_ok() {
        prune_avatars(&dir);
        Some(path)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

fn prune_avatars(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= AVATAR_CACHE_MAX {
        return;
    }
    files.sort();
    for (_, p) in files.iter().take(files.len() - AVATAR_CACHE_MAX) {
        let _ = std::fs::remove_file(p);
    }
}

/// Small, stable hash for cache file names (not security-relevant).
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_untrusted_text() {
        assert_eq!(clean("--exec rm -rf", 80), "\u{2011}exec rm -rf");
        assert_eq!(
            clean("<b>hi</b> & bye", 80),
            "&lt;b&gt;hi&lt;/b&gt; &amp; bye"
        );
        assert_eq!(clean("a\nb\tc", 80), "a b c");
        assert_eq!(clean("abcdef", 3), "abc…");
    }

    #[test]
    fn opens_only_web_and_nostr_links() {
        assert!(openable("https://njump.me/nevent1abc"));
        assert!(openable("nostr:nevent1qqs0abc"));
        assert!(openable("web+nostr:nevent1qqs0abc"));
        for bad in [
            "http://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "nostr:",
            "nostr:nevent1 --flag",
            "nostr:NEVENT1ABC",
            "web+nostr:nevent1abc/../x",
            "nostrconnect://x",
            "--help",
        ] {
            assert!(!openable(bad), "{bad}");
        }
    }
}
