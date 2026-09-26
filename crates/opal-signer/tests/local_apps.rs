//! Local apps (Peridot): pairing, tokens, the shared approval path.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use nostr_sdk::prelude::*;
use opal_core::config::Policy;
use opal_core::db::Db;
use opal_core::keystore::SecretStore;
use opal_core::vault::Vault;
use opal_signer::permissions::{Remember, Source};
use opal_signer::protocol::Method;
use opal_signer::store::ActivityQuery;
use opal_signer::{
    AppKind, LocalPairing, Op, Outcome, PolicyApprover, PromptAnswer, PromptEvent, PromptHub,
    Signer, SignerEvent, SignerSettings, SignerStore, grantable,
};

const TIMEOUT: Duration = Duration::from_secs(5);
const EXE: &str = "/home/me/.local/bin/peridotd";
const UNIT: &str = "peridot.service";

struct Env {
    vault: Arc<Vault>,
    db: Db,
    account: PublicKey,
    prompts: Arc<PromptHub>,
}

async fn env() -> Env {
    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    let account = vault.add_account(Keys::generate(), "pw").await.unwrap();
    vault.unlock("pw").await.unwrap();
    Env {
        vault,
        db: Db::open_in_memory().unwrap(),
        account,
        prompts: Arc::new(PromptHub::default()),
    }
}

/// No relays, never started: local apps don't need the network.
async fn signer(e: &Env, log_activity: bool) -> Signer {
    let store = SignerStore::new(e.db.clone()).unwrap();
    let approver = Arc::new(PolicyApprover::new(store.clone(), e.prompts.clone()));
    Signer::with_store(
        e.vault.clone(),
        approver,
        SignerSettings {
            default_relays: vec![],
            pending_timeout: TIMEOUT,
            log_activity,
        },
        store,
    )
    .await
    .unwrap()
}

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

fn pairing(e: &Env, policy: Policy, grant: &[(Method, Option<u16>)]) -> LocalPairing {
    LocalPairing {
        app_key: "peridot".into(),
        name: "Peridot".into(),
        account: e.account,
        policy,
        kinds: vec![30078, 22242, 24242],
        nip44: true,
        exe: Some(EXE.into()),
        unit: Some(UNIT.into()),
        grant: grant.to_vec(),
    }
}

fn unsigned(account: PublicKey, kind: u16) -> UnsignedEvent {
    UnsignedEvent::new(account, Timestamp::now(), Kind::from(kind), vec![], "x")
}

async fn sign(s: &Signer, token: &str, account: PublicKey, kind: u16) -> Result<Event, String> {
    let app = s.local_app_by_token(token).ok_or("not paired")?;
    match s
        .local_request(&app, Op::SignEvent(unsigned(account, kind)))
        .await?
    {
        Outcome::Event(ev) => Ok(ev),
        _ => Err("not an event".into()),
    }
}

async fn last_activity(s: &Signer) -> Option<opal_signer::store::ActivityEntry> {
    s.store()
        .unwrap()
        .activity(&ActivityQuery::default())
        .unwrap()
        .into_iter()
        .next()
}

#[tokio::test]
async fn pairing_creates_row_rules_and_token() {
    let e = env().await;
    let s = signer(&e, true).await;
    let mut events = s.subscribe_events();
    let (info, token) = s
        .pair_local_app(pairing(
            &e,
            Policy::Basic,
            &[(Method::SignEvent, Some(30078))],
        ))
        .await
        .unwrap();
    assert_eq!(info.kind, AppKind::Local);
    assert_eq!(info.display_name, "Peridot");
    assert_eq!(info.exe.as_deref(), Some(EXE));
    assert_eq!(info.unit.as_deref(), Some(UNIT));
    assert_eq!(info.kinds, vec![30078, 22242, 24242]);
    assert!(info.nip44);
    assert_eq!(token.len(), 64);
    assert!(matches!(
        events.try_recv(),
        Ok(SignerEvent::Connected { .. })
    ));

    let apps = s.all_apps().await;
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, info.id);
    assert_eq!(
        s.app_info(&info.id).await.map(|a| a.kind),
        Some(AppKind::Local)
    );

    let rules = s.store().unwrap().rules(&info.id).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].kind, Some(30078));
    assert_eq!(rules[0].until, None, "granted forever");

    assert_eq!(s.local_app_by_token(&token).map(|a| a.id), Some(info.id));
    assert!(s.local_app_by_token(&"0".repeat(64)).is_none());
    assert!(s.local_app_by_token("short").is_none());
}

#[tokio::test]
async fn grants_never_include_sensitive_kinds() {
    let e = env().await;
    let s = signer(&e, true).await;
    let (info, _) = s
        .pair_local_app(pairing(
            &e,
            Policy::Manual,
            // Asks for a Blossom grant it may not have, and one it may.
            &[
                (Method::SignEvent, Some(24242)),
                (Method::Nip44Encrypt, None),
            ],
        ))
        .await
        .unwrap();
    let rules = s.store().unwrap().rules(&info.id).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].method, Method::Nip44Encrypt);
    assert_eq!(
        grantable(&info.kinds, info.nip44),
        vec![
            (Method::SignEvent, Some(30078)),
            (Method::Nip44Encrypt, None)
        ]
    );
}

#[tokio::test]
async fn sign_within_declared_kinds_uses_rules() {
    let e = env().await;
    let s = signer(&e, true).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: true,
            remember: Remember::Once,
        },
    );
    let (_, token) = s
        .pair_local_app(pairing(
            &e,
            Policy::Manual,
            &[(Method::SignEvent, Some(30078))],
        ))
        .await
        .unwrap();
    let mut events = s.subscribe_events();

    let ev = sign(&s, &token, e.account, 30078).await.unwrap();
    assert!(ev.verify().is_ok());
    assert_eq!(ev.pubkey, e.account);
    assert_eq!(prompts.load(Ordering::SeqCst), 0, "a saved rule, no prompt");

    let entry = last_activity(&s).await.unwrap();
    assert_eq!(entry.app_name, "Peridot");
    assert_eq!(entry.kind, Some(30078));
    assert_eq!(entry.source, Source::Rule);
    assert!(entry.allowed);
    assert!(matches!(
        events.recv().await,
        Ok(SignerEvent::Request {
            allowed: true,
            source: Source::Rule,
            ..
        })
    ));
}

#[tokio::test]
async fn undeclared_kind_and_wrong_account_are_refused_before_approval() {
    let e = env().await;
    let s = signer(&e, true).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: true,
            remember: Remember::Once,
        },
    );
    let (_, token) = s
        .pair_local_app(pairing(&e, Policy::Basic, &[]))
        .await
        .unwrap();

    let err = sign(&s, &token, e.account, 1).await.unwrap_err();
    assert_eq!(err, "Peridot didn't declare kind 1 when it paired");

    let other = Keys::generate().public_key();
    let err = sign(&s, &token, other, 30078).await.unwrap_err();
    assert_eq!(err, "the event isn't for that account");

    assert_eq!(prompts.load(Ordering::SeqCst), 0);
    assert!(last_activity(&s).await.is_none(), "nothing reached the log");
}

#[tokio::test]
async fn sensitive_kind_prompts_and_is_remembered_for_an_hour_at_most() {
    let e = env().await;
    let s = signer(&e, true).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: true,
            remember: Remember::Always,
        },
    );
    let (info, token) = s
        .pair_local_app(pairing(&e, Policy::Basic, &[]))
        .await
        .unwrap();

    sign(&s, &token, e.account, 24242).await.unwrap();
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
    assert_eq!(last_activity(&s).await.unwrap().source, Source::User);

    let now = Timestamp::now().as_secs();
    let rules = s.store().unwrap().rules(&info.id).unwrap();
    assert_eq!(rules.len(), 1);
    let until = rules[0].until.unwrap();
    assert!(
        (now + 3500..=now + 3600).contains(&until),
        "capped at an hour"
    );

    sign(&s, &token, e.account, 24242).await.unwrap();
    assert_eq!(prompts.load(Ordering::SeqCst), 1, "remembered");
    assert_eq!(last_activity(&s).await.unwrap().source, Source::Rule);
}

#[tokio::test]
async fn relay_auth_is_automatic_under_basic_and_asks_under_manual() {
    let e = env().await;
    let s = signer(&e, true).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: true,
            remember: Remember::Once,
        },
    );
    let (_, token) = s
        .pair_local_app(pairing(&e, Policy::Basic, &[]))
        .await
        .unwrap();
    sign(&s, &token, e.account, 22242).await.unwrap();
    assert_eq!(prompts.load(Ordering::SeqCst), 0);
    assert_eq!(last_activity(&s).await.unwrap().source, Source::BasicPolicy);

    let (_, token) = s
        .pair_local_app(pairing(&e, Policy::Manual, &[]))
        .await
        .unwrap();
    sign(&s, &token, e.account, 22242).await.unwrap();
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn full_trust_signs_declared_kinds_without_prompt() {
    let e = env().await;
    let s = signer(&e, true).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: true,
            remember: Remember::Once,
        },
    );
    let (_, token) = s
        .pair_local_app(pairing(&e, Policy::FullTrust, &[]))
        .await
        .unwrap();
    for kind in [30078, 22242, 24242] {
        sign(&s, &token, e.account, kind).await.unwrap();
    }
    assert_eq!(prompts.load(Ordering::SeqCst), 0);
    assert_eq!(last_activity(&s).await.unwrap().source, Source::FullTrust);
    // Still only what it declared.
    assert!(sign(&s, &token, e.account, 1).await.is_err());
}

#[tokio::test]
async fn nip44_is_self_only_and_decrypt_asks() {
    let e = env().await;
    let s = signer(&e, true).await;
    let prompts = auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: true,
            remember: Remember::Once,
        },
    );

    let mut no_nip44 = pairing(&e, Policy::Basic, &[]);
    no_nip44.nip44 = false;
    let (_, token) = s.pair_local_app(no_nip44).await.unwrap();
    let app = s.local_app_by_token(&token).unwrap();
    let err = s
        .local_request(
            &app,
            Op::Cipher {
                method: Method::Nip44Encrypt,
                with: e.account,
                text: "secret".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, "Peridot didn't ask for encryption when it paired");

    let (_, token) = s
        .pair_local_app(pairing(&e, Policy::Basic, &[(Method::Nip44Encrypt, None)]))
        .await
        .unwrap();
    let app = s.local_app_by_token(&token).unwrap();
    let Outcome::Text(cipher) = s
        .local_request(
            &app,
            Op::Cipher {
                method: Method::Nip44Encrypt,
                with: e.account,
                text: "secret".into(),
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected text");
    };
    assert_eq!(prompts.load(Ordering::SeqCst), 0, "encrypt was granted");

    let Outcome::Text(plain) = s
        .local_request(
            &app,
            Op::Cipher {
                method: Method::Nip44Decrypt,
                with: e.account,
                text: cipher,
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected text");
    };
    assert_eq!(plain, "secret");
    assert_eq!(prompts.load(Ordering::SeqCst), 1, "decrypt asks");

    let other = Keys::generate().public_key();
    let err = s
        .local_request(
            &app,
            Op::Cipher {
                method: Method::Nip44Encrypt,
                with: other,
                text: "x".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, "local apps only encrypt to their own account");
}

#[tokio::test]
async fn revoked_app_is_refused_and_its_rules_are_gone() {
    let e = env().await;
    let s = signer(&e, true).await;
    let (info, token) = s
        .pair_local_app(pairing(
            &e,
            Policy::Basic,
            &[(Method::SignEvent, Some(30078))],
        ))
        .await
        .unwrap();
    let mut events = s.subscribe_events();
    assert!(s.remove_app(&info.id).await.unwrap());
    assert!(matches!(
        events.recv().await,
        Ok(SignerEvent::Disconnected { .. })
    ));
    assert!(s.local_app_by_token(&token).is_none());
    assert!(s.store().unwrap().rules(&info.id).unwrap().is_empty());
    assert!(s.all_apps().await.is_empty());
    assert!(!s.remove_app(&info.id).await.unwrap());
    assert_eq!(
        sign(&s, &token, e.account, 30078).await.unwrap_err(),
        "not paired"
    );
}

#[tokio::test]
async fn repairing_keeps_id_clears_rules_and_rotates_token() {
    let e = env().await;
    let s = signer(&e, true).await;
    let (first, old_token) = s
        .pair_local_app(pairing(
            &e,
            Policy::Basic,
            &[(Method::SignEvent, Some(30078))],
        ))
        .await
        .unwrap();
    let mut events = s.subscribe_events();
    let (second, new_token) = s
        .pair_local_app(pairing(&e, Policy::Manual, &[(Method::Nip44Encrypt, None)]))
        .await
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.policy, Policy::Manual);
    assert!(matches!(events.try_recv(), Ok(SignerEvent::Updated { .. })));
    assert_eq!(s.all_apps().await.len(), 1);
    assert!(s.local_app_by_token(&old_token).is_none());
    assert!(s.local_app_by_token(&new_token).is_some());
    let rules = s.store().unwrap().rules(&second.id).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].method, Method::Nip44Encrypt);
}

#[tokio::test]
async fn update_app_changes_name_and_policy() {
    let e = env().await;
    let s = signer(&e, true).await;
    let (info, _) = s
        .pair_local_app(pairing(&e, Policy::Basic, &[]))
        .await
        .unwrap();
    let updated = s
        .update_app(&info.id, Some("Peri".into()), Some(Policy::Manual), None)
        .await
        .unwrap();
    assert_eq!(updated.display_name, "Peri");
    assert_eq!(updated.policy, Policy::Manual);
    assert_eq!(updated.kind, AppKind::Local);
    assert!(s.update_app("nope", None, None, None).await.is_err());
}

#[tokio::test]
async fn privacy_mode_logs_nothing_but_still_emits() {
    let e = env().await;
    let s = signer(&e, false).await;
    let (_, token) = s
        .pair_local_app(pairing(
            &e,
            Policy::Basic,
            &[(Method::SignEvent, Some(30078))],
        ))
        .await
        .unwrap();
    let mut events = s.subscribe_events();
    sign(&s, &token, e.account, 30078).await.unwrap();
    assert!(last_activity(&s).await.is_none());
    assert!(matches!(
        events.recv().await,
        Ok(SignerEvent::Request { .. })
    ));
}

#[tokio::test]
async fn local_apps_survive_restart() {
    let e = env().await;
    let (info, token) = {
        let s = signer(&e, true).await;
        s.pair_local_app(pairing(
            &e,
            Policy::Basic,
            &[(Method::SignEvent, Some(30078))],
        ))
        .await
        .unwrap()
    };
    // A second signer over the same database: the NIP-46 loader must leave
    // local rows alone (they have no transport key to check).
    let s = signer(&e, true).await;
    let apps = s.all_apps().await;
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, info.id);
    assert!(s.connections().await.is_empty());
    sign(&s, &token, e.account, 30078).await.unwrap();
}

#[tokio::test]
async fn locked_vault_fails_fast_and_unlogged() {
    let e = env().await;
    let s = signer(&e, true).await;
    let (_, token) = s
        .pair_local_app(pairing(
            &e,
            Policy::Basic,
            &[(Method::SignEvent, Some(30078))],
        ))
        .await
        .unwrap();
    e.vault.lock().await;
    let mut events = s.subscribe_events();
    let started = Instant::now();
    assert_eq!(
        sign(&s, &token, e.account, 30078).await.unwrap_err(),
        "Opal is locked"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(last_activity(&s).await.is_none());
    assert!(events.try_recv().is_err(), "no unlock nag for local apps");
}

#[tokio::test]
async fn denied_prompt_is_logged_with_its_reason() {
    let e = env().await;
    let s = signer(&e, true).await;
    auto_answer(
        e.prompts.clone(),
        PromptAnswer {
            allow: false,
            remember: Remember::Once,
        },
    );
    let (_, token) = s
        .pair_local_app(pairing(&e, Policy::Manual, &[]))
        .await
        .unwrap();
    let err = sign(&s, &token, e.account, 30078).await.unwrap_err();
    assert_eq!(err, "user rejected");
    let entry = last_activity(&s).await.unwrap();
    assert!(!entry.allowed);
    assert_eq!(entry.source, Source::User);
}
