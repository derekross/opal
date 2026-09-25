//! Background work: forwarding events to the UI, auto-lock, lock-on-screen-lock
//! and housekeeping.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use opal_signer::{PromptEvent, SignerEvent};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::app::App;
use crate::{notify, profiles};

pub fn spawn_all(app: &Arc<App>) {
    tokio::spawn(forward_signer_events(app.clone()));
    tokio::spawn(forward_prompt_events(app.clone()));
    tokio::spawn(auto_lock(app.clone()));
    tokio::spawn(watch_hyprland_lock(app.clone()));
    tokio::spawn(watch_logind(app.clone()));
    tokio::spawn(housekeeping(app.clone()));
}

async fn forward_signer_events(app: Arc<App>) {
    let mut rx = app.signer.subscribe_events();
    loop {
        let ev = match rx.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        match &ev {
            // Only *your* approvals count as use for the auto-lock timer, so an
            // app can't keep the vault unlocked by making requests.
            SignerEvent::Request {
                allowed: true,
                source: opal_signer::permissions::Source::User,
                ..
            } => app.touch().await,
            SignerEvent::UnlockNeeded {
                app_name, method, ..
            } => notify::unlock_needed(&app, app_name, method.as_str()).await,
            _ => {}
        }
        app.emit("signer", serde_json::to_value(&ev).unwrap_or_default());
    }
}

async fn forward_prompt_events(app: Arc<App>) {
    let mut rx = app.prompts.subscribe();
    loop {
        let ev = match rx.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        };
        if let PromptEvent::Opened { prompt } = &ev {
            notify::prompt_opened(&app, prompt).await;
        }
        app.emit("prompt", serde_json::to_value(&ev).unwrap_or_default());
        app.emit("pending", json!({"count": app.prompts.pending().len()}));
    }
}

async fn auto_lock(app: Arc<App>) {
    let mut tick = tokio::time::interval(Duration::from_secs(15));
    loop {
        tick.tick().await;
        let minutes = app.config.read().await.signer.auto_lock_minutes;
        if let Some(m) = minutes
            && app.vault.is_unlocked()
            && app.idle_for().await >= Duration::from_secs(u64::from(m) * 60)
        {
            app.lock("idle").await;
        }
    }
}

/// Omarchy's lock screen doesn't announce itself, but Hyprland lists an active
/// session lock as a reason in each monitor's `solitaryBlockedBy`
/// (the same check as `omarchy-hyprland-session-locked`).
async fn watch_hyprland_lock(app: Arc<App>) {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    let mut tick = tokio::time::interval(Duration::from_secs(3));
    loop {
        tick.tick().await;
        if !app.vault.is_unlocked() || !app.config.read().await.signer.lock_on_screen_lock {
            continue;
        }
        if hyprland_session_locked().await == Some(true) {
            app.lock("screen locked").await;
        }
    }
}

async fn hyprland_session_locked() -> Option<bool> {
    // A hung hyprctl must not stall this watcher (it would stop locking).
    let out = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("hyprctl")
            .args(["-j", "monitors"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let monitors: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let locked = monitors.as_array()?.iter().any(|m| {
        m["solitaryBlockedBy"]
            .as_array()
            .is_some_and(|b| b.iter().any(|r| r == "LOCK"))
    });
    Some(locked)
}

/// Lock on `loginctl lock-session` and before suspend.
async fn watch_logind(app: Arc<App>) {
    if let Err(e) = logind_loop(&app).await {
        tracing::info!("not watching logind: {e}");
    }
}

async fn logind_loop(app: &Arc<App>) -> zbus::Result<()> {
    let conn = zbus::Connection::system().await?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .build();
    let mut stream = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        let Some(member) = header.member() else {
            continue;
        };
        let lock_on_screen_lock = app.config.read().await.signer.lock_on_screen_lock;
        match member.as_str() {
            "Lock" if lock_on_screen_lock => app.lock("session locked").await,
            "PrepareForSleep" if msg.body().deserialize::<bool>().unwrap_or(false) => {
                app.lock("suspending").await;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn housekeeping(app: Arc<App>) {
    // First profile fetch shortly after start, then every few hours.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut tick = tokio::time::interval(Duration::from_secs(10 * 60));
    let mut rounds: u64 = 0;
    loop {
        tick.tick().await;
        if let Err(e) = app.signer.prune().await {
            tracing::warn!("pruning failed: {e}");
        }
        if rounds.is_multiple_of(18) {
            profiles::refresh_all(&app).await;
        }
        rounds += 1;
    }
}
