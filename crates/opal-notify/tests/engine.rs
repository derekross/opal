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
