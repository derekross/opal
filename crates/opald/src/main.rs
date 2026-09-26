//! opald — the Opal daemon.

mod api;
mod app;
mod ipc;
mod modules;
mod notify;
mod profiles;
mod signers;
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
    // Decrypted keys live in this process: no core dumps (which any process
    // of this user could read) and no /proc/<pid>/mem access from the same
    // user. The unit also sets LimitCORE=0.
    if let Err(e) =
        rustix::process::set_dumpable_behavior(rustix::process::DumpableBehavior::NotDumpable)
    {
        eprintln!("warning: could not disable core dumps: {e}");
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("OPAL_LOG")
                .unwrap_or_else(|_| "opald=info,opal_signer=info,opal_core=info".into()),
        )
        .with_target(false)
        .init();

    let args = Args::parse();
    opal_core::identity::ensure_crypto_provider();
    let config_path = args.config.clone().unwrap_or_else(paths::config_file);
    let config = Config::load_from(&config_path)?;
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

    let app = app::App::new(app::Options {
        config,
        config_path,
        db,
        store,
        vault_log_n: None,
    })
    .await?;
    tasks::spawn_all(&app);
    modules::reconcile(&app).await;
    modules::connect_bunker_in_background(&app);

    let socket = args.socket.unwrap_or_else(paths::socket_path);
    let server = opal_kit::ipc::serve(app.clone(), &socket, "opald");
    tokio::select! {
        r = server => r?,
        _ = shutdown_signal() => tracing::info!("shutting down"),
    }
    // Don't leave "now playing" up after we're gone (needs the key, so
    // before locking).
    if let Some((engine, _)) = app.status.lock().await.take() {
        engine.stop().await;
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

#[cfg(test)]
mod tests {
    /// nostr-sdk (websockets) and reqwest (NIP-05) both use rustls. If two
    /// crypto backends get compiled in, rustls can't pick one and every TLS
    /// connection panics at runtime. Keep it to exactly one.
    #[test]
    fn rustls_has_a_single_crypto_provider() {
        let _ = rustls::ClientConfig::builder();
    }
}
