//! Desktop notifications (org.freedesktop.Notifications via `notify-send`).
//! The panel is the main UI; these only make sure a waiting request or a
//! needed unlock isn't missed while it is closed.

use std::sync::Arc;

use opal_signer::Prompt;

use crate::app::App;

pub async fn prompt_opened(app: &Arc<App>, prompt: &Prompt) {
    let what = match (&prompt.request.method, &prompt.kind_label) {
        (_, Some(label)) => format!("wants to sign: {label}"),
        (m, None) => format!("wants to use {m}"),
    };
    send(app, &format!("{} {what}", prompt.request.app_name)).await;
}

pub async fn unlock_needed(app: &Arc<App>, app_name: &str, method: &str) {
    send(
        app,
        &format!("{app_name} is waiting ({method}). Unlock Opal to continue."),
    )
    .await;
}

async fn send(app: &Arc<App>, body: &str) {
    let _ = app;
    let result = tokio::process::Command::new("notify-send")
        .args([
            "--app-name=Opal",
            "--icon=dialog-password",
            "--urgency=normal",
            "--expire-time=15000",
            "Nostr signer",
            body,
        ])
        .status()
        .await;
    if let Err(e) = result {
        tracing::debug!("notify-send failed: {e}");
    }
}
