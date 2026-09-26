# Opal

A Nostr suite for [Omarchy](https://omarchy.org), in your bar. One small
daemon, one gem icon, and modules you switch on as you need them:

| Module | What it does |
|---|---|
| **Signer** | A NIP-46 remote signer ("bunker"). Your nsec stays in the system keyring, encrypted with your passphrase. Web and desktop apps log in with a `bunker://` link or a `nostrconnect://` QR/link, and you decide what each one may do. Inspired by [Amber](https://github.com/greenart7c3/Amber). |
| **Notifications** | Replies, mentions, reposts, reactions, zaps and NIP-17 DMs in an Inbox and as desktop popups, from your NIP-65 relays, honoring your NIP-51 mute list (private entries too). Inspired by [Omastr](https://github.com/barrydeen/omastr). |
| **Status** | NIP-38 statuses: what you're playing in any MPRIS player, a status you set, and automatic ones (calendar, away, focus). Optional scrobbles as kind 1073 ([draft NIP](docs/nip-scrobble.md)). Grew out of [noscrobble](https://github.com/derekross/noscrobble). |

## Features

### Profiles
- Several keys, plus **read-only profiles**: watch anyone's notifications with an npub or a NIP-05 address (`you@example.com`), no key needed.
- Import an nsec, hex key, `ncryptsec` (NIP-49) or recovery phrase (NIP-06), or create a new key.
- Copy your npub or an encrypted backup (`ncryptsec`) from the panel.
- Or sign through an **external signer** (e.g. Amber on your phone) by pasting its `bunker://` link.

### Signer (NIP-46)
- `bunker://` links (single-use, expire after an hour unused, QR code in the panel) and `nostrconnect://` links (click one anywhere; Opal asks you first).
- Every NIP-46 method: `connect`, `get_public_key`, `sign_event`, NIP-04/NIP-44 encrypt and decrypt, `ping`, `switch_relays`, `logout`.
- Each app gets its own key, so apps don't learn your npub until they ask, and one app's relay can't link your other apps.
- Per-app policy: **Basic** (everyday actions like notes, reactions and reposts are signed automatically), **Ask** for everything, or **Trust**.
- An approval dialog shows exactly what will be signed (content, key tags, odd dates), with "remember" choices from once to always.
- Profile, follow-list, relay-list and mute-list updates, deletions, HTTP/Blossom auth tokens and wallet events always ask, and can be remembered for an hour at most. So does anything dated more than 10 minutes from now.
- Activity log of what each app did and why it was allowed or denied (saved rule, basic policy, you…), with a privacy mode.
- Kill switch to stop answering every app at once.
- **Local apps** (programs on this computer, like [Peridot](https://github.com/derekross/peridot)) can use your key too, but only after you pair them in the same dialog, which shows which program is asking. They appear under Apps with the same policies, saved answers and activity log, and can be revoked there. Blossom and relay-auth kinds still ask every time. See [Local apps](#local-apps).

### Notifications
- Inbox with filters (all, replies, zaps, DMs), unread dot on the gem, context of the note being replied to or reacted to.
- Desktop popups with avatars; clicking opens the note in your client of choice (Primal, Jumble, Coracle, Ditto, njump).
- Zaps are checked (the zap request must be signed and addressed to you, and the amount must match the invoice) before they're shown.
- DMs are decrypted with your key while Opal is unlocked; ones that arrive while locked wait until you unlock. Message text stays out of popups and storage unless you turn previews on.

### Status (NIP-38)
- Now playing from Spotify, browsers, mpv or any MPRIS player, with a link to the song, cleared when you pause. Choose which players to ignore.
- Set a status with an optional link and an expiry (1h, 4h, today, until cleared).
- Automatic statuses you can switch on: "In a meeting" during khal events, "Away" while the screen is locked, "Focusing" during Do Not Disturb.
- A local listening history with top artists; optionally published as kind 1073 scrobbles.

## Security

Opal holds your nsec, so it's built to be careful:

- **Encrypted at rest.** Keys go into the Secret Service (gnome-keyring) *only* as NIP-49 `ncryptsec` (scrypt, N=2^18). Omarchy's keyring has no password of its own, so your Opal passphrase is what protects the key. New passphrases must pass a strength check.
- **In memory only while unlocked.** Keys are wiped when Opal locks: after a timeout you choose, when the screen locks, and before suspend. Only your own approvals count as activity; apps can't keep it unlocked.
- **No leaks from the process.** Core dumps are off and other processes of your user can't read the daemon's memory.
- **Local control socket** (`$XDG_RUNTIME_DIR/opal.sock`) is `0600`; the daemon checks the caller's user id and records which program connected. Any program running as you can reach it, so reaching it grants nothing by itself: signing needs a pairing you approved (see [Local apps](#local-apps)), and changes that weaken protection (full trust for an app, permanent allow rules, turning auto-lock off, deleting a key) need your passphrase. Wrong passphrases back off.
- **Remote apps get nothing without a connection**, and only what their policy or you allow once connected. Strangers get no reply at all.
- **Untrusted text is only ever shown as plain text** in the panel and in popups; images are https-only.
- **Hardened systemd unit**: no capabilities, seccomp filter, read-only home except Opal's own directories.

## Requirements

- Omarchy (the Quattro shell with plugins) on Arch
- Optional: Rust, to build the daemon yourself (`sudo pacman -S --needed rustup && rustup default stable`). Without it, the installer downloads release binaries (x86_64 and aarch64).
- A Secret Service provider: gnome-keyring (Omarchy's default)
- Already on Omarchy: `systemd` (user services), `curl`, `jq`, `xdg-utils`
- Optional: `khal` for the calendar status

## Install

Opal is a small daemon plus an Omarchy shell plugin. `omarchy plugin add`
only copies plugin files (it never builds anything), so the daemon is
installed with the included script.

**With the Omarchy plugin command**

```sh
omarchy plugin add https://github.com/derekross/opal.git --enable
~/.config/omarchy/plugins/derekross.opal/dist/install.sh
```

**Or from a clone**

```sh
git clone https://github.com/derekross/opal.git
cd opal && ./dist/install.sh
```

Either way, `install.sh` puts `opald` and `opal` in `~/.local/bin`,
enables the `opal.service` systemd user service, registers the
`nostrconnect://` link handler (asking first if another app has it), and puts
the Opal gem in the bar. Click it to add a key or watch someone.

**Binaries.** With Rust installed, `install.sh` builds from source. Without it,
it downloads the release matching the plugin's version from
[GitHub Releases](https://github.com/derekross/opal/releases). Each release is
built by GitHub Actions from its tag. The installer checks the download against
the release's `SHA256SUMS`, and against its build attestation too when the
GitHub CLI is signed in. Choose explicitly with `install.sh --build` or
`install.sh --prebuilt`. To check a download yourself:
`gh attestation verify opal-v0.3.1-x86_64-linux.tar.gz --repo derekross/opal`.

## Update

```sh
omarchy plugin update derekross.opal
~/.config/omarchy/plugins/derekross.opal/dist/install.sh
```

(From a clone: `git pull && ./dist/install.sh`.) Updating keeps your keys and
settings.

## Remove

```sh
~/.config/omarchy/plugins/derekross.opal/dist/uninstall.sh      # or ./dist/uninstall.sh in a clone
omarchy plugin remove derekross.opal                             # if added with omarchy plugin add
```

This stops and removes the service, binaries, link handler and plugin. Your
keys stay in the keyring (encrypted) along with Opal's settings and history,
so reinstalling picks up where you left off. To delete those too, export a
backup of your keys first, then run `uninstall.sh --purge`.

## Use

Everything is in the panel. From a terminal:

```sh
opal status                          # lock state, profiles, modules
opal unlock / opal lock
opal account add [--generate]        # import (asks for the key) or create
opal bunker --qr                     # single-use bunker:// login for an app
opal connect 'nostrconnect://…'      # hand a link to the panel for approval
opal prompts / opal approve <id> --remember 1h / opal deny <id>
opal apps / opal revoke <id> / opal log
opal inbox [--read]                  # notifications
opal watch derekross@grownostr.org   # read-only profile
opal set-status "At Nostrville" --for 4h / opal clear-status
opal plays                           # now playing and recent listens
opal module notifications on         # turn modules on or off
```

Bind a key to the panel, e.g. in `~/.config/hypr/hyprland.lua`:
`omarchy-shell opal panel` (also `opal notifications`, `opal lock`, `opal approvals`).

## Local apps

A program on this computer can use your key over the control socket, the way
[Peridot](https://github.com/derekross/peridot) does to sign its synced
settings. It never sees the key: it pairs once, then sends a pairing token
with every request, and each request goes through the same policy, saved
answers and prompts as a NIP-46 app.

**Pairing.** The program calls `app.connect` with its id, a display name,
the event kinds it will ask for and whether it needs NIP-44 encryption of
its own data. The approval dialog shows all of that plus where the program
runs (its systemd unit, e.g. `peridot.service`, and its executable path when
that can be read) and the account it would sign as. You pick a
policy and tick what it may always do; sensitive kinds (Blossom and HTTP
auth, relay logins, deletions…) and decryption can't be pre-allowed, so they
ask each time, remembered for an hour at most, unless you give the app full
trust (which needs your passphrase). Pairing again replaces the earlier
pairing: new token, fresh rules.

**Afterwards.** The app is listed under Apps with where it paired from;
change its policy, delete saved answers or revoke it there. Revoking stops
its token at once. The token only works from where the pairing was made:
the same systemd unit, and the same executable where Opal could read it. A
daemon running from a different unit (or a development build run from a
terminal) is told to pair again, and you get a prompt. While Opal is locked,
local apps are refused straight away rather than kept waiting, and nothing
is logged.

Why the unit and not just the path: the daemon runs in a hardened user
service, and from inside one Linux won't let it read `/proc/<pid>/exe` of
another sandboxed, non-dumpable service (that needs ptrace rights over the
peer). The cgroup, and so the unit name, is set by systemd and readable by
anyone.

**What it can't do.** Any program running as your user can open the socket,
so the socket itself grants nothing: it takes a pairing you clicked through,
and then only the declared kinds, under the rules you set. A program that is
already running as you could copy the paired app's token and exec the real
program, so the executable check is a second lock, not a wall; what protects
the key is the prompt, the rules and the activity log.

The methods, over `$XDG_RUNTIME_DIR/opal.sock` (newline-delimited JSON, see
`crates/opal-core/src/ipc.rs`):

```jsonc
{"id":1,"method":"app.connect","params":{"app":"peridot","name":"Peridot","kinds":[30078,22242,24242],"nip44":true,"pubkey":"<hex, optional>"}}
// blocks until you answer (5 minutes at most) → {"token":"<64 hex>","pubkey":"<hex>"}
// or an error: "declined", "timed out", "that app is already waiting for your approval"
{"id":2,"method":"app.sign","params":{"token":"…","event":{"pubkey":"…","created_at":1727300000,"kind":30078,"tags":[],"content":"…"}}}
// → the signed event
{"id":3,"method":"app.nip44","params":{"token":"…","op":"encrypt","content":"…"}}   // op: encrypt | decrypt, to the app's own account
// → {"content":"…"}
{"id":4,"method":"app.status","params":{"token":"…"}}
// → {"pubkey":"…","name":"Peridot","policy":"basic"}
```

Errors apps should expect: `not paired` (unknown or revoked token: pair
again), `paired with a different program (<path>); pair again`, `Opal is
locked`, `<name> didn't declare kind <n> when it paired`, `user rejected`,
`denied by a saved rule`, `timed out waiting for approval` and
`too many pending requests`.

## Layout

```
crates/opal-core     config, key import, keyring store, vault, NIP-05, database
crates/opal-kit      shared runtime: signers, relays and outbox, control socket, QR
crates/opal-signer   NIP-46 signer (protocol, URIs, permissions, request loop)
crates/opal-notify   notifications (classification, relay engine, store, links)
crates/opal-status   statuses and scrobbles (MPRIS, music tracker, auto statuses)
crates/opald         the daemon: socket API, modules, locking, desktop popups
crates/opal-cli      the `opal` command
shell-plugin         Omarchy shell plugin (bar gem, panel, approval dialog)
dist                 systemd unit, link handler, install script
.github/workflows    release builds (tag vX.Y.Z → GitHub Release)
docs/nip-scrobble.md draft NIP for kind 1073 scrobbles
tests/interop        nostr-tools BunkerSigner against the signer
```

## Development

```sh
cargo test                                             # unit + end-to-end tests
cargo test -p opal-core --test keyring -- --ignored    # real Secret Service
(cd tests/interop && npm install && npm test)          # nostr-tools interop
./dist/dev-plugin.sh                                   # sync the shell plugin and reload it
```

To release, bump the version in `manifest.json` and `Cargo.toml`, commit, then
`git tag vX.Y.Z && git push origin vX.Y.Z`. The workflow checks that all three
match, runs the tests, and publishes the binaries.

The plugin's service is `keepLoaded`, so changes to `OpalService.qml` need
`omarchy-restart-shell`. For experiments, run a separate daemon that can't
touch your keys: `opald --memory-keyring --socket /tmp/x.sock --db /tmp/x.db --config /tmp/x.toml`.

## License

MIT
