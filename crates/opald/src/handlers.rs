//! The desktop's apps for Nostr links: which installed apps open `nostr:`
//! or `web+nostr:` links, which one is the default, and changing it.
//!
//! Native Nostr apps register `nostr:`. Installed web apps (Ditto, Coracle,
//! noStrudel) can only register `web+nostr:`, because browsers drop schemes
//! that aren't on the HTML spec's safelist. Opal reads the `.desktop` files
//! for both and leaves the choice to the desktop's own default (xdg-mime),
//! so a pick in Opal applies to every other app's Nostr links too.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use opal_core::config::NotificationsConfig;
use opal_notify::links::{self, Opener};
use serde::Serialize;

pub const NOSTR: &str = "nostr";
pub const WEB_NOSTR: &str = "web+nostr";

/// An installed app that opens Nostr links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NostrApp {
    /// Desktop id, e.g. `chrome-<appid>-Default.desktop`.
    pub id: String,
    pub name: String,
    #[serde(skip)]
    nostr: bool,
    #[serde(skip)]
    web_nostr: bool,
}

impl NostrApp {
    fn handles(&self, scheme: &str) -> bool {
        match scheme {
            NOSTR => self.nostr,
            WEB_NOSTR => self.web_nostr,
            _ => false,
        }
    }

    /// The scheme a link for this app uses: `nostr:` when it has it.
    fn scheme(&self) -> &'static str {
        if self.nostr { NOSTR } else { WEB_NOSTR }
    }

    fn schemes(&self) -> impl Iterator<Item = &'static str> + '_ {
        [NOSTR, WEB_NOSTR].into_iter().filter(|s| self.handles(s))
    }
}

/// The installed Nostr apps and where a "Default app" link would open.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub apps: Vec<NostrApp>,
    /// The app and scheme a link goes to; `None` means the njump fallback.
    pub opens_in: Option<(&'static str, NostrApp)>,
}

/// Where "Default app" opens, given the installed apps, the one picked in
/// Settings (desktop id, empty for none) and the desktop's defaults for
/// `nostr:` and `web+nostr:`. A pick wins while it's still installed, then
/// the `nostr:` default, then the `web+nostr:` one. A default only counts
/// when it names an installed app that really handles that scheme.
pub fn resolve(
    apps: &[NostrApp],
    picked: &str,
    default_nostr: Option<&str>,
    default_web: Option<&str>,
) -> Option<(&'static str, NostrApp)> {
    if let Some(app) = apps.iter().find(|a| !picked.is_empty() && a.id == picked) {
        return Some((app.scheme(), app.clone()));
    }
    [(NOSTR, default_nostr), (WEB_NOSTR, default_web)]
        .into_iter()
        .find_map(|(scheme, default)| {
            let id = default?;
            apps.iter()
                .find(|a| a.id == id && a.handles(scheme))
                .map(|a| (scheme, a.clone()))
        })
}

/// How to build notification links for these settings: the web client, or
/// for "Default app" the resolved scheme, else njump.
pub async fn opener(cfg: &NotificationsConfig) -> Opener {
    if cfg.client != links::DEFAULT_APP {
        return Opener::Web(links::client_base(&cfg.client));
    }
    match snapshot(&cfg.default_app).await.opens_in {
        Some((scheme, _)) => Opener::Scheme(scheme),
        None => Opener::Web(links::DEFAULT_APP_FALLBACK),
    }
}

/// How long a snapshot is reused. Links are built for every notification
/// list the panel loads; the desktop's defaults rarely change.
const SNAPSHOT_TTL: Duration = Duration::from_secs(30);

static CACHE: Mutex<Option<(Instant, String, Snapshot)>> = Mutex::new(None);

/// The installed Nostr apps and where links open, for the pick `picked`.
pub async fn snapshot(picked: &str) -> Snapshot {
    if let Some((at, p, snap)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && at.elapsed() < SNAPSHOT_TTL
        && p == picked
    {
        return snap.clone();
    }
    let apps = tokio::task::spawn_blocking(scan).await.unwrap_or_default();
    let snap = if apps.is_empty() {
        Snapshot::default()
    } else {
        let (nostr, web) = tokio::join!(query_default(NOSTR), query_default(WEB_NOSTR));
        let opens_in = resolve(&apps, picked, nostr.as_deref(), web.as_deref());
        Snapshot { apps, opens_in }
    };
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), picked.to_string(), snap.clone()));
    snap
}

/// Make `id` the desktop's default for every Nostr scheme it handles. `id`
/// must be one of the installed Nostr apps; anything else is refused.
pub async fn set_default(id: &str) -> Result<NostrApp> {
    let apps = tokio::task::spawn_blocking(scan).await.unwrap_or_default();
    let Some(app) = apps.into_iter().find(|a| a.id == id) else {
        bail!("that app doesn't open Nostr links");
    };
    for scheme in app.schemes() {
        // Outside opald's sandbox: its home is read-only, and mimeapps.list
        // lives there.
        let run = tokio::process::Command::new("systemd-run")
            .args(["--user", "--quiet", "--collect", "--wait", "--"])
            .args(["xdg-mime", "default", &app.id])
            .arg(format!("x-scheme-handler/{scheme}"))
            .kill_on_drop(true)
            .status();
        match tokio::time::timeout(Duration::from_secs(10), run).await {
            Ok(Ok(status)) if status.success() => {}
            _ => bail!("couldn't make {} the default for {scheme}: links", app.name),
        }
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(app)
}

/// The desktop id xdg-mime reports as the default for `scheme:` links.
async fn query_default(scheme: &str) -> Option<String> {
    let run = tokio::process::Command::new("xdg-mime")
        .args(["query", "default"])
        .arg(format!("x-scheme-handler/{scheme}"))
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .ok()?
        .ok()?;
    let id = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !id.is_empty()).then_some(id)
}

/// Installed Nostr apps, by name.
fn scan() -> Vec<NostrApp> {
    let mut apps = scan_dirs(&application_dirs());
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// `applications` folders in XDG lookup order: the user's first.
fn application_dirs() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let data_home = var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| Path::new(&h).join(".local/share")));
    let data_dirs = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .into_iter()
        .chain(
            data_dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(PathBuf::from),
        )
        .filter(|d| d.is_absolute())
        .map(|d| d.join("applications"))
        .collect()
}

/// Nostr apps in `dirs`. A desktop id found in an earlier folder hides the
/// same id in later ones, as the desktop entry spec says, even when the
/// earlier file isn't a Nostr app or is `Hidden`.
fn scan_dirs(dirs: &[PathBuf]) -> Vec<NostrApp> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        desktop_files(dir, dir, 0, &mut files);
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(app) = read_entry(&path).and_then(|text| parse(&id, &text)) {
                apps.push(app);
            }
        }
    }
    apps
}

/// `.desktop` files under `dir` with their desktop ids (subfolders become
/// `-`-joined prefixes).
fn desktop_files(root: &Path, dir: &Path, depth: u8, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 3 {
                desktop_files(root, &path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "desktop")
            && let Ok(rel) = path.strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push((rel.replace('/', "-"), path));
        }
    }
}

/// A desktop file's text; oversized files aren't desktop entries.
fn read_entry(path: &Path) -> Option<String> {
    const MAX: u64 = 256 * 1024;
    if std::fs::metadata(path).ok()?.len() > MAX {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// The Nostr app a desktop entry describes, if it is one.
fn parse(id: &str, text: &str) -> Option<NostrApp> {
    let mut in_entry = false;
    let (mut name, mut mime, mut hidden) = (None, "", false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim()),
            "MimeType" => mime = value,
            "Hidden" => hidden = value.trim() == "true",
            _ => {}
        }
    }
    let has = |scheme: &str| {
        let want = format!("x-scheme-handler/{scheme}");
        mime.split(';').any(|m| m.trim() == want)
    };
    let (nostr, web_nostr) = (has(NOSTR), has(WEB_NOSTR));
    if hidden || !(nostr || web_nostr) {
        return None;
    }
    Some(NostrApp {
        id: id.to_string(),
        name: name.filter(|n| !n.is_empty()).unwrap_or(id).to_string(),
        nostr,
        web_nostr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, nostr: bool, web_nostr: bool) -> NostrApp {
        NostrApp {
            id: id.into(),
            name: id.into(),
            nostr,
            web_nostr,
        }
    }

    #[test]
    fn parses_desktop_entries() {
        let ditto = "[Desktop Entry]\nName=Ditto\nExec=chromium --app-id=x %U\n\
            MimeType=x-scheme-handler/bitcoin;x-scheme-handler/web+nostr;\n";
        let got = parse("chrome-x-Default.desktop", ditto).unwrap();
        assert_eq!(got.name, "Ditto");
        assert!(got.web_nostr && !got.nostr);
        assert_eq!(got.scheme(), WEB_NOSTR);

        let native = "[Desktop Entry]\nName=Gossip\nMimeType=x-scheme-handler/nostr;x-scheme-handler/web+nostr\n";
        let got = parse("gossip.desktop", native).unwrap();
        assert!(got.nostr && got.web_nostr);
        assert_eq!(got.scheme(), NOSTR);

        // Not a Nostr app; a prefix match isn't a match.
        assert!(
            parse(
                "a.desktop",
                "[Desktop Entry]\nName=A\nMimeType=x-scheme-handler/nostrconnect;\n"
            )
            .is_none()
        );
        // Hidden entries are uninstalled ones.
        assert!(
            parse(
                "b.desktop",
                "[Desktop Entry]\nName=B\nHidden=true\nMimeType=x-scheme-handler/nostr;\n"
            )
            .is_none()
        );
        // Keys in other groups (actions) don't count.
        assert!(
            parse(
                "c.desktop",
                "[Desktop Entry]\nName=C\n[Desktop Action x]\nMimeType=x-scheme-handler/nostr;\n"
            )
            .is_none()
        );
        // No name: the desktop id stands in.
        assert_eq!(
            parse(
                "d.desktop",
                "[Desktop Entry]\nMimeType=x-scheme-handler/nostr;\n"
            )
            .unwrap()
            .name,
            "d.desktop"
        );
    }

    #[test]
    fn scans_folders_in_order() {
        let tmp = std::env::temp_dir().join(format!("opal-handlers-{}", std::process::id()));
        let (user, system) = (tmp.join("user"), tmp.join("system"));
        std::fs::create_dir_all(user.join("vendor")).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let entry = |name: &str| {
            format!("[Desktop Entry]\nName={name}\nMimeType=x-scheme-handler/nostr;\n")
        };
        std::fs::write(user.join("vendor/app.desktop"), entry("Sub")).unwrap();
        // The user's non-Nostr copy hides the system's Nostr one.
        std::fs::write(user.join("shadow.desktop"), "[Desktop Entry]\nName=Mine\n").unwrap();
        std::fs::write(system.join("shadow.desktop"), entry("System")).unwrap();
        std::fs::write(system.join("other.desktop"), entry("Other")).unwrap();

        let mut ids: Vec<_> = scan_dirs(&[user, system])
            .into_iter()
            .map(|a| a.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["other.desktop", "vendor-app.desktop"]);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn resolves_where_links_open() {
        let apps = [
            app("ditto.desktop", false, true),
            app("gossip.desktop", true, false),
        ];
        let at = |r: Option<(&'static str, NostrApp)>| r.map(|(s, a)| (s, a.id));

        // A pick wins, with the scheme it handles.
        assert_eq!(
            at(resolve(
                &apps,
                "ditto.desktop",
                Some("gossip.desktop"),
                None
            )),
            Some((WEB_NOSTR, "ditto.desktop".into()))
        );
        // A pick that's no longer installed falls through to the defaults.
        assert_eq!(
            at(resolve(
                &apps,
                "gone.desktop",
                Some("gossip.desktop"),
                Some("ditto.desktop")
            )),
            Some((NOSTR, "gossip.desktop".into()))
        );
        // nostr: before web+nostr:.
        assert_eq!(
            at(resolve(
                &apps,
                "",
                Some("gossip.desktop"),
                Some("ditto.desktop")
            )),
            Some((NOSTR, "gossip.desktop".into()))
        );
        assert_eq!(
            at(resolve(&apps, "", None, Some("ditto.desktop"))),
            Some((WEB_NOSTR, "ditto.desktop".into()))
        );
        // A default that isn't an installed Nostr app, or doesn't handle
        // that scheme, doesn't count.
        assert_eq!(
            at(resolve(
                &apps,
                "",
                Some("chromium.desktop"),
                Some("gossip.desktop")
            )),
            None
        );
        assert_eq!(at(resolve(&[], "", None, None)), None);
    }
}
