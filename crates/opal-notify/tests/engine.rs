//! The engine against a local relay.

use std::sync::Arc;
use std::time::Duration;

use nostr_sdk::prelude::*;
use opal_core::config::NotificationsConfig;
use opal_core::db::Db;
use opal_core::keystore::SecretStore;
use opal_core::vault::Vault;
use opal_notify::{NotifType, NotifyEngine, NotifyEvent, NotifyParams, NotifyStore};

async fn publish(client: &Client, ev: Event) {
    client.send_event(&ev).await.unwrap();
}

fn tagged(keys: &Keys, kind: Kind, content: &str, me: &PublicKey, extra: Vec<Tag>) -> Event {
    EventBuilder::new(kind, content)
        .tag(Tag::public_key(*me))
        .tags(extra)
        .finalize(keys)
        .unwrap()
}

async fn wait_for(store: &NotifyStore, me: &str, n: usize) -> Vec<opal_notify::StoredNotification> {
    for _ in 0..50 {
        let list = store.list(me, 100, None).unwrap();
        if list.len() >= n {
            return list;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    store.list(me, 100, None).unwrap()
}

#[tokio::test]
async fn notifications_end_to_end() {
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await;
    let publisher = Client::default();
    publisher.add_relay(&url).await.unwrap();
    publisher.connect().and_wait(Duration::from_secs(3)).await;

    let me_keys = Keys::generate();
    let me = me_keys.public_key();
    let alice = Keys::generate();
    let muted_public = Keys::generate();
    let muted_private = Keys::generate();

    // Relay list (read) and a mute list with one public and one private entry.
    publish(
        &publisher,
        EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::parse(["r", url.as_str()]).unwrap())
            .finalize(&me_keys)
            .unwrap(),
    )
    .await;
    let private = serde_json::to_string(&vec![vec![
        "p".to_string(),
        muted_private.public_key().to_hex(),
    ]])
    .unwrap();
    let private = nip44::encrypt(me_keys.secret_key(), &me, private, nip44::Version::V2).unwrap();
    publish(
        &publisher,
        EventBuilder::new(Kind::MuteList, private)
            .tag(Tag::public_key(muted_public.public_key()))
            .finalize(&me_keys)
            .unwrap(),
    )
    .await;

    // The key is local and unlocked, so private mutes and DMs work.
    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    vault.add_account(me_keys.clone(), "pw").await.unwrap();
    vault.unlock("pw").await.unwrap();

    let store = NotifyStore::new(Db::open_in_memory().unwrap()).unwrap();
    let cfg = NotificationsConfig {
        bootstrap_relays: vec![url.to_string()],
        ..Default::default()
    };
    let engine = NotifyEngine::start(NotifyParams {
        account: me,
        config: cfg,
        store: store.clone(),
        vault: Some(vault.clone()),
    });
    let mut events = engine.subscribe();
    // Wait until it's subscribed.
    loop {
        if let Ok(NotifyEvent::Status { status }) = events.recv().await {
            assert_eq!(status.read_relays, vec![url.to_string()]);
            assert_eq!(status.muted, 2, "public + private mutes");
            break;
        }
    }

    let note = EventBuilder::new(Kind::TextNote, "my note")
        .finalize(&me_keys)
        .unwrap();
    publish(&publisher, note.clone()).await;
    let reply_tags = vec![Tag::parse(["e", &note.id.to_hex(), "", "reply"]).unwrap()];
    publish(
        &publisher,
        tagged(&alice, Kind::TextNote, "great note!", &me, reply_tags),
    )
    .await;
    publish(
        &publisher,
        tagged(&alice, Kind::Reaction, "🔥", &me, vec![Tag::event(note.id)]),
    )
    .await;
    publish(
        &publisher,
        tagged(&muted_public, Kind::TextNote, "spam", &me, vec![]),
    )
    .await;
    publish(
        &publisher,
        tagged(&muted_private, Kind::TextNote, "spam 2", &me, vec![]),
    )
    .await;
    publish(
        &publisher,
        tagged(&me_keys, Kind::TextNote, "self mention", &me, vec![]),
    )
    .await;

    let list = wait_for(&store, &me.to_hex(), 2).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let list = if list.len() < 2 {
        list
    } else {
        store.list(&me.to_hex(), 100, None).unwrap()
    };
    let types: Vec<_> = list.iter().map(|n| n.n.ntype).collect();
    assert_eq!(list.len(), 2, "muted and own events are dropped: {types:?}");
    assert!(types.contains(&NotifType::Reply) && types.contains(&NotifType::Reaction));

    // The referenced note is fetched for context.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let list = store.list(&me.to_hex(), 100, None).unwrap();
    assert!(
        list.iter().any(|n| n.context.as_deref() == Some("my note")),
        "context backfilled"
    );

    // A DM that arrives while locked is kept, then shows up on unlock.
    vault.lock().await;
    let rumor = EventBuilder::new(Kind::PrivateDirectMessage, "hello there")
        .tag(Tag::public_key(me))
        .finalize_unsigned(alice.public_key());
    let wrap = GiftWrapBuilder::new(me, rumor).finalize(&alice).unwrap();
    publish(&publisher, wrap).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        store.list(&me.to_hex(), 100, None).unwrap().len(),
        2,
        "not yet readable"
    );
    vault.unlock("pw").await.unwrap();
    let list = wait_for(&store, &me.to_hex(), 3).await;
    let dm = list
        .iter()
        .find(|n| n.n.ntype == NotifType::Dm)
        .expect("dm after unlock");
    assert_eq!(dm.n.author, alice.public_key().to_hex());
    assert_eq!(dm.n.detail, "", "no previews unless enabled");

    // Everything above arrived after the engine started, so it's all unread.
    assert_eq!(store.unread_count(&me.to_hex()).unwrap(), 3);
    engine.stop().await;
}

#[tokio::test]
async fn private_mutes_survive_a_restart_while_locked() {
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await;
    let publisher = Client::default();
    publisher.add_relay(&url).await.unwrap();
    publisher.connect().and_wait(Duration::from_secs(3)).await;

    let me_keys = Keys::generate();
    let me = me_keys.public_key();
    let muted = Keys::generate();
    let alice = Keys::generate();

    // Only private entries, like most clients write them.
    let private =
        serde_json::to_string(&vec![vec!["p".to_string(), muted.public_key().to_hex()]]).unwrap();
    let private = nip44::encrypt(me_keys.secret_key(), &me, private, nip44::Version::V2).unwrap();
    publish(
        &publisher,
        EventBuilder::new(Kind::MuteList, private)
            .finalize(&me_keys)
            .unwrap(),
    )
    .await;

    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    vault.add_account(me_keys.clone(), "pw").await.unwrap();
    let cfg = NotificationsConfig {
        bootstrap_relays: vec![url.to_string()],
        ..Default::default()
    };
    let start = |store: &NotifyStore| {
        NotifyEngine::start(NotifyParams {
            account: me,
            config: cfg.clone(),
            store: store.clone(),
            vault: Some(vault.clone()),
        })
    };
    async fn muted_count(engine: &NotifyEngine) -> usize {
        let mut events = engine.subscribe();
        loop {
            if let Ok(NotifyEvent::Status { status }) = events.recv().await {
                return status.muted;
            }
        }
    }

    // Locked and never decrypted: nothing to go on yet.
    let store = NotifyStore::new(Db::open_in_memory().unwrap()).unwrap();
    let engine = start(&store);
    assert_eq!(muted_count(&engine).await, 0);
    engine.stop().await;

    // Unlocked once: decrypted and cached.
    vault.unlock("pw").await.unwrap();
    let engine = start(&store);
    assert_eq!(muted_count(&engine).await, 1);
    engine.stop().await;

    // Restarted while locked: the cache still applies.
    vault.lock().await;
    let engine = start(&store);
    assert_eq!(muted_count(&engine).await, 1, "cached private mutes apply");
    publish(
        &publisher,
        tagged(&muted, Kind::TextNote, "spam", &me, vec![]),
    )
    .await;
    publish(
        &publisher,
        tagged(&alice, Kind::TextNote, "hi", &me, vec![]),
    )
    .await;
    let list = wait_for(&store, &me.to_hex(), 1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let list = if list.is_empty() {
        list
    } else {
        store.list(&me.to_hex(), 100, None).unwrap()
    };
    assert_eq!(list.len(), 1, "only alice gets through");
    assert_eq!(list[0].n.author, alice.public_key().to_hex());
    engine.stop().await;

    // Removing the account drops its cache.
    vault.unlock("pw").await.unwrap();
    vault.remove_account(&me).await.unwrap();
    assert!(
        vault
            .store()
            .get(opal_core::keystore::ItemKind::MuteCache, &me.to_hex())
            .await
            .unwrap()
            .is_none()
    );
}

async fn newest_mute_list(client: &Client, me: PublicKey) -> Event {
    client
        .fetch_events(Filter::new().author(me).kind(Kind::MuteList))
        .timeout(Duration::from_secs(3))
        .await
        .unwrap()
        .into_iter()
        .max_by_key(|e| e.created_at)
        .unwrap()
}

fn private_tags(keys: &Keys, ev: &Event) -> Vec<Vec<String>> {
    if ev.content.is_empty() {
        return vec![];
    }
    let plain = nip44::decrypt(keys.secret_key(), &keys.public_key(), &ev.content).unwrap();
    serde_json::from_str(&plain).unwrap()
}

#[tokio::test]
async fn muting_from_opal_keeps_the_rest_of_the_list() {
    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await;
    let publisher = Client::default();
    publisher.add_relay(&url).await.unwrap();
    publisher.connect().and_wait(Duration::from_secs(3)).await;

    let me_keys = Keys::generate();
    let me = me_keys.public_key();
    let public_one = Keys::generate().public_key();
    let private_one = Keys::generate().public_key();
    let target = Keys::generate();

    publish(
        &publisher,
        EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::parse(["r", url.as_str()]).unwrap())
            .finalize(&me_keys)
            .unwrap(),
    )
    .await;
    // Written by another client: a public entry, a hashtag, a private entry.
    let private = serde_json::to_string(&vec![
        vec!["p".to_string(), private_one.to_hex()],
        vec!["word".to_string(), "gm".to_string()],
    ])
    .unwrap();
    let private = nip44::encrypt(me_keys.secret_key(), &me, private, nip44::Version::V2).unwrap();
    let original = EventBuilder::new(Kind::MuteList, private)
        .tag(Tag::public_key(public_one))
        .tag(Tag::hashtag("spam"))
        .finalize(&me_keys)
        .unwrap();
    publish(&publisher, original.clone()).await;

    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    vault.add_account(me_keys.clone(), "pw").await.unwrap();
    let store = NotifyStore::new(Db::open_in_memory().unwrap()).unwrap();
    let engine = NotifyEngine::start(NotifyParams {
        account: me,
        config: NotificationsConfig {
            bootstrap_relays: vec![url.to_string()],
            ..Default::default()
        },
        store: store.clone(),
        vault: Some(vault.clone()),
    });
    let mut events = engine.subscribe();
    while !matches!(events.recv().await, Ok(NotifyEvent::Status { .. })) {}
    let handle = engine.handle();

    // Locked: asks for the passphrase and publishes nothing.
    assert_eq!(
        handle.set_muted(target.public_key(), true).await,
        Err(opal_notify::MUTE_NEEDS_UNLOCK.to_string())
    );

    // A note from the target is already stored; muting removes it.
    vault.unlock("pw").await.unwrap();
    publish(
        &publisher,
        tagged(&target, Kind::TextNote, "noise", &me, vec![]),
    )
    .await;
    assert_eq!(wait_for(&store, &me.to_hex(), 1).await.len(), 1);

    handle.set_muted(target.public_key(), true).await.unwrap();
    let ev = newest_mute_list(&publisher, me).await;
    assert!(ev.created_at > original.created_at);
    assert_eq!(ev.tags, original.tags, "public part untouched");
    let private = private_tags(&me_keys, &ev);
    assert!(private.contains(&vec!["p".to_string(), private_one.to_hex()]));
    assert!(private.contains(&vec!["word".to_string(), "gm".to_string()]));
    assert!(
        private.contains(&vec!["p".to_string(), target.public_key().to_hex()]),
        "the new mute is private"
    );
    assert!(store.list(&me.to_hex(), 100, None).unwrap().is_empty());
    let mutes = handle.mutes().await;
    assert!(
        mutes
            .iter()
            .any(|e| e.pubkey == target.public_key().to_hex() && e.private)
    );
    assert!(
        mutes
            .iter()
            .any(|e| e.pubkey == public_one.to_hex() && !e.private)
    );

    // Unmuting removes it from either part, and only it.
    handle.set_muted(public_one, false).await.unwrap();
    handle.set_muted(target.public_key(), false).await.unwrap();
    let ev = newest_mute_list(&publisher, me).await;
    assert_eq!(ev.tags.len(), 1, "only the hashtag is left: {:?}", ev.tags);
    assert_eq!(
        private_tags(&me_keys, &ev),
        vec![
            vec!["p".to_string(), private_one.to_hex()],
            vec!["word".to_string(), "gm".to_string()],
        ]
    );

    // A list Opal can't read is never overwritten.
    let unreadable = EventBuilder::new(Kind::MuteList, "not encrypted")
        .custom_created_at(Timestamp::from(ev.created_at.as_secs() + 10))
        .finalize(&me_keys)
        .unwrap();
    publish(&publisher, unreadable.clone()).await;
    assert!(handle.set_muted(target.public_key(), true).await.is_err());
    assert_eq!(newest_mute_list(&publisher, me).await.id, unreadable.id);
    engine.stop().await;
}

#[tokio::test]
async fn muting_with_no_relay_answering_publishes_nothing() {
    let me_keys = Keys::generate();
    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    vault.add_account(me_keys.clone(), "pw").await.unwrap();
    vault.unlock("pw").await.unwrap();
    let engine = NotifyEngine::start(NotifyParams {
        account: me_keys.public_key(),
        config: NotificationsConfig {
            // Nothing listens here, so the fetch comes back empty.
            bootstrap_relays: vec!["ws://127.0.0.1:9".to_string()],
            ..Default::default()
        },
        store: NotifyStore::new(Db::open_in_memory().unwrap()).unwrap(),
        vault: Some(vault),
    });
    let r = engine
        .handle()
        .set_muted(Keys::generate().public_key(), true)
        .await;
    assert!(
        r.as_ref().is_err_and(|e| e.contains("couldn't confirm")),
        "{r:?}"
    );
    engine.stop().await;
}

/// An engine for `me` with `bootstrap` as its only bootstrap relay, unlocked.
async fn unlocked_engine(me_keys: &Keys, bootstrap: &str) -> NotifyEngine {
    let vault = Arc::new(Vault::with_log_n(SecretStore::memory(), 4));
    vault.add_account(me_keys.clone(), "pw").await.unwrap();
    vault.unlock("pw").await.unwrap();
    let engine = NotifyEngine::start(NotifyParams {
        account: me_keys.public_key(),
        config: NotificationsConfig {
            bootstrap_relays: vec![bootstrap.to_string()],
            ..Default::default()
        },
        store: NotifyStore::new(Db::open_in_memory().unwrap()).unwrap(),
        vault: Some(vault),
    });
    let mut events = engine.subscribe();
    while !matches!(events.recv().await, Ok(NotifyEvent::Status { .. })) {}
    engine
}

async fn connected(url: &RelayUrl) -> Client {
    let c = Client::default();
    c.add_relay(url).await.unwrap();
    c.connect().and_wait(Duration::from_secs(3)).await;
    c
}

fn private_list(keys: &Keys, entries: &[PublicKey], at: u64) -> Event {
    let tags: Vec<Vec<String>> = entries
        .iter()
        .map(|p| vec!["p".to_string(), p.to_hex()])
        .collect();
    let content = nip44::encrypt(
        keys.secret_key(),
        &keys.public_key(),
        serde_json::to_string(&tags).unwrap(),
        nip44::Version::V2,
    )
    .unwrap();
    EventBuilder::new(Kind::MuteList, content)
        .custom_created_at(Timestamp::from(at))
        .finalize(keys)
        .unwrap()
}

#[tokio::test]
async fn muting_builds_on_the_list_your_write_relays_have() {
    // Bootstrap holds your relay list and an old copy of your mutes; the
    // current list lives only on your write relay.
    let boot = MockRelay::run().await.unwrap();
    let boot_url = boot.url().await;
    let outbox = MockRelay::run().await.unwrap();
    let outbox_url = outbox.url().await;
    let me_keys = Keys::generate();
    let me = me_keys.public_key();
    let old_one = Keys::generate().public_key();
    let new_one = Keys::generate().public_key();
    let target = Keys::generate().public_key();
    let now = Timestamp::now().as_secs();

    let b = connected(&boot_url).await;
    publish(
        &b,
        EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::parse(["r", outbox_url.as_str(), "write"]).unwrap())
            .finalize(&me_keys)
            .unwrap(),
    )
    .await;
    publish(&b, private_list(&me_keys, &[old_one], now - 100)).await;
    let o = connected(&outbox_url).await;
    publish(&o, private_list(&me_keys, &[old_one, new_one], now - 10)).await;

    let engine = unlocked_engine(&me_keys, boot_url.as_str()).await;
    engine.handle().set_muted(target, true).await.unwrap();
    let ev = newest_mute_list(&o, me).await;
    let private = private_tags(&me_keys, &ev);
    for pk in [old_one, new_one, target] {
        assert!(
            private.contains(&vec!["p".to_string(), pk.to_hex()]),
            "{pk} kept: {private:?}"
        );
    }
    engine.stop().await;
}

#[tokio::test]
async fn muting_with_your_write_relays_down_publishes_nothing() {
    let boot = MockRelay::run().await.unwrap();
    let boot_url = boot.url().await;
    let me_keys = Keys::generate();
    let b = connected(&boot_url).await;
    publish(
        &b,
        EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::parse(["r", "ws://127.0.0.1:9", "write"]).unwrap())
            .finalize(&me_keys)
            .unwrap(),
    )
    .await;
    let old = private_list(&me_keys, &[Keys::generate().public_key()], 1_700_000_000);
    publish(&b, old.clone()).await;

    let engine = unlocked_engine(&me_keys, boot_url.as_str()).await;
    let r = engine
        .handle()
        .set_muted(Keys::generate().public_key(), true)
        .await;
    assert!(
        r.as_ref().is_err_and(|e| e.contains("couldn't confirm")),
        "{r:?}"
    );
    assert_eq!(newest_mute_list(&b, me_keys.public_key()).await.id, old.id);
    engine.stop().await;
}

#[tokio::test]
async fn muting_follows_your_newest_relay_list() {
    // Bootstrap has an old relay list naming an old relay, which still has
    // your old mutes and a newer relay list naming the relay you use now.
    let boot = MockRelay::run().await.unwrap();
    let old = MockRelay::run().await.unwrap();
    let new = MockRelay::run().await.unwrap();
    let (boot_url, old_url, new_url) = (boot.url().await, old.url().await, new.url().await);
    let me_keys = Keys::generate();
    let me = me_keys.public_key();
    let now = Timestamp::now().as_secs();
    let relay_list = |url: &RelayUrl, at: u64| {
        EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::parse(["r", url.as_str()]).unwrap())
            .custom_created_at(Timestamp::from(at))
            .finalize(&me_keys)
            .unwrap()
    };
    let kept = Keys::generate().public_key();
    let target = Keys::generate().public_key();

    publish(&connected(&boot_url).await, relay_list(&old_url, now - 300)).await;
    let o = connected(&old_url).await;
    publish(&o, relay_list(&new_url, now - 200)).await;
    let stale = private_list(&me_keys, &[], now - 150);
    publish(&o, stale.clone()).await;
    let n = connected(&new_url).await;
    publish(&n, relay_list(&new_url, now - 200)).await;
    publish(&n, private_list(&me_keys, &[kept], now - 100)).await;

    let engine = unlocked_engine(&me_keys, boot_url.as_str()).await;
    engine.handle().set_muted(target, true).await.unwrap();
    let ev = newest_mute_list(&n, me).await;
    let private = private_tags(&me_keys, &ev);
    assert!(
        private.contains(&vec!["p".to_string(), kept.to_hex()]),
        "{private:?}"
    );
    assert!(private.contains(&vec!["p".to_string(), target.to_hex()]));
    engine.stop().await;
}

#[tokio::test]
async fn muting_without_a_relay_list_changes_nothing() {
    let boot = MockRelay::run().await.unwrap();
    let boot_url = boot.url().await;
    let me_keys = Keys::generate();
    let engine = unlocked_engine(&me_keys, boot_url.as_str()).await;
    assert_eq!(
        engine
            .handle()
            .set_muted(Keys::generate().public_key(), true)
            .await,
        Err(opal_notify::MUTE_NO_RELAY_LIST.to_string())
    );
    engine.stop().await;
}

/// A relay that takes the REQ and then never finishes it: it hangs up, or
/// sends a CLOSED with no machine-readable prefix.
async fn unfinished_relay(hang_up: bool) -> String {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(text) = msg else { continue };
                    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                    if v[0] != "REQ" {
                        continue;
                    }
                    if hang_up {
                        return;
                    }
                    let closed = serde_json::json!(["CLOSED", v[1], "too many subscriptions"]);
                    let _ = ws.send(Message::text(closed.to_string())).await;
                }
            });
        }
    });
    url
}

#[tokio::test]
async fn relays_that_never_finish_answering_dont_count() {
    for hang_up in [true, false] {
        let boot = MockRelay::run().await.unwrap();
        let boot_url = boot.url().await;
        let bad = unfinished_relay(hang_up).await;
        let me_keys = Keys::generate();
        let b = connected(&boot_url).await;
        // Two write relays: the bootstrap one answers, the other never does.
        publish(
            &b,
            EventBuilder::new(Kind::RelayList, "")
                .tag(Tag::parse(["r", boot_url.as_str(), "write"]).unwrap())
                .tag(Tag::parse(["r", bad.as_str(), "write"]).unwrap())
                .finalize(&me_keys)
                .unwrap(),
        )
        .await;
        let old = private_list(&me_keys, &[Keys::generate().public_key()], 1_700_000_000);
        publish(&b, old.clone()).await;

        let engine = unlocked_engine(&me_keys, boot_url.as_str()).await;
        let r = engine
            .handle()
            .set_muted(Keys::generate().public_key(), true)
            .await;
        assert_eq!(
            r,
            Err(opal_notify::MUTE_UNSURE.to_string()),
            "hang_up={hang_up}"
        );
        assert_eq!(newest_mute_list(&b, me_keys.public_key()).await.id, old.id);
        engine.stop().await;
    }
}

#[tokio::test]
async fn unmuting_the_last_private_entry_leaves_nothing_behind() {
    let boot = MockRelay::run().await.unwrap();
    let boot_url = boot.url().await;
    let me_keys = Keys::generate();
    let me = me_keys.public_key();
    let them = Keys::generate().public_key();
    let b = connected(&boot_url).await;
    publish(
        &b,
        EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::parse(["r", boot_url.as_str()]).unwrap())
            .finalize(&me_keys)
            .unwrap(),
    )
    .await;
    publish(
        &b,
        private_list(&me_keys, &[them], Timestamp::now().as_secs() - 60),
    )
    .await;

    let engine = unlocked_engine(&me_keys, boot_url.as_str()).await;
    let handle = engine.handle();
    assert_eq!(handle.mutes().await.len(), 1);
    handle.set_muted(them, false).await.unwrap();
    assert!(newest_mute_list(&b, me).await.content.is_empty());
    assert!(
        handle.mutes().await.is_empty(),
        "{:?}",
        handle.mutes().await
    );
    // Unmuting again changes nothing and says so by not failing.
    handle.set_muted(them, false).await.unwrap();
    engine.stop().await;
}
