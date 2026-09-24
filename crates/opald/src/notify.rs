//! Desktop notifications. The panel and approval dialog are the main UI; these
//! make sure a waiting request isn't missed while you're elsewhere. On Omarchy
//! they go through `omarchy-notification-send` (clicking opens the approval
//! dialog); elsewhere through `notify-send`.

use std::sync::Arc;

use opal_signer::Prompt;

use crate::app::App;

const GLYPH: &str = "󰇈";

pub async fn prompt_opened(_app: &Arc<App>, prompt: &Prompt) {
    let what = match &prompt.kind_label {
        Some(label) => format!("wants to sign: {label}"),
        None => format!("wants to use {}", prompt.request.method),
    };
    send(&prompt.request.app_name, &what).await;
}

pub async fn unlock_needed(_app: &Arc<App>, app_name: &str, method: &str) {
    send(
        app_name,
        &format!("is waiting ({method}). Unlock Opal to continue."),
    )
    .await;
}

async fn send(headline: &str, body: &str) {
    let omarchy = which("omarchy-notification-send");
    let mut cmd = if omarchy {
        let mut c = tokio::process::Command::new("omarchy-notification-send");
        c.args([
            "--app-name",
            "Opal",
            "-g",
            GLYPH,
            "-t",
            "15000",
            headline,
            body,
        ]);
        c.args(["--exec", "omarchy-shell", "opal", "approvals"]);
        c
    } else {
        let mut c = tokio::process::Command::new("notify-send");
        c.args(["--app-name=Opal", "--expire-time=15000", headline, body]);
        c
    };
    // Don't wait: omarchy-notification-send blocks until the notification is
    // clicked or closed when --exec is used.
    match cmd.kill_on_drop(false).spawn() {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(e) => tracing::debug!("sending a notification failed: {e}"),
    }
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
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
    // Give the profile lookup a moment so the popup shows a name and avatar.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    let profile = app
        .notify_store
        .profile(&n.author)
        .ok()
        .flatten()
        .map(|(p, _)| p);
    let name = profile
        .as_ref()
        .and_then(|p| p.display_name.clone().or_else(|| p.name.clone()))
        .unwrap_or_else(|| short_hex(&n.author));
    let headline = match n.ntype {
        NotifType::Reply => format!("{name} replied"),
        NotifType::Mention => format!("{name} mentioned you"),
        NotifType::Repost => format!("{name} reposted you"),
        NotifType::Reaction => format!("{name} reacted {}", n.detail),
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
    let url = if n.ntype == NotifType::Dm {
        None
    } else {
        opal_notify::links::event_url(
            &cfg.client,
            &target,
            author.as_deref(),
            kind,
            n.relay.as_deref(),
        )
    };

    let mut cmd;
    if which("omarchy-notification-send") {
        cmd = tokio::process::Command::new("omarchy-notification-send");
        cmd.args(["--app-name", "Nostr", "-g", GLYPH, "-t", "10000"]);
        if let Some(img) = &avatar {
            cmd.arg("--image").arg(img);
        }
        cmd.args([headline.as_str(), body.as_str()]);
        match &url {
            Some(u) => cmd.args(["--exec", "xdg-open", u.as_str()]),
            None => cmd.args(["--exec", "omarchy-shell", "opal", "notifications"]),
        };
    } else {
        cmd = tokio::process::Command::new("notify-send");
        cmd.args(["--app-name=Nostr", "--expire-time=10000"]);
        if let Some(img) = &avatar {
            cmd.arg(format!("--icon={}", img.display()));
        }
        cmd.args([headline.as_str(), body.as_str()]);
    }
    if let Ok(mut child) = cmd.spawn() {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

fn short_hex(hex: &str) -> String {
    use nostr_sdk::prelude::{PublicKey, ToBech32};
    PublicKey::from_hex(hex)
        .ok()
        .and_then(|pk| pk.to_bech32().ok())
        .map(|npub| format!("{}…", &npub[..12]))
        .unwrap_or_else(|| hex.chars().take(8).collect())
}

/// Download an avatar once into ~/.cache/opal/avatars (the notification
/// daemon only shows local images).
async fn cached_avatar(pubkey: &str, url: &str) -> Option<std::path::PathBuf> {
    let dir = opal_core::paths::cache_dir().join("avatars");
    let path = dir.join(pubkey);
    if path.metadata().is_ok_and(|m| m.len() > 0) {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    let status = tokio::process::Command::new("curl")
        .args([
            "-fsL",
            "--max-time",
            "6",
            "--max-filesize",
            "5000000",
            "--proto",
            "=https",
            "-o",
        ])
        .arg(&path)
        .arg(url)
        .status()
        .await
        .ok()?;
    if status.success() && path.metadata().is_ok_and(|m| m.len() > 0) {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}
