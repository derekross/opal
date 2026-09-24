//! opald — the Opal daemon.

mod api;
mod app;
mod ipc;
mod modules;
mod notify;
mod profiles;
mod tasks;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use opal_core::config::Config;
use opal_core::db::Db;
use opal_core::keystore::SecretStore;
use opal_core::paths;

#[derive(Parser)]
#[command(version, about = "Opal daemon: Nostr signer and friends for Omarchy")]
struct Args {
    /// Control socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Config file path.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Database path.
    #[arg(long)]
    db: Option<PathBuf>,
    /// Keep keys in memory instead of the keyring (development only).
    #[arg(long, hide = true)]
    memory_keyring: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("OPAL_LOG")
                .unwrap_or_else(|_| "opald=info,opal_signer=info,opal_core=info".into()),
        )
        .with_target(false)
        .init();

    let args = Args::parse();
    let config = match &args.config {
        Some(p) => Config::load_from(p)?,
        None => Config::load()?,
    };
    let db = match &args.db {
        Some(p) => Db::open(p)?,
        None => Db::open_default()?,
    };
    let store = if args.memory_keyring {
        tracing::warn!("using an in-memory keyring: keys are lost on exit");
        SecretStore::memory()
    } else {
        SecretStore::keyring()
            .await
            .context("connecting to the Secret Service (is gnome-keyring running?)")?
    };

    let app = app::App::new(app::Options { config, db, store }).await?;
    // Relays can be slow to answer; the socket (and so the UI) comes up at once.
    let signer = app.signer.clone();
    tokio::spawn(async move {
        if let Err(e) = signer.start().await {
            tracing::error!("starting the signer failed: {e}");
        }
    });
    tasks::spawn_all(&app);
    modules::reconcile(&app).await;

    let socket = args.socket.unwrap_or_else(paths::socket_path);
    let server = ipc::serve(app.clone(), &socket);
    tokio::select! {
        r = server => r?,
        _ = shutdown_signal() => tracing::info!("shutting down"),
    }
    app.vault.lock().await;
    app.signer.shutdown().await;
    std::fs::remove_file(&socket).ok();
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
