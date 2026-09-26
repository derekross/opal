//! `opal` — command-line control for the Opal daemon.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use opal_core::ipc::{IpcMessage, IpcRequest};
use opal_core::paths;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(name = "opal", version, about = "Control the Opal Nostr signer")]
struct Cli {
    /// Control socket path.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print raw JSON.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show lock state, accounts and modules.
    Status,
    /// Unlock the signer (asks for the passphrase).
    Unlock,
    /// Lock the signer now.
    Lock,
    /// Manage accounts.
    #[command(subcommand)]
    Account(AccountCmd),
    /// Create a bunker:// URI for a new app.
    Bunker {
        #[arg(long)]
        name: Option<String>,
        /// basic, manual or full-trust.
        #[arg(long)]
        policy: Option<String>,
        /// Relay to use (repeatable). Defaults to the configured signer relays.
        #[arg(long = "relay")]
        relays: Vec<String>,
        /// Forget the URI if no app uses it within this many minutes.
        #[arg(long)]
        expires_min: Option<u64>,
        /// Also print a QR code.
        #[arg(long)]
        qr: bool,
    },
    /// Handle a nostrconnect:// URI. By default it is handed to the panel
    /// for approval; --accept connects right away.
    Connect {
        uri: String,
        #[arg(long)]
        accept: bool,
        /// With --accept: also allow what the app asked for.
        #[arg(long)]
        grant_requested: bool,
    },
    /// List connected apps, or show one.
    Apps { id: Option<String> },
    /// Disconnect and forget an app.
    Revoke { id: String },
    /// List requests waiting for a decision.
    Prompts,
    /// Approve a waiting request.
    Approve {
        id: String,
        /// once, 5m, 1h, 1d, 1w or always.
        #[arg(long, default_value = "once")]
        remember: String,
    },
    /// Deny a waiting request.
    Deny {
        id: String,
        #[arg(long, default_value = "once")]
        remember: String,
    },
    /// Show recent activity.
    Log {
        #[arg(long)]
        app: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: u32,
    },
    /// Watch someone's notifications without a key: an npub or a NIP-05
    /// address like derekross@grownostr.org.
    Watch { who: String },
    /// Stop watching someone (read-only mode).
    Unwatch,
    /// Set your NIP-38 status: `opal set-status "At the office" --for 4h`.
    SetStatus {
        text: String,
        #[arg(long)]
        link: Option<String>,
        /// How long: 30m, 4h, 2d. Default: until cleared.
        #[arg(long = "for")]
        duration: Option<String>,
    },
    /// Clear the status you set.
    ClearStatus,
    /// Show what's playing and recent listens.
    Plays {
        #[arg(long, default_value_t = 15)]
        limit: u32,
    },
    /// Show recent notifications (replies, mentions, reactions, zaps, DMs).
    Inbox {
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Mark everything as read afterwards.
        #[arg(long)]
        read: bool,
    },
    /// Turn a module on or off.
    Module {
        name: String,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Disconnect from all relays (off) or reconnect (on).
    Online {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Print daemon events as they happen (debugging).
    Events,
    /// Send a raw request: `opal raw <method> '<json params>'`.
    Raw {
        method: String,
        params: Option<String>,
    },
}

#[derive(Subcommand)]
enum AccountCmd {
    /// List accounts.
    List,
    /// Import a key (asks for it) or generate one with --generate.
    Add {
        #[arg(long)]
        generate: bool,
        #[arg(long)]
        nickname: Option<String>,
        /// NIP-06 account index for recovery phrases.
        #[arg(long)]
        account_index: Option<u32>,
    },
    /// Use this account for new connections.
    Select { pubkey: String },
    /// Set a nickname.
    Rename { pubkey: String, nickname: String },
    /// Print the account's ncryptsec backup (protected by the Opal passphrase).
    Export { pubkey: String },
    /// Delete an account and its app connections.
    Remove { pubkey: String },
}

struct Conn {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
}

impl Conn {
    async fn open(path: &PathBuf) -> Result<Self> {
        let stream = UnixStream::connect(path).await.with_context(|| {
            format!(
                "can't reach opald at {} (start it with `systemctl --user start opal`)",
                path.display()
            )
        })?;
        let (r, w) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(r),
            writer: w,
            next_id: 1,
        })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = IpcRequest {
            id,
            method: method.into(),
            params,
        };
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        loop {
            let msg = self.read().await?;
            if let IpcMessage::Response(r) = msg
                && r.id == id
            {
                return match r.error {
                    Some(e) => Err(anyhow!(e)),
                    None => Ok(r.result.unwrap_or(Value::Null)),
                };
            }
        }
    }

    async fn read(&mut self) -> Result<IpcMessage> {
        let mut line = String::new();
        if self.reader.read_line(&mut line).await? == 0 {
            bail!("opald closed the connection");
        }
        Ok(serde_json::from_str(&line)?)
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("opal: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.clone().unwrap_or_else(paths::socket_path);
    let mut c = Conn::open(&socket).await?;
    let out = |v: &Value| {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    };

    match cli.cmd {
        Cmd::Status => {
            let s = c.call("status", json!(null)).await?;
            if cli.json {
                out(&s);
            } else {
                print_status(&s);
            }
        }
        Cmd::Unlock => {
            let passphrase = read_secret("Opal passphrase: ")?;
            c.call("unlock", json!({"passphrase": passphrase})).await?;
            println!("Unlocked.");
        }
        Cmd::Lock => {
            c.call("lock", json!(null)).await?;
            println!("Locked.");
        }
        Cmd::Account(a) => account(&mut c, a, cli.json).await?,
        Cmd::Bunker {
            name,
            policy,
            relays,
            expires_min,
            qr,
        } => {
            let mut p = json!({"name": name, "policy": policy});
            if policy.as_deref() == Some("full-trust") {
                p["passphrase"] = json!(read_secret("Opal passphrase (needed for full trust): ")?);
            }
            if !relays.is_empty() {
                p["relays"] = json!(relays);
            }
            if let Some(m) = expires_min {
                p["unused_ttl_secs"] = json!(m * 60);
            }
            let r = c.call("apps.create_bunker", p).await?;
            let uri = r["uri"].as_str().unwrap_or_default();
            if cli.json {
                out(&r);
            } else {
                if qr {
                    print_qr(uri)?;
                }
                println!("{uri}");
                eprintln!(
                    "\nPaste this into the app's \"login with bunker\" field. It works once."
                );
            }
        }
        Cmd::Connect {
            uri,
            accept,
            grant_requested,
        } => {
            if accept {
                let parsed = c.call("nostrconnect.parse", json!({"uri": uri})).await?;
                let grant: Vec<Value> = if grant_requested {
                    parsed["perms"]
                        .as_array()
                        .map(|a| a.iter().map(|p| p["perm"].clone()).collect())
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                let r = c
                    .call("nostrconnect.accept", json!({"uri": uri, "grant": grant}))
                    .await?;
                println!("Connected {}", r["display_name"].as_str().unwrap_or("app"));
            } else {
                let r = c.call("nostrconnect.offer", json!({"uri": uri})).await?;
                let name = r["name"].as_str().unwrap_or("An app");
                println!("{name} wants to connect. Approve it in the Opal panel.");
            }
        }
        Cmd::Apps { id: None } => {
            let apps = c.call("apps.list", json!(null)).await?;
            if cli.json {
                out(&apps);
            } else {
                print_apps(&apps);
            }
        }
        Cmd::Apps { id: Some(id) } => out(&c.call("apps.get", json!({"id": id})).await?),
        Cmd::Revoke { id } => {
            let r = c.call("apps.remove", json!({"id": id})).await?;
            println!(
                "{}",
                if r["removed"] == json!(true) {
                    "Removed."
                } else {
                    "No such app."
                }
            );
        }
        Cmd::Prompts => {
            let p = c.call("prompts.list", json!(null)).await?;
            if cli.json {
                out(&p);
            } else {
                for pr in p.as_array().into_iter().flatten() {
                    println!(
                        "{}  {}  {} {}",
                        pr["id"].as_str().unwrap_or(""),
                        pr["app_name"].as_str().unwrap_or(""),
                        pr["method"].as_str().unwrap_or(""),
                        pr["kind_label"].as_str().unwrap_or(""),
                    );
                }
            }
        }
        Cmd::Approve { id, remember } => answer(&mut c, &id, true, &remember).await?,
        Cmd::Deny { id, remember } => answer(&mut c, &id, false, &remember).await?,
        Cmd::Log { app, limit } => {
            let l = c
                .call("activity.list", json!({"app_id": app, "limit": limit}))
                .await?;
            if cli.json {
                out(&l);
            } else {
                print_log(&l);
            }
        }
        Cmd::Watch { who } => {
            let r = c.call("identity.watch", json!({"input": who})).await?;
            println!(
                "Watching {}{}",
                r["nip05"]
                    .as_str()
                    .map(|n| format!("{n} · "))
                    .unwrap_or_default(),
                r["npub"].as_str().unwrap_or_default()
            );
        }
        Cmd::Unwatch => {
            c.call("identity.unwatch", json!(null)).await?;
            println!("Stopped watching.");
        }
        Cmd::SetStatus {
            text,
            link,
            duration,
        } => {
            let expires_in = duration.as_deref().map(parse_duration).transpose()?;
            c.call(
                "status.set",
                json!({"text": text, "link": link, "expires_in": expires_in}),
            )
            .await?;
            println!("Status set.");
        }
        Cmd::ClearStatus => {
            c.call("status.clear", json!(null)).await?;
            println!("Status cleared.");
        }
        Cmd::Plays { limit } => {
            let st = c.call("status.get", json!(null)).await?;
            if let Some(why) = st["blocked"].as_str() {
                println!("Status module: {why}");
            }
            let np = &st["snapshot"]["now_playing"];
            if np.is_object() {
                println!(
                    "Now playing: {} — {} ({}){}",
                    np["title"].as_str().unwrap_or(""),
                    np["artist"].as_str().unwrap_or(""),
                    np["player"].as_str().unwrap_or(""),
                    if st["snapshot"]["music"].is_string() {
                        ", shared"
                    } else {
                        ""
                    }
                );
            }
            let plays = c.call("scrobbles.recent", json!({"limit": limit})).await?;
            for p in plays.as_array().into_iter().flatten() {
                println!(
                    "  {}  {} — {}",
                    p["played_at"].as_u64().unwrap_or(0),
                    p["title"].as_str().unwrap_or(""),
                    p["artist"].as_str().unwrap_or("")
                );
            }
        }
        Cmd::Inbox { limit, read } => {
            let l = c
                .call("notifications.list", json!({"limit": limit}))
                .await?;
            if cli.json {
                out(&l);
            } else {
                let items = l.as_array().cloned().unwrap_or_default();
                if items.is_empty() {
                    println!(
                        "Nothing yet (is the notifications module on? `opal module notifications on`)"
                    );
                }
                for n in items {
                    let who = n["author_name"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| {
                            n["author"]
                                .as_str()
                                .unwrap_or("")
                                .chars()
                                .take(10)
                                .collect()
                        });
                    let what = match n["type"].as_str().unwrap_or("") {
                        "zap" => format!("zapped {} sats", n["sats"].as_u64().unwrap_or(0)),
                        "reaction" => format!("reacted {}", n["detail"].as_str().unwrap_or("")),
                        "dm" => "sent a message".into(),
                        t => t.to_string(),
                    };
                    println!(
                        "{} {:<20} {:<18} {}",
                        if n["unread"] == json!(true) {
                            "•"
                        } else {
                            " "
                        },
                        who.chars().take(20).collect::<String>(),
                        what,
                        n["detail"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(60)
                            .collect::<String>()
                    );
                }
            }
            if read {
                c.call("notifications.mark_read", json!(null)).await?;
            }
        }
        Cmd::Module { name, state } => {
            let r = c
                .call("config.set", json!({"modules": {name: state == "on"}}))
                .await?;
            out(&r["modules"]);
        }
        Cmd::Online { state } => {
            c.call("online.set", json!({"online": state == "on"}))
                .await?;
        }
        Cmd::Events => {
            c.call("subscribe", json!(null)).await?;
            loop {
                if let IpcMessage::Event(e) = c.read().await? {
                    println!("{}", serde_json::to_string(&e)?);
                }
            }
        }
        Cmd::Raw { method, params } => {
            let p = match params {
                Some(s) => serde_json::from_str(&s).context("params must be JSON")?,
                None => Value::Null,
            };
            out(&c.call(&method, p).await?);
        }
    }
    Ok(())
}

async fn answer(c: &mut Conn, id: &str, allow: bool, remember: &str) -> Result<()> {
    let r = c
        .call(
            "prompts.answer",
            json!({"id": id, "allow": allow, "remember": remember}),
        )
        .await?;
    if r["answered"] != json!(true) {
        bail!("that request is no longer waiting");
    }
    Ok(())
}

async fn account(c: &mut Conn, cmd: AccountCmd, raw: bool) -> Result<()> {
    match cmd {
        AccountCmd::List => {
            let s = c.call("status", json!(null)).await?;
            if raw {
                println!("{}", serde_json::to_string_pretty(&s["accounts"])?);
            } else {
                print_accounts(&s);
            }
        }
        AccountCmd::Add {
            generate,
            nickname,
            account_index,
        } => {
            let status = c.call("status", json!(null)).await?;
            let first = status["has_accounts"] != json!(true);
            let secret = if generate {
                None
            } else {
                Some(read_secret(
                    "Key (nsec, hex, ncryptsec or recovery phrase): ",
                )?)
            };
            let mut params = json!({"nickname": nickname, "account_index": account_index});
            if let Some(s) = &secret {
                if s.trim().starts_with("ncryptsec1") {
                    params["ncryptsec_password"] =
                        json!(read_secret("Password for that ncryptsec: ")?);
                }
                params["secret"] = json!(s);
            }
            let passphrase = if first {
                println!("Choose the Opal passphrase. It unlocks every account.");
                let a = read_secret("New passphrase: ")?;
                let b = read_secret("Repeat: ")?;
                if a != b {
                    bail!("passphrases don't match");
                }
                a
            } else {
                read_secret("Opal passphrase: ")?
            };
            params["passphrase"] = json!(passphrase);
            let r = c.call("accounts.add", params).await?;
            println!("Added {}", r["npub"].as_str().unwrap_or_default());
        }
        AccountCmd::Select { pubkey } => {
            c.call("accounts.select", json!({"pubkey": pubkey})).await?;
        }
        AccountCmd::Rename { pubkey, nickname } => {
            c.call(
                "accounts.rename",
                json!({"pubkey": pubkey, "nickname": nickname}),
            )
            .await?;
        }
        AccountCmd::Export { pubkey } => {
            let r = c.call("accounts.export", json!({"pubkey": pubkey})).await?;
            println!("{}", r["ncryptsec"].as_str().unwrap_or_default());
            eprintln!("Encrypted with your Opal passphrase (NIP-49). Keep it private anyway.");
        }
        AccountCmd::Remove { pubkey } => {
            if std::io::stdin().is_terminal() {
                eprint!("Delete this account and disconnect its apps? Type yes: ");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if answer.trim() != "yes" {
                    bail!("cancelled");
                }
            }
            let passphrase = read_secret("Opal passphrase: ")?;
            c.call(
                "accounts.remove",
                json!({"pubkey": pubkey, "passphrase": passphrase}),
            )
            .await?;
            println!("Removed.");
        }
    }
    Ok(())
}

/// "30m", "4h", "2d" → seconds.
fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num.parse().context("durations look like 30m, 4h or 2d")?;
    Ok(match unit {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => bail!("durations look like 30m, 4h or 2d"),
    })
}

/// Read a secret without echo on a terminal, or one line from stdin when
/// piped (so several prompts can be answered from one pipe).
fn read_secret(prompt: &str) -> Result<String> {
    if std::io::stdin().is_terminal() {
        Ok(rpassword::prompt_password(prompt)?)
    } else {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        Ok(s.trim_end_matches(['\n', '\r']).to_string())
    }
}

fn print_status(s: &Value) {
    let locked = s["locked"] == json!(true);
    println!(
        "Signer:   {}{}",
        if locked { "locked" } else { "unlocked" },
        if s["online"] == json!(false) {
            " (offline)"
        } else {
            ""
        }
    );
    let pending = s["pending_prompts"].as_u64().unwrap_or(0);
    if pending > 0 {
        println!("Waiting:  {pending} request(s) — see `opal prompts`");
    }
    print_accounts(s);
    let m = &s["modules"];
    let on = |k: &str| if m[k] == json!(true) { "on" } else { "off" };
    println!(
        "Modules:  signer {}, notifications {}, status {}",
        on("signer"),
        on("notifications"),
        on("status")
    );
}

fn print_accounts(s: &Value) {
    let accounts = s["accounts"].as_array().cloned().unwrap_or_default();
    if accounts.is_empty() {
        println!("Accounts: none — add one with `opal account add`");
        return;
    }
    println!("Accounts:");
    for a in accounts {
        println!(
            "  {} {}  {}",
            if a["current"] == json!(true) {
                "*"
            } else {
                " "
            },
            a["label"].as_str().unwrap_or(""),
            a["npub"].as_str().unwrap_or(""),
        );
    }
}

fn print_apps(apps: &Value) {
    let list = apps.as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        println!("No apps yet. Create a login with `opal bunker`.");
        return;
    }
    for a in list {
        println!(
            "{}  {:<24} {:<10} {}",
            a["id"].as_str().unwrap_or(""),
            a["display_name"].as_str().unwrap_or(""),
            a["policy"].as_str().unwrap_or(""),
            if a["kind"] == json!("local") {
                "local app"
            } else if a["connected"] == json!(true) {
                "connected"
            } else {
                "waiting for app"
            },
        );
    }
}

fn print_log(l: &Value) {
    for e in l.as_array().into_iter().flatten() {
        let what = e["kind_label"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| e["method"].as_str().unwrap_or("").to_string());
        println!(
            "{}  {:<20} {:<28} {:<8} {}",
            e["at"].as_u64().unwrap_or(0),
            e["app_name"].as_str().unwrap_or(""),
            what,
            if e["allowed"] == json!(true) {
                "allowed"
            } else {
                "denied"
            },
            e["source"].as_str().unwrap_or(""),
        );
    }
}

fn print_qr(data: &str) -> Result<()> {
    let code = qrcode::QrCode::new(data.as_bytes())?;
    let s = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build();
    println!("{s}");
    Ok(())
}
