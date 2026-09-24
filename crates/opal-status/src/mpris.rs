//! What media players (MPRIS over D-Bus) are playing.
//!
//! Re-reads players when they signal a change (PropertiesChanged, Seeked,
//! players appearing/disappearing), plus a slow poll for players that don't.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use serde::Serialize;
use tokio::sync::watch;
use zbus::zvariant::OwnedValue;

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const FALLBACK_POLL: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Track {
    /// Player base name ("spotify", "chromium"), instance suffix removed.
    pub player: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    #[serde(serialize_with = "secs_opt")]
    pub length: Option<Duration>,
    #[serde(serialize_with = "secs_opt")]
    pub position: Option<Duration>,
    /// `xesam:url`: the track's own web page, when the player gives one.
    pub url: Option<String>,
    /// `mpris:trackid`, e.g. `/com/spotify/track/<id>`.
    pub track_id: Option<String>,
    pub art_url: Option<String>,
    pub playing: bool,
}

fn secs_opt<S: serde::Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_some(&d.as_secs()),
        None => s.serialize_none(),
    }
}

impl Track {
    /// "Artist - Title", or just the title.
    pub fn display(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }

    /// Same song regardless of player position.
    pub fn key(&self) -> String {
        format!(
            "{}\u{1f}{}",
            self.artist.to_lowercase(),
            self.title.to_lowercase()
        )
    }
}

/// `org.mpris.MediaPlayer2.chromium.instance123` → `chromium`.
pub fn base_name(bus: &str) -> String {
    let rest = bus.trim_start_matches(MPRIS_PREFIX);
    rest.split('.').next().unwrap_or(rest).to_string()
}

/// Publish the current tracks (playing ones first) into `tx` until the
/// receiver goes away.
pub async fn watch_players(tx: watch::Sender<Vec<Track>>) {
    loop {
        match zbus::Connection::session().await {
            Ok(conn) => {
                if let Err(e) = run(&conn, &tx).await {
                    tracing::warn!("mpris watcher stopped: {e}");
                }
            }
            Err(e) => tracing::warn!("no session bus: {e}"),
        }
        if tx.is_closed() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run(conn: &zbus::Connection, tx: &watch::Sender<Vec<Track>>) -> zbus::Result<()> {
    let props = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path(PATH)?
        .build();
    let seeked = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.mpris.MediaPlayer2.Player")?
        .member("Seeked")?
        .build();
    let owners = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .build();
    let mut signals = futures::stream::select_all([
        zbus::MessageStream::for_match_rule(props, conn, Some(64)).await?,
        zbus::MessageStream::for_match_rule(seeked, conn, Some(64)).await?,
        zbus::MessageStream::for_match_rule(owners, conn, Some(64)).await?,
    ]);

    let mut poll = tokio::time::interval(FALLBACK_POLL);
    loop {
        tokio::select! {
            msg = signals.next() => {
                if msg.is_none() {
                    return Ok(());
                }
                // Players send bursts of changes; read once they settle.
                tokio::time::sleep(SETTLE).await;
                while let Some(Some(_)) = futures::FutureExt::now_or_never(signals.next()) {}
            }
            _ = poll.tick() => {}
        }
        let tracks = read_all(conn).await.unwrap_or_default();
        tx.send_if_modified(|cur| {
            if *cur != tracks {
                *cur = tracks;
                true
            } else {
                false
            }
        });
        if tx.is_closed() {
            return Ok(());
        }
    }
}

async fn read_all(conn: &zbus::Connection) -> zbus::Result<Vec<Track>> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    let mut tracks = Vec::new();
    for name in dbus.list_names().await? {
        let name = name.to_string();
        // playerctld mirrors other players; it would double everything.
        if !name.starts_with(MPRIS_PREFIX) || name.contains("playerctld") {
            continue;
        }
        match read_player(conn, &name).await {
            Ok(Some(t)) => tracks.push(t),
            Ok(None) => {}
            Err(e) => tracing::debug!("reading {name}: {e}"),
        }
    }
    tracks.sort_by(|a, b| b.playing.cmp(&a.playing).then(a.player.cmp(&b.player)));
    Ok(tracks)
}

async fn read_player(conn: &zbus::Connection, bus: &str) -> zbus::Result<Option<Track>> {
    let proxy: zbus::Proxy = zbus::proxy::Builder::new(conn)
        .destination(bus.to_owned())?
        .path(PATH)?
        .interface("org.mpris.MediaPlayer2.Player")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await?;
    let status: String = proxy.get_property("PlaybackStatus").await?;
    if status == "Stopped" {
        return Ok(None);
    }
    let meta: HashMap<String, OwnedValue> = proxy.get_property("Metadata").await?;
    let title = string(meta.get("xesam:title")).unwrap_or_default();
    let artist = meta
        .get("xesam:artist")
        .and_then(|v| {
            Vec::<String>::try_from(v.try_clone().ok()?)
                .ok()
                .map(|a| a.join(", "))
                .or_else(|| string(Some(v)))
        })
        .unwrap_or_default();
    if title.trim().is_empty() && artist.trim().is_empty() {
        return Ok(None);
    }
    let length = meta.get("mpris:length").and_then(micros);
    let position = proxy
        .get_property::<i64>("Position")
        .await
        .ok()
        .filter(|p| *p >= 0)
        .map(|p| Duration::from_micros(p as u64));
    let track_id = meta.get("mpris:trackid").and_then(|v| {
        zbus::zvariant::OwnedObjectPath::try_from(v.try_clone().ok()?)
            .ok()
            .map(|p| p.to_string())
            .or_else(|| string(Some(v)))
    });
    Ok(Some(Track {
        player: base_name(bus),
        title: title.trim().to_string(),
        artist: artist.trim().to_string(),
        album: string(meta.get("xesam:album")).filter(|s| !s.trim().is_empty()),
        length,
        position,
        url: string(meta.get("xesam:url")),
        track_id,
        art_url: string(meta.get("mpris:artUrl")),
        playing: status == "Playing",
    }))
}

fn string(v: Option<&OwnedValue>) -> Option<String> {
    String::try_from(v?.try_clone().ok()?).ok()
}

fn micros(v: &OwnedValue) -> Option<Duration> {
    let n = i64::try_from(v.try_clone().ok()?)
        .ok()
        .or_else(|| u64::try_from(v.try_clone().ok()?).ok().map(|u| u as i64))?;
    (n > 0).then(|| Duration::from_micros(n as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_names() {
        assert_eq!(base_name("org.mpris.MediaPlayer2.spotify"), "spotify");
        assert_eq!(
            base_name("org.mpris.MediaPlayer2.chromium.instance4521"),
            "chromium"
        );
    }
}
