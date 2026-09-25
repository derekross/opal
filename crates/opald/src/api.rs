//! Request handlers for the control socket.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use nostr_sdk::prelude::{Keys, PublicKey, Timestamp, ToBech32};
use opal_core::config::{Config, Policy};
use opal_core::import::{ImportOptions, parse_secret};
use opal_signer::kinds;
use opal_signer::permissions::{Remember, Rule, expand_perms};
use opal_signer::protocol::Method;
use opal_signer::store::ActivityQuery;
use opal_signer::{NostrConnectUri, PromptAnswer};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::app::{App, parse_relays};
use crate::profiles;

/// Handle one request. `Err` becomes the response's `error` string.
pub async fn dispatch(app: &Arc<App>, method: &str, params: Value) -> Result<Value> {
    match method {
        "ping" => Ok(json!("pong")),
        "status" => Ok(app.status().await),

        // ── Lock ────────────────────────────────────────────────────────
        "unlock" => {
            let p: Passphrase = parse(params)?;
            app.vault.unlock(&p.passphrase).await?;
            app.touch().await;
            app.emit_state().await;
            Ok(json!({"locked": false}))
        }
        "lock" => {
            app.lock("requested").await;
            Ok(json!({"locked": true}))
        }
        "passphrase.change" => {
            #[derive(Deserialize)]
            struct P {
                old: String,
                new: String,
            }
            let p: P = parse(params)?;
            check_passphrase_strength(&p.new)?;
            app.vault.change_passphrase(&p.old, &p.new).await?;
            Ok(json!({"ok": true}))
        }

        // ── Accounts ────────────────────────────────────────────────────
        "accounts.list" => Ok(app.status().await["accounts"].clone()),
        "accounts.add" => accounts_add(app, params).await,
        "accounts.remove" => {
            let p: Pk = parse(params)?;
            let pk = p.pubkey()?;
            for c in app.signer.connections().await {
                if c.account == pk.to_hex() {
                    app.signer.remove_connection(&c.id).await?;
                }
            }
            app.vault.remove_account(&pk).await?;
            app.accounts.remove(&pk)?;
            crate::modules::reconcile(app).await;
            app.emit_state().await;
            Ok(json!({"ok": true}))
        }
        "accounts.select" => {
            let p: Pk = parse(params)?;
            let pk = p.pubkey()?;
            if app.accounts.get(&pk)?.is_none() {
                bail!("unknown account");
            }
            app.accounts.set_current(&pk)?;
            crate::modules::reconcile(app).await;
            app.emit_state().await;
            Ok(json!({"ok": true}))
        }
        "accounts.rename" => {
            #[derive(Deserialize)]
            struct P {
                pubkey: String,
                nickname: Option<String>,
            }
            let p: P = parse(params)?;
            let pk = PublicKey::parse(&p.pubkey)?;
            app.accounts.set_nickname(&pk, p.nickname.as_deref())?;
            app.emit_state().await;
            Ok(json!({"ok": true}))
        }
        "accounts.export" => {
            let p: Pk = parse(params)?;
            let ncryptsec = app.vault.export_ncryptsec(&p.pubkey()?).await?;
            Ok(json!({"ncryptsec": ncryptsec.as_str()}))
        }
        "accounts.refresh_profiles" => {
            let app = app.clone();
            tokio::spawn(async move { profiles::refresh_all(&app).await });
            Ok(json!({"ok": true}))
        }

        // ── Apps ────────────────────────────────────────────────────────
        "apps.list" => Ok(json!(app.signer.connections().await)),
        "apps.get" => {
            let p: Id = parse(params)?;
            let info = app
                .signer
                .connection(&p.id)
                .await
                .ok_or(anyhow!("unknown app"))?;
            let rules = rules_json(app, &p.id)?;
            Ok(json!({"app": info, "rules": rules}))
        }
        "apps.create_bunker" => {
            require_signer(app).await?;
            #[derive(Deserialize, Default)]
            struct P {
                account: Option<String>,
                name: Option<String>,
                relays: Option<Vec<String>>,
                policy: Option<Policy>,
                unused_ttl_secs: Option<u64>,
            }
            let p: P = parse_or_default(params)?;
            let account = account_or_current(app, p.account.as_deref())?;
            let policy = match p.policy {
                Some(pol) => pol,
                None => app.config.read().await.signer.default_policy,
            };
            let (info, uri) = app
                .signer
                .create_bunker(
                    account,
                    p.name,
                    p.relays.map(|r| parse_relays(&r)),
                    policy,
                    p.unused_ttl_secs.map(Duration::from_secs),
                )
                .await?;
            Ok(json!({"app": info, "uri": uri}))
        }
        "apps.update" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                name: Option<String>,
                policy: Option<Policy>,
                relays: Option<Vec<String>>,
            }
            let p: P = parse(params)?;
            let info = app
                .signer
                .update_connection(&p.id, p.name, p.policy, p.relays.map(|r| parse_relays(&r)))
                .await?;
            Ok(json!(info))
        }
        "apps.remove" => {
            let p: Id = parse(params)?;
            Ok(json!({"removed": app.signer.remove_connection(&p.id).await?}))
        }
        "apps.set_rule" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                method: String,
                kind: Option<u16>,
                allow: bool,
                remember: Option<Remember>,
            }
            let p: P = parse(params)?;
            let now = Timestamp::now().as_secs();
            let Some(until) = p.remember.unwrap_or(Remember::Always).until(now) else {
                bail!("a saved rule needs a duration other than \"once\"");
            };
            store(app)?.put_rule(&Rule {
                app_id: p.id.clone(),
                method: Method::from(p.method.as_str()),
                kind: p.kind,
                allow: p.allow,
                until,
                created_at: now,
            })?;
            Ok(json!(rules_json(app, &p.id)?))
        }
        "apps.delete_rule" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                method: String,
                kind: Option<u16>,
            }
            let p: P = parse(params)?;
            store(app)?.delete_rule(&p.id, &Method::from(p.method.as_str()), p.kind)?;
            Ok(json!(rules_json(app, &p.id)?))
        }
        "apps.clear_rules" => {
            let p: Id = parse(params)?;
            store(app)?.clear_rules(&p.id)?;
            Ok(json!([]))
        }

        // ── nostrconnect:// ─────────────────────────────────────────────
        "nostrconnect.parse" => {
            let p: Uri = parse(params)?;
            Ok(describe_nostrconnect(&NostrConnectUri::parse(&p.uri)?))
        }
        "nostrconnect.offer" => {
            require_signer(app).await?;
            // Hand a URI to the UI (used by the xdg handler / CLI).
            let p: Uri = parse(params)?;
            let parsed = NostrConnectUri::parse(&p.uri)?;
            let id = parsed.client.to_hex();
            let mut data = describe_nostrconnect(&parsed);
            data["id"] = json!(id);
            app.offers.lock().await.insert(id, parsed);
            app.emit("nostrconnect_offer", data.clone());
            Ok(data)
        }
        "nostrconnect.offers" => {
            let offers = app.offers.lock().await;
            Ok(json!(
                offers
                    .iter()
                    .map(|(id, u)| {
                        let mut d = describe_nostrconnect(u);
                        d["id"] = json!(id);
                        d
                    })
                    .collect::<Vec<_>>()
            ))
        }
        "nostrconnect.accept" => {
            require_signer(app).await?;
            #[derive(Deserialize)]
            struct P {
                /// Either the URI itself or the id of an offer.
                uri: Option<String>,
                offer_id: Option<String>,
                account: Option<String>,
                policy: Option<Policy>,
                /// `method` or `method:kind` strings to allow from now on.
                #[serde(default)]
                grant: Vec<String>,
            }
            let p: P = parse(params)?;
            let uri = match (&p.uri, &p.offer_id) {
                (Some(u), _) => NostrConnectUri::parse(u)?,
                (None, Some(id)) => app
                    .offers
                    .lock()
                    .await
                    .remove(id)
                    .ok_or(anyhow!("that request is gone"))?,
                (None, None) => bail!("uri or offer_id required"),
            };
            let account = account_or_current(app, p.account.as_deref())?;
            let policy = match p.policy {
                Some(pol) => pol,
                None => app.config.read().await.signer.default_policy,
            };
            let grant = expand_perms(&opal_signer::perms::parse_perms(&p.grant.join(",")));
            let info = app
                .signer
                .accept_nostrconnect(&uri, account, policy, &grant)
                .await?;
            Ok(json!(info))
        }
        "nostrconnect.reject" => {
            let p: Id = parse(params)?;
            Ok(json!({"removed": app.offers.lock().await.remove(&p.id).is_some()}))
        }

        // ── Prompts ─────────────────────────────────────────────────────
        "prompts.list" => Ok(json!(app.prompts.pending())),
        "prompts.answer" => {
            #[derive(Deserialize)]
            struct P {
                id: String,
                allow: bool,
                #[serde(default = "once")]
                remember: Remember,
            }
            let p: P = parse(params)?;
            let ok = app.prompts.answer(
                &p.id,
                PromptAnswer {
                    allow: p.allow,
                    remember: p.remember,
                },
            );
            Ok(json!({"answered": ok}))
        }
        "prompts.dismiss" => {
            let p: Id = parse(params)?;
            Ok(json!({"dismissed": app.prompts.dismiss(&p.id)}))
        }

        // ── Activity ────────────────────────────────────────────────────
        "activity.list" => {
            #[derive(Deserialize, Default)]
            struct P {
                app_id: Option<String>,
                allowed: Option<bool>,
                before_id: Option<i64>,
                limit: Option<u32>,
            }
            let p: P = parse_or_default(params)?;
            let q = ActivityQuery {
                app_id: p.app_id,
                allowed: p.allowed,
                before_id: p.before_id,
                limit: p.limit.unwrap_or(100),
            };
            Ok(json!(store(app)?.activity(&q)?))
        }
        "activity.stats" => {
            let now = Timestamp::now().as_secs();
            let s = store(app)?;
            Ok(json!({
                "day": s.activity_stats(now.saturating_sub(86_400))?,
                "week": s.activity_stats(now.saturating_sub(7 * 86_400))?,
            }))
        }
        "activity.clear" => {
            store(app)?.clear_activity()?;
            Ok(json!({"ok": true}))
        }

        // ── Settings ────────────────────────────────────────────────────
        "config.get" => Ok(json!(*app.config.read().await)),
        "config.set" => {
            // Merge a partial config into the current one.
            let mut current = serde_json::to_value(&*app.config.read().await)?;
            merge(&mut current, params);
            let new: Config = serde_json::from_value(current).context("invalid settings")?;
            new.save_to(&app.config_path)?;
            *app.config.write().await = new.clone();
            crate::modules::reconcile(app).await;
            app.emit_state().await;
            Ok(json!(new))
        }
        "online.set" => {
            #[derive(Deserialize)]
            struct P {
                online: bool,
            }
            let p: P = parse(params)?;
            app.set_online(p.online).await;
            Ok(json!({"online": p.online}))
        }

        "qr.svg" => {
            // Render a QR code (e.g. a bunker URI) to a private file the panel
            // can show. The URI's secret is single-use, so the file is too.
            #[derive(Deserialize)]
            struct P {
                data: String,
            }
            let p: P = parse(params)?;
            let code = qrcode::QrCode::new(p.data.as_bytes())?;
            let svg = code
                .render::<qrcode::render::svg::Color>()
                .min_dimensions(256, 256)
                .quiet_zone(true)
                .build();
            let path = opal_core::paths::socket_path().with_file_name("opal-qr.svg");
            write_private(&path, svg.as_bytes())?;
            Ok(json!({"path": path}))
        }
        // ── Identity ────────────────────────────────────────────────────
        "identity.resolve" => {
            // npub, nprofile, hex, or a NIP-05 address.
            let p: Input = parse(params)?;
            Ok(json!(opal_core::identity::resolve(&p.input).await?))
        }
        "identity.watch" => {
            // Watch someone (read-only): resolve, save, turn notifications on.
            let p: Input = parse(params)?;
            let r = opal_core::identity::resolve(&p.input).await?;
            {
                let mut cfg = app.config.write().await;
                cfg.identity.mode = opal_core::config::IdentityMode::ReadOnly;
                cfg.identity.npub = Some(r.npub.clone());
                cfg.identity.nip05 = r.nip05.clone();
                cfg.modules.notifications = true;
                cfg.save_to(&app.config_path)?;
            }
            crate::modules::reconcile(app).await;
            app.emit_state().await;
            Ok(json!(r))
        }

        "identity.external" => {
            // Sign statuses through a bunker (e.g. Amber). Waits for approval.
            let p: Uri = parse(params)?;
            let (bunker, pk) = crate::signers::BunkerSigner::connect(&app.vault, &p.uri).await?;
            *app.bunker.lock().await = Some(Arc::new(bunker));
            {
                let mut cfg = app.config.write().await;
                cfg.identity.mode = opal_core::config::IdentityMode::External;
                cfg.identity.bunker_uri = Some(p.uri.trim().to_string());
                cfg.identity.npub = Some(pk.to_bech32()?);
                cfg.identity.nip05 = None;
                cfg.save_to(&app.config_path)?;
            }
            crate::modules::reconcile(app).await;
            app.emit_state().await;
            Ok(json!({"pubkey": pk.to_hex(), "npub": pk.to_bech32()?}))
        }
        "identity.unwatch" => {
            // Stop watching someone (read-only mode) and forget them.
            {
                let mut cfg = app.config.write().await;
                if cfg.identity.mode == opal_core::config::IdentityMode::ReadOnly {
                    cfg.identity.mode = opal_core::config::IdentityMode::Local;
                    // Without a key there's nothing left to show notifications for.
                    if app.vault.accounts().await?.is_empty() {
                        cfg.modules.notifications = false;
                    }
                }
                cfg.identity.npub = None;
                cfg.identity.nip05 = None;
                cfg.save_to(&app.config_path)?;
            }
            crate::modules::reconcile(app).await;
            app.emit_state().await;
            Ok(json!({"ok": true}))
        }
        "identity.local" => {
            {
                let mut cfg = app.config.write().await;
                cfg.identity.mode = opal_core::config::IdentityMode::Local;
                cfg.save_to(&app.config_path)?;
            }
            *app.bunker.lock().await = None;
            crate::modules::reconcile(app).await;
            app.emit_state().await;
            Ok(json!({"ok": true}))
        }

        // ── Status & scrobbles ──────────────────────────────────────────
        "status.get" => {
            let enabled = app.config.read().await.modules.status;
            let guard = app.status.lock().await;
            let (snapshot, account) = match guard.as_ref() {
                Some((engine, _)) => (
                    Some(engine.snapshot().await),
                    Some(engine.account().to_hex()),
                ),
                None => (None, None),
            };
            drop(guard);
            Ok(json!({
                "enabled": enabled,
                "running": snapshot.is_some(),
                "blocked": *app.status_blocked.lock().await,
                "snapshot": snapshot,
                "manual": app.status_store.manual()?,
                "account": account,
            }))
        }
        "status.set" => {
            #[derive(Deserialize)]
            struct P {
                text: String,
                link: Option<String>,
                /// Seconds from now; omit to keep it until cleared.
                expires_in: Option<u64>,
            }
            let p: P = parse(params)?;
            if p.text.trim().is_empty() {
                bail!("write something, or use status.clear");
            }
            if let Some(l) = &p.link
                && !l.is_empty()
                && !l.starts_with("https://")
            {
                bail!("links must start with https://");
            }
            let manual = opal_status::general::Manual {
                text: p.text.chars().take(280).collect(),
                link: p.link.filter(|l| !l.is_empty()),
                expires_at: p.expires_in.map(|s| Timestamp::now().as_secs() + s),
            };
            set_manual(app, Some(manual)).await
        }
        "status.clear" => set_manual(app, None).await,
        "scrobbles.recent" => {
            #[derive(Deserialize, Default)]
            struct P {
                limit: Option<u32>,
            }
            let p: P = parse_or_default(params)?;
            let Some(account) = status_account(app).await else {
                return Ok(json!([]));
            };
            Ok(json!(
                app.status_store.recent(&account, p.limit.unwrap_or(50))?
            ))
        }
        "scrobbles.stats" => {
            let Some(account) = status_account(app).await else {
                return Ok(json!({}));
            };
            let now = Timestamp::now().as_secs();
            let week = now.saturating_sub(7 * 86_400);
            let month = now.saturating_sub(30 * 86_400);
            Ok(json!({
                "today": app.status_store.play_count(&account, now.saturating_sub(86_400))?,
                "week": app.status_store.play_count(&account, week)?,
                "top_week": app.status_store.top_artists(&account, week, 5)?,
                "top_month": app.status_store.top_artists(&account, month, 10)?,
            }))
        }
        "scrobbles.clear" => {
            if let Some(account) = status_account(app).await {
                app.status_store.clear_plays(&account)?;
            }
            Ok(json!({"ok": true}))
        }

        // ── Notifications ───────────────────────────────────────────────
        "notifications.status" => {
            let enabled = app.config.read().await.modules.notifications;
            let guard = app.notify.lock().await;
            let status = match guard.as_ref() {
                Some((engine, _)) => Some(engine.status().await),
                None => None,
            };
            drop(guard);
            Ok(json!({
                "enabled": enabled,
                "running": status.is_some(),
                "status": status,
                "unread": app.unread_notifications().await,
                "clients": opal_notify::links::CLIENTS
                    .iter()
                    .map(|(id, label, _)| json!({"value": id, "label": label}))
                    .collect::<Vec<_>>(),
            }))
        }
        "notifications.list" => {
            #[derive(Deserialize, Default)]
            struct P {
                limit: Option<u32>,
                before: Option<u64>,
            }
            let p: P = parse_or_default(params)?;
            let Some(account) = notify_account(app).await else {
                return Ok(json!([]));
            };
            let client = app.config.read().await.notifications.client.clone();
            let list = app
                .notify_store
                .list(&account, p.limit.unwrap_or(100), p.before)?;
            Ok(json!(
                list.into_iter()
                    .map(|n| {
                        let url = notification_url(&client, &n);
                        let mut v = serde_json::to_value(&n).unwrap_or_default();
                        v["url"] = json!(url);
                        v
                    })
                    .collect::<Vec<_>>()
            ))
        }
        "notifications.mark_read" => {
            if let Some(account) = notify_account(app).await {
                app.notify_store
                    .mark_read(&account, Timestamp::now().as_secs())?;
            }
            app.emit("unread", json!({"count": 0}));
            app.emit_state().await;
            Ok(json!({"ok": true}))
        }
        "notifications.clear" => {
            if let Some(account) = notify_account(app).await {
                app.notify_store.clear(&account)?;
            }
            app.emit_state().await;
            Ok(json!({"ok": true}))
        }
        "notifications.open" => {
            let p: Id = parse(params)?;
            let account = notify_account(app)
                .await
                .ok_or(anyhow!("notifications are off"))?;
            let n = app
                .notify_store
                .get(&account, &p.id)?
                .ok_or(anyhow!("no such notification"))?;
            let client = app.config.read().await.notifications.client.clone();
            let url = notification_url(&client, &n).ok_or(anyhow!("nothing to open"))?;
            tokio::process::Command::new("xdg-open").arg(&url).spawn()?;
            Ok(json!({"url": url}))
        }
        "notifications.block" => {
            let p: Pk = parse(params)?;
            let hex = p.pubkey()?.to_hex();
            {
                let mut cfg = app.config.write().await;
                if !cfg.notifications.blocked.contains(&hex) {
                    cfg.notifications.blocked.push(hex);
                }
                cfg.save_to(&app.config_path)?;
            }
            crate::modules::reconcile(app).await;
            Ok(json!({"ok": true}))
        }

        "kinds.label" => {
            #[derive(Deserialize)]
            struct P {
                kind: u16,
            }
            let p: P = parse(params)?;
            Ok(json!({"label": kinds::label(p.kind), "nip": kinds::nip_of(p.kind)}))
        }

        other => bail!("unknown method: {other}"),
    }
}

async fn accounts_add(app: &Arc<App>, params: Value) -> Result<Value> {
    #[derive(Deserialize)]
    struct P {
        /// nsec, hex, ncryptsec or recovery phrase; omit to generate a key.
        secret: Option<String>,
        /// The Opal passphrase (sets it if this is the first account).
        passphrase: String,
        nickname: Option<String>,
        ncryptsec_password: Option<String>,
        account_index: Option<u32>,
        mnemonic_passphrase: Option<String>,
    }
    let p: P = parse(params)?;
    let first = app.vault.accounts().await?.is_empty();
    if first {
        check_passphrase_strength(&p.passphrase)?;
    }
    let keys = match p.secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => Keys::generate(),
        Some(secret) => parse_secret(
            secret,
            ImportOptions {
                ncryptsec_password: p.ncryptsec_password.as_deref(),
                account_index: p.account_index,
                mnemonic_passphrase: p.mnemonic_passphrase.as_deref(),
            },
        )?,
    };
    let pk = app.vault.add_account(keys, &p.passphrase).await?;
    app.accounts
        .add(&pk, p.nickname.as_deref(), Timestamp::now().as_secs())?;
    if first && !app.vault.is_unlocked() {
        // Setting up the first account leaves the signer ready to use.
        app.vault.unlock(&p.passphrase).await?;
        app.touch().await;
    }
    crate::modules::reconcile(app).await;
    app.emit_state().await;
    let app2 = app.clone();
    tokio::spawn(async move { profiles::refresh(&app2, &[pk]).await });
    Ok(json!({"pubkey": pk.to_hex(), "npub": pk.to_bech32()?}))
}

fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(data)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

async fn require_signer(app: &App) -> Result<()> {
    if app.config.read().await.modules.signer {
        Ok(())
    } else {
        bail!("the signer module is off (turn it on in Settings)")
    }
}

async fn status_account(app: &App) -> Option<String> {
    app.status
        .lock()
        .await
        .as_ref()
        .map(|(engine, _)| engine.account().to_hex())
}

/// Set or clear the manual status (running module, or just saved for later).
async fn set_manual(app: &Arc<App>, m: Option<opal_status::general::Manual>) -> Result<Value> {
    let guard = app.status.lock().await;
    match guard.as_ref() {
        Some((engine, _)) => engine.set_manual(m.clone()).await,
        None => app.status_store.set_manual(m.as_ref())?,
    }
    drop(guard);
    Ok(json!({"manual": m}))
}

async fn notify_account(app: &App) -> Option<String> {
    app.notify
        .lock()
        .await
        .as_ref()
        .map(|(engine, _)| engine.account().to_hex())
}

/// Where clicking a notification goes: the note itself for replies and
/// mentions, the note it is about for reactions, reposts and zaps.
fn notification_url(client: &str, n: &opal_notify::StoredNotification) -> Option<String> {
    use opal_notify::NotifType;
    let n2 = &n.n;
    match (n2.ntype, &n2.ref_id) {
        (NotifType::Dm, _) => None,
        (NotifType::Reply | NotifType::Mention, _) | (_, None) => opal_notify::links::event_url(
            client,
            &n2.id,
            Some(&n2.author),
            Some(n2.kind),
            n2.relay.as_deref(),
        ),
        (_, Some(r)) => opal_notify::links::event_url(
            client,
            r,
            n.ref_author.as_deref(),
            n.ref_kind,
            n2.relay.as_deref(),
        ),
    }
}

fn check_passphrase_strength(p: &str) -> Result<()> {
    if p.chars().count() < 8 {
        bail!("use a passphrase of at least 8 characters");
    }
    Ok(())
}

fn account_or_current(app: &App, given: Option<&str>) -> Result<PublicKey> {
    match given {
        Some(s) => Ok(PublicKey::parse(s)?),
        None => app
            .current_account()?
            .ok_or(anyhow!("add an account first")),
    }
}

fn store(app: &App) -> Result<&opal_signer::SignerStore> {
    app.signer.store().ok_or(anyhow!("no signer store"))
}

fn rules_json(app: &App, id: &str) -> Result<Value> {
    let rules = store(app)?.rules(id)?;
    Ok(json!(
        rules
            .into_iter()
            .map(|r| {
                let label = r.kind.map(kinds::label);
                let mut v = serde_json::to_value(&r).unwrap_or_default();
                v["kind_label"] = json!(label);
                v
            })
            .collect::<Vec<_>>()
    ))
}

fn describe_nostrconnect(u: &NostrConnectUri) -> Value {
    let perms: Vec<Value> = expand_perms(&u.perms)
        .into_iter()
        .map(|(m, k)| {
            json!({
                "perm": match k { Some(k) => format!("{m}:{k}"), None => m.to_string() },
                "method": m,
                "kind": k,
                "label": k.map(kinds::label),
            })
        })
        .collect();
    json!({
        "client": u.client.to_hex(),
        "name": u.name,
        "url": u.url,
        "image": u.image,
        "relays": u.relays.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
        "perms": perms,
    })
}

fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p,
    }
}

fn once() -> Remember {
    Remember::Once
}

#[derive(Deserialize)]
struct Passphrase {
    passphrase: String,
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
struct Input {
    input: String,
}

#[derive(Deserialize)]
struct Uri {
    uri: String,
}

#[derive(Deserialize)]
struct Pk {
    pubkey: String,
}

impl Pk {
    fn pubkey(&self) -> Result<PublicKey> {
        Ok(PublicKey::parse(&self.pubkey)?)
    }
}

fn parse<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| anyhow!("bad params: {e}"))
}

fn parse_or_default<T: for<'de> Deserialize<'de> + Default>(v: Value) -> Result<T> {
    if v.is_null() {
        Ok(T::default())
    } else {
        parse(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_is_deep() {
        let mut base = json!({"modules": {"signer": true, "status": false}, "x": 1});
        merge(&mut base, json!({"modules": {"status": true}}));
        assert_eq!(
            base,
            json!({"modules": {"signer": true, "status": true}, "x": 1})
        );
    }
}
