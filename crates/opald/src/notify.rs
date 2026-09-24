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
