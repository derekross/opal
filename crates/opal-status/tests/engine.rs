//! The status engine with a fake MPRIS player and a local relay.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use nostr_sdk::prelude::*;
use opal_core::config::StatusConfig;
use opal_core::db::Db;
use opal_status::engine::SignError;
use opal_status::general::Manual;
use opal_status::{StatusEngine, StatusParams, StatusSigner, StatusStore};
use tokio::sync::watch;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

struct Player {
    status: String,
    title: String,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.status.clone()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let v = |x: Value<'_>| OwnedValue::try_from(x).unwrap();
        m.insert("xesam:title".into(), v(Value::from(self.title.clone())));
        m.insert(
            "xesam:artist".into(),
            v(Value::from(vec!["Test Artist".to_string()])),
        );
        m.insert("mpris:length".into(), v(Value::from(200_000_000i64)));
        m.insert(
            "mpris:trackid".into(),
            v(Value::from(
                ObjectPath::try_from("/com/spotify/track/abc123").unwrap(),
            )),
        );
        m
    }
    #[zbus(property)]
    fn position(&self) -> i64 {
        0
    }
}

/// Signs with local keys unless "locked".
struct TestSigner {
    keys: Keys,
    locked: AtomicBool,
}

impl StatusSigner for TestSigner {
    fn sign(&self, unsigned: UnsignedEvent) -> BoxFuture<'_, Result<Event, SignError>> {
        Box::pin(async move {
            if self.locked.load(Ordering::SeqCst) {
                return Err(SignError::Unavailable("locked".into()));
            }
            self.keys
                .sign_event(unsigned)
                .map_err(|e| SignError::Failed(e.to_string()))
        })
    }
}

async fn statuses(reader: &Client, author: PublicKey) -> Vec<Event> {
    let f = Filter::new().author(author).kind(Kind::from(30315u16));
    let mut v: Vec<Event> = reader
        .fetch_events(f)
        .timeout(Duration::from_secs(3))
        .await
        .unwrap()
        .into_iter()
        .collect();
    v.sort_by_key(|e| e.created_at);
    v
}

fn d_tag(e: &Event) -> String {
    e.tags.identifier().unwrap_or_default().to_string()
}

#[tokio::test]
async fn music_and_manual_statuses() {
    // A fake player on the session bus.
    let Ok(conn) = zbus::Connection::session().await else {
        eprintln!("no session bus; skipping");
        return;
    };
    let name = format!("org.mpris.MediaPlayer2.opaltest{}", std::process::id());
    conn.object_server()
        .at(
            "/org/mpris/MediaPlayer2",
            Player {
                status: "Playing".into(),
                title: "First Song".into(),
            },
        )
        .await
        .unwrap();
    conn.request_name(name.as_str()).await.unwrap();

    let relay = MockRelay::run().await.unwrap();
    let url = relay.url().await;
    let reader = Client::default();
    reader.add_relay(&url).await.unwrap();
    reader.connect().and_wait(Duration::from_secs(3)).await;

    let keys = Keys::generate();
    let signer = Arc::new(TestSigner {
        keys: keys.clone(),
        locked: AtomicBool::new(false),
    });
    let (ready_tx, ready_rx) = watch::channel(true);
    let cfg = StatusConfig {
        players: vec![format!("opaltest{}", std::process::id())],
        relays: vec![url.to_string()],
        ..Default::default()
    };
    let engine = StatusEngine::start(StatusParams {
        account: keys.public_key(),
        config: cfg,
        store: StatusStore::new(Db::open_in_memory().unwrap()).unwrap(),
        signer: signer.clone(),
        bootstrap_relays: vec![url.to_string()],
        signer_ready: Some(ready_rx),
    });

    // Debounce is 3s; give it a little more.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let evs = statuses(&reader, keys.public_key()).await;
    let music: Vec<_> = evs.iter().filter(|e| d_tag(e) == "music").collect();
    assert_eq!(music.len(), 1, "one music status: {evs:?}");
    assert_eq!(music[0].content, "Test Artist - First Song");
    assert!(
        music[0]
            .tags
            .iter()
            .any(|t| t.as_slice() == ["r", "https://open.spotify.com/track/abc123"])
    );
    let snap = engine.snapshot().await;
    assert_eq!(snap.music.as_deref(), Some("Test Artist - First Song"));

    // Pause: the status is cleared right away.
    let iface = conn
        .object_server()
        .interface::<_, Player>("/org/mpris/MediaPlayer2")
        .await
        .unwrap();
    iface.get_mut().await.status = "Paused".into();
    iface
        .get()
        .await
        .playback_status_changed(iface.signal_emitter())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let evs = statuses(&reader, keys.public_key()).await;
    let newest_music = evs.iter().rfind(|e| d_tag(e) == "music").unwrap();
    assert_eq!(newest_music.content, "", "cleared on pause");

    // A manual status while "locked" waits, then goes out on unlock.
    signer.locked.store(true, Ordering::SeqCst);
    let _ = ready_tx.send(false);
    engine
        .set_manual(Some(Manual {
            text: "Testing Opal".into(),
            link: None,
            expires_at: None,
        }))
        .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        statuses(&reader, keys.public_key())
            .await
            .iter()
            .all(|e| d_tag(e) != "general")
    );
    assert_eq!(engine.snapshot().await.waiting, 1);
    signer.locked.store(false, Ordering::SeqCst);
    let _ = ready_tx.send(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let evs = statuses(&reader, keys.public_key()).await;
    let general = evs
        .iter()
        .find(|e| d_tag(e) == "general")
        .expect("general status");
    assert_eq!(general.content, "Testing Opal");

    engine.stop().await;
}
