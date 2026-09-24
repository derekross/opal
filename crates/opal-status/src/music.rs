//! Deciding when to publish, refresh or clear the music status, and when a
//! play counts as a scrobble. Pure logic; time is passed in.

use std::time::{Duration, Instant};

use crate::mpris::Track;

/// Wait this long on a new track before publishing (skipping through tracks
/// shouldn't spam relays).
pub const DEBOUNCE: Duration = Duration::from_secs(3);
/// Status lifetime when the player doesn't report a length.
pub const FALLBACK_TTL: u64 = 600;
/// Republish when the expected end moved by more than this (seek, resume).
const DRIFT: u64 = 30;
/// Last.fm rule: a play counts after half the track or four minutes…
const SCROBBLE_CAP: Duration = Duration::from_secs(240);
/// …for tracks at least this long.
const SCROBBLE_MIN_LEN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Publish {
        content: String,
        link: Option<String>,
        expires_at: u64,
    },
    Clear,
    Scrobble {
        track: Track,
        played_at: u64,
    },
}

/// Which players to listen to.
#[derive(Debug, Clone, Default)]
pub struct PlayerFilter {
    pub allow: Vec<String>,
    pub block: Vec<String>,
}

impl PlayerFilter {
    pub fn accepts(&self, player: &str) -> bool {
        let p = player.to_lowercase();
        let hit = |list: &[String]| list.iter().any(|x| x.to_lowercase() == p);
        !hit(&self.block) && (self.allow.is_empty() || hit(&self.allow))
    }
}

struct Current {
    key: String,
    started_unix: u64,
    listened: Duration,
    last_tick: Option<Instant>,
    scrobbled: bool,
}

struct Published {
    key: String,
    expires_at: u64,
}

#[derive(Default)]
pub struct MusicTracker {
    current: Option<Current>,
    published: Option<Published>,
    pending: Option<(String, Instant)>,
}

impl MusicTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The key of the track the status currently shows.
    pub fn published_key(&self) -> Option<&str> {
        self.published.as_ref().map(|p| p.key.as_str())
    }

    /// Feed the latest player state (and call it periodically, e.g. every
    /// second, so debounce and listening time advance).
    pub fn update(
        &mut self,
        tracks: &[Track],
        filter: &PlayerFilter,
        link_style: &str,
        now: Instant,
        now_unix: u64,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        let chosen = tracks
            .iter()
            .find(|t| t.playing && filter.accepts(&t.player));

        // Listening time for scrobbles.
        match (chosen, self.current.as_mut()) {
            (Some(t), Some(cur)) if cur.key == t.key() => {
                if let Some(last) = cur.last_tick {
                    cur.listened += now.saturating_duration_since(last);
                }
                cur.last_tick = Some(now);
            }
            (Some(t), _) => {
                self.current = Some(Current {
                    key: t.key(),
                    started_unix: now_unix,
                    listened: Duration::ZERO,
                    last_tick: Some(now),
                    scrobbled: false,
                });
            }
            (None, Some(cur)) => cur.last_tick = None, // paused: stop counting
            (None, None) => {}
        }
        if let (Some(t), Some(cur)) = (chosen, self.current.as_mut())
            && !cur.scrobbled
            && counts_as_play(t.length, cur.listened)
        {
            cur.scrobbled = true;
            out.push(Action::Scrobble {
                track: t.clone(),
                played_at: cur.started_unix,
            });
        }

        // The status itself.
        let Some(t) = chosen else {
            self.pending = None;
            if self.published.take().is_some() {
                out.push(Action::Clear);
            }
            return out;
        };
        let key = t.key();
        let expires_at = expiry(t, now_unix);
        match &self.published {
            Some(p) if p.key == key => {
                // Same song: refresh only when the end moved (seek, resume).
                if p.expires_at.abs_diff(expires_at) > DRIFT {
                    out.push(publish(t, link_style, expires_at));
                    self.published = Some(Published { key, expires_at });
                }
                self.pending = None;
            }
            _ => {
                let since = match &self.pending {
                    Some((k, since)) if *k == key => *since,
                    _ => {
                        self.pending = Some((key.clone(), now));
                        now
                    }
                };
                if now.saturating_duration_since(since) >= DEBOUNCE {
                    out.push(publish(t, link_style, expires_at));
                    self.published = Some(Published { key, expires_at });
                    self.pending = None;
                }
            }
        }
        out
    }

    /// Forget what was published (e.g. after a failed publish) so the next
    /// update tries again.
    pub fn forget_published(&mut self) {
        self.published = None;
    }
}

fn counts_as_play(length: Option<Duration>, listened: Duration) -> bool {
    match length {
        Some(len) if len < SCROBBLE_MIN_LEN => false,
        Some(len) => listened >= (len / 2).min(SCROBBLE_CAP),
        None => listened >= SCROBBLE_CAP,
    }
}

fn expiry(t: &Track, now_unix: u64) -> u64 {
    match (t.length, t.position) {
        (Some(len), Some(pos)) if pos < len => now_unix + (len - pos).as_secs() + 5,
        (Some(len), _) => now_unix + len.as_secs() + 5,
        _ => now_unix + FALLBACK_TTL,
    }
}

fn publish(t: &Track, link_style: &str, expires_at: u64) -> Action {
    Action::Publish {
        content: t.display(),
        link: link_for(t, link_style),
        expires_at,
    }
}

/// The `r` link on a music status.
pub fn link_for(t: &Track, style: &str) -> Option<String> {
    let query = urlencoding::encode(&t.display()).into_owned();
    match style {
        "none" => None,
        "youtube-music" => Some(format!("https://music.youtube.com/search?q={query}")),
        "spotify" => Some(format!("https://open.spotify.com/search/{query}")),
        _ => spotify_track(t)
            .or_else(|| t.url.clone().filter(|u| u.starts_with("https://")))
            .or_else(|| Some(format!("https://music.youtube.com/search?q={query}"))),
    }
}

fn spotify_track(t: &Track) -> Option<String> {
    let id = t.track_id.as_deref()?;
    let id = id
        .strip_prefix("/com/spotify/track/")
        .or_else(|| id.strip_prefix("spotify:track:"))?;
    id.chars()
        .all(|c| c.is_ascii_alphanumeric())
        .then(|| format!("https://open.spotify.com/track/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, playing: bool, len: Option<u64>, pos: Option<u64>) -> Track {
        Track {
            player: "spotify".into(),
            title: title.into(),
            artist: "Artist".into(),
            album: None,
            length: len.map(Duration::from_secs),
            position: pos.map(Duration::from_secs),
            url: None,
            track_id: None,
            art_url: None,
            playing,
        }
    }

    struct Clock {
        start: Instant,
        secs: u64,
    }
    impl Clock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                secs: 0,
            }
        }
        fn at(&mut self, secs: u64) -> (Instant, u64) {
            self.secs = secs;
            (self.start + Duration::from_secs(secs), 1_000_000 + secs)
        }
    }

    fn step(m: &mut MusicTracker, c: &mut Clock, t: &[Track], secs: u64) -> Vec<Action> {
        let (now, unix) = c.at(secs);
        m.update(t, &PlayerFilter::default(), "auto", now, unix)
    }

    #[test]
    fn debounces_skips_and_publishes_with_expiry() {
        let (mut m, mut c) = (MusicTracker::new(), Clock::new());
        let a = [track("A", true, Some(200), Some(0))];
        let b = [track("B", true, Some(180), Some(0))];
        assert!(step(&mut m, &mut c, &a, 0).is_empty());
        assert!(step(&mut m, &mut c, &b, 2).is_empty(), "skipped before 3s");
        assert!(step(&mut m, &mut c, &b, 4).is_empty());
        let out = step(&mut m, &mut c, &b, 5);
        assert_eq!(
            out,
            vec![Action::Publish {
                content: "Artist - B".into(),
                link: Some("https://music.youtube.com/search?q=Artist%20-%20B".into()),
                expires_at: 1_000_005 + 180 + 5,
            }]
        );
        assert!(
            step(&mut m, &mut c, &[track("B", true, Some(180), Some(1))], 6).is_empty(),
            "no repeat"
        );
    }

    #[test]
    fn pause_clears_and_resume_republishes() {
        let (mut m, mut c) = (MusicTracker::new(), Clock::new());
        let playing = [track("A", true, Some(200), Some(10))];
        step(&mut m, &mut c, &playing, 0);
        assert!(matches!(
            step(&mut m, &mut c, &playing, 3)[..],
            [Action::Publish { .. }]
        ));
        let paused = [track("A", false, Some(200), Some(12))];
        assert_eq!(step(&mut m, &mut c, &paused, 5), vec![Action::Clear]);
        assert!(step(&mut m, &mut c, &paused, 6).is_empty());
        step(&mut m, &mut c, &[track("A", true, Some(200), Some(12))], 60);
        assert!(matches!(
            step(&mut m, &mut c, &[track("A", true, Some(200), Some(15))], 63)[..],
            [Action::Publish { .. }]
        ));
    }

    #[test]
    fn seek_refreshes_expiry() {
        let (mut m, mut c) = (MusicTracker::new(), Clock::new());
        step(&mut m, &mut c, &[track("A", true, Some(300), Some(0))], 0);
        step(&mut m, &mut c, &[track("A", true, Some(300), Some(3))], 3);
        // Jump to 250s into a 300s track: it now ends ~250s earlier.
        let out = step(
            &mut m,
            &mut c,
            &[track("A", true, Some(300), Some(250))],
            10,
        );
        assert!(
            matches!(out[..], [Action::Publish { expires_at, .. }] if expires_at == 1_000_010 + 50 + 5)
        );
    }

    #[test]
    fn scrobbles_after_half_the_track_counting_only_playing_time() {
        let (mut m, mut c) = (MusicTracker::new(), Clock::new());
        let playing = |pos| [track("A", true, Some(100), Some(pos))];
        let paused = [track("A", false, Some(100), Some(20))];
        step(&mut m, &mut c, &playing(0), 0);
        step(&mut m, &mut c, &playing(20), 20);
        step(&mut m, &mut c, &paused, 20);
        step(&mut m, &mut c, &paused, 500); // paused time doesn't count
        step(&mut m, &mut c, &playing(20), 500);
        let before: Vec<_> = step(&mut m, &mut c, &playing(45), 525)
            .into_iter()
            .filter(|a| matches!(a, Action::Scrobble { .. }))
            .collect();
        assert!(before.is_empty(), "45s of 100s");
        let out = step(&mut m, &mut c, &playing(51), 531);
        assert!(
            out.iter().any(
                |a| matches!(a, Action::Scrobble { played_at, .. } if *played_at == 1_000_000)
            )
        );
        let again = step(&mut m, &mut c, &playing(90), 570);
        assert!(
            !again.iter().any(|a| matches!(a, Action::Scrobble { .. })),
            "once per play"
        );
    }

    #[test]
    fn short_tracks_never_scrobble() {
        let (mut m, mut c) = (MusicTracker::new(), Clock::new());
        let jingle = [track("Jingle", true, Some(20), Some(0))];
        for s in 0..25 {
            assert!(
                !step(&mut m, &mut c, &jingle, s)
                    .iter()
                    .any(|a| matches!(a, Action::Scrobble { .. }))
            );
        }
    }

    #[test]
    fn filters_players() {
        let f = PlayerFilter {
            allow: vec![],
            block: vec!["Chromium".into()],
        };
        assert!(!f.accepts("chromium"));
        assert!(f.accepts("spotify"));
        let only = PlayerFilter {
            allow: vec!["spotify".into()],
            block: vec![],
        };
        assert!(!only.accepts("vlc"));
    }

    #[test]
    fn links() {
        let mut t = track("Song", true, None, None);
        t.track_id = Some("/com/spotify/track/4uLU6hMCjMI75M1A2tKUQC".into());
        assert_eq!(
            link_for(&t, "auto").unwrap(),
            "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"
        );
        t.track_id = None;
        t.url = Some("https://music.youtube.com/watch?v=abc".into());
        assert_eq!(
            link_for(&t, "auto").unwrap(),
            "https://music.youtube.com/watch?v=abc"
        );
        t.url = Some("file:///home/me/song.mp3".into());
        assert!(
            link_for(&t, "auto")
                .unwrap()
                .starts_with("https://music.youtube.com/search")
        );
        assert_eq!(link_for(&t, "none"), None);
    }
}
