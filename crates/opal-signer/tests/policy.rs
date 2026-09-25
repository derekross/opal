//! Rules, prompts, activity and persistence, end to end.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nostr_connect::client::NostrConnect;
use nostr_sdk::prelude::*;
use opal_core::config::Policy;
use opal_core::db::Db;
use opal_core::keystore::SecretStore;
use opal_core::vault::Vault;
use opal_signer::permissions::{Remember, Source};
use opal_signer::protocol::Method;
use opal_signer::store::ActivityQuery;
use opal_signer::{
    PolicyApprover, PromptAnswer, PromptEvent, PromptHub, Signer, SignerSettings, SignerStore,
};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Env {
    _relay: MockRelay,
    relay_url: RelayUrl,
    vault: Arc<Vault>,
    db: Db,
    account: PublicKey,
    prompts: Arc<PromptHub>,
}

async fn env() -> Env {
    let relay = MockRelay::run().await.unwrap();
    let relay_url = relay.url().await;
    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    let account = vault.add_account(Keys::generate(), "pw").await.unwrap();
    vault.unlock("pw").await.unwrap();
    Env {
        _relay: relay,
        relay_url,
        vault,
        db: Db::open_in_memory().unwrap(),
        account,
        prompts: Arc::new(PromptHub::default()),
    }
}

async fn signer(e: &Env) -> Signer {
    let store = SignerStore::new(e.db.clone()).unwrap();
    let approver = Arc::new(PolicyApprover::new(store.clone(), e.prompts.clone()));
    let s = Signer::with_store(
        e.vault.clone(),
        approver,
        SignerSettings {
            default_relays: vec![e.relay_url.clone()],
            pending_timeout: TIMEOUT,
            log_activity: true,
        },
        store,
    )
    .await
    .unwrap();
    s.start().await.unwrap();
    s
}

/// Answer every prompt with `answer`, counting them.
fn auto_answer(prompts: Arc<PromptHub>, answer: PromptAnswer) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let mut rx = prompts.subscribe();
    tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            if let PromptEvent::Opened { prompt } = ev {
                c.fetch_add(1, Ordering::SeqCst);
                prompts.answer(&prompt.id, answer);
            }
        }
    });
    count
}

fn client(uri: &str) -> NostrConnect {
    let parsed = nostr_connect::prelude::NostrConnectUri::parse(uri).unwrap();
    NostrConnect::new(parsed, Keys::generate(), TIMEOUT, None).unwrap()
}

async fn sign(app: &NostrConnect, kind: u16) -> std::result::Result<Event, String> {
    EventBuilder::new(Kind::from(kind), "x")
        .finalize_async(app)
        .await
        .map_err(|e| e.to_string())
}

#[tokio::test]
async fn basic_policy_prompts_and_remembers() {
    let e = env().await;
    let s = signer(&e).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: true,
            remember: Remember::OneHour,
        },
    );
    let (info, uri) = s
        .create_bunker(e.account, Some("App".into()), None, Policy::Basic, None)
        .await
        .unwrap();
    let app = client(&uri);

    // get_public_key and kind 1 are on the Basic list: no prompt.
    sign(&app, 1).await.unwrap();
    assert_eq!(prompts.load(Ordering::SeqCst), 0);

    // Profile updates ask once, then the 1h rule covers the next one.
    sign(&app, 0).await.unwrap();
    sign(&app, 0).await.unwrap();
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
    let rules = s.store().unwrap().rules(&info.id).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!((rules[0].kind, rules[0].allow), (Some(0), true));
    assert!(rules[0].until.is_some());

    let log = s
        .store()
        .unwrap()
        .activity(&ActivityQuery::default())
        .unwrap();
    let sources: Vec<_> = log
        .iter()
        .rev()
        .map(|a| (a.method.clone(), a.source))
        .collect();
    assert_eq!(
        sources,
        vec![
            (Method::GetPublicKey, Source::BasicPolicy),
            (Method::SignEvent, Source::BasicPolicy),
            (Method::SignEvent, Source::User),
            (Method::SignEvent, Source::Rule),
        ]
    );
    assert_eq!(log[0].kind_label.as_deref(), Some("Profile metadata"));
}

#[tokio::test]
async fn remembered_denial_blocks_without_prompting() {
    let e = env().await;
    let s = signer(&e).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: false,
            remember: Remember::Always,
        },
    );
    let (_, uri) = s
        .create_bunker(e.account, None, None, Policy::Basic, None)
        .await
        .unwrap();
    let app = client(&uri);
    assert!(sign(&app, 3).await.unwrap_err().contains("rejected"));
    assert!(sign(&app, 3).await.unwrap_err().contains("saved rule"));
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unanswered_prompt_times_out_and_is_closed() {
    let mut e = env().await;
    e.prompts = Arc::new(PromptHub::default());
    let store = SignerStore::new(e.db.clone()).unwrap();
    let s = Signer::with_store(
        e.vault.clone(),
        Arc::new(PolicyApprover::new(store.clone(), e.prompts.clone())),
        SignerSettings {
            default_relays: vec![e.relay_url.clone()],
            pending_timeout: Duration::from_millis(700),
            log_activity: true,
        },
        store,
    )
    .await
    .unwrap();
    s.start().await.unwrap();
    let (_, uri) = s
        .create_bunker(e.account, None, None, Policy::Manual, None)
        .await
        .unwrap();
    let app = client(&uri);
    // Manual policy: even get_public_key asks, and nobody answers.
    let err = app.get_public_key_async().await.unwrap_err().to_string();
    assert!(err.contains("timed out"), "{err}");
    assert!(
        e.prompts.pending().is_empty(),
        "the prompt went away with the request"
    );
}

#[tokio::test]
async fn apps_survive_a_restart() {
    let e = env().await;
    let first = signer(&e).await;
    let (info, uri) = first
        .create_bunker(
            e.account,
            Some("Persisted".into()),
            None,
            Policy::FullTrust,
            None,
        )
        .await
        .unwrap();
    let app = client(&uri);
    sign(&app, 1).await.unwrap();
    first.shutdown().await;

    // A new signer over the same database and keyring picks the app up.
    let second = signer(&e).await;
    let conns = second.connections().await;
    assert_eq!(conns.len(), 1);
    assert_eq!(conns[0].id, info.id);
    assert!(conns[0].connected);
    let ev = sign(&app, 1).await.unwrap();
    assert_eq!(ev.pubkey, e.account);

    // Removing it deletes the transport key from the keyring too.
    assert!(second.remove_connection(&info.id).await.unwrap());
    assert!(
        e.vault
            .store()
            .list(opal_core::keystore::ItemKind::ConnKey)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn nostrconnect_grants_become_rules() {
    let e = env().await;
    let s = signer(&e).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: false,
            remember: Remember::Once,
        },
    );
    let app_keys = Keys::generate();
    let uri = nostr_connect::prelude::NostrConnectUri::client_with_secret(
        app_keys.public_key(),
        [e.relay_url.clone()],
        "Granted",
        "sekrit",
    );
    let app = NostrConnect::new(uri.clone(), app_keys, TIMEOUT, None).unwrap();
    let waiting = tokio::spawn(async move {
        let r = app.get_public_key_async().await;
        (app, r)
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    let ours = opal_signer::NostrConnectUri::parse(&uri.to_string()).unwrap();
    s.accept_nostrconnect(
        &ours,
        e.account,
        Policy::Manual,
        &[
            (Method::GetPublicKey, None),
            (Method::SignEvent, Some(30023)),
        ],
    )
    .await
    .unwrap();
    let (app, pk) = waiting.await.unwrap();
    assert_eq!(pk.unwrap(), e.account);
    sign(&app, 30023).await.unwrap();
    assert_eq!(
        prompts.load(Ordering::SeqCst),
        0,
        "granted perms don't prompt"
    );
    assert!(
        sign(&app, 1).await.is_err(),
        "other kinds still ask (and get denied)"
    );
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn expired_bunker_links_are_refused() {
    let e = env().await;
    let s = signer(&e).await;
    let (_, uri) = s
        .create_bunker(
            e.account,
            None,
            None,
            Policy::Basic,
            Some(Duration::from_secs(1)),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let err = client(&uri)
        .get_public_key_async()
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("expired"), "{err}");
}

#[tokio::test]
async fn forward_dated_and_auth_events_always_ask() {
    let e = env().await;
    let s = signer(&e).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: false,
            remember: Remember::Once,
        },
    );
    let (_, uri) = s
        .create_bunker(e.account, None, None, Policy::Basic, None)
        .await
        .unwrap();
    let app = client(&uri);
    // A note dated a day ahead is not signed silently, even under Basic.
    let future = Timestamp::now() + Duration::from_secs(86_400);
    let err = EventBuilder::new(Kind::TextNote, "later")
        .custom_created_at(future)
        .finalize_async(&app)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("rejected"), "{err}");
    // HTTP auth (NIP-98) is a login token: it asks.
    assert!(sign(&app, 27235).await.is_err());
    assert_eq!(prompts.load(Ordering::SeqCst), 2);
    // An ordinary note still goes through on its own.
    sign(&app, 1).await.unwrap();
    assert_eq!(prompts.load(Ordering::SeqCst), 2);
}
