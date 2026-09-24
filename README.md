# Opal

A modular Nostr suite for [Omarchy](https://omarchy.org): one daemon, one bar
icon, and features you switch on as you need them.

| Module | Status | What it does |
|---|---|---|
| **Signer** | working (P1) | NIP-46 remote signer ("bunker"). Your nsec lives in the system keyring, encrypted with your passphrase; apps log in with `bunker://` or `nostrconnect://` and you approve what they may do. Inspired by [Amber](https://github.com/greenart7c3/Amber). |
| **Notifications** | working (P2) | Replies, mentions, reposts, reactions, zaps and NIP-17 DMs in the bar and as desktop notifications, from your NIP-65 relays, honoring your NIP-51 mute list (private entries too, when unlocked). Works read-only for any npub. Inspired by [Omastr](https://github.com/barrydeen/omastr). |
| **Status** | working (P3) | NIP-38 statuses: what you're playing in any MPRIS player, a status you set (with expiry), and automatic ones (calendar via khal, away while locked, focus during Do Not Disturb). Local listening history, optionally published as kind 1073 scrobbles ([draft NIP](docs/nip-scrobble.md)). Signs with your local key or an external bunker. Grew out of [noscrobble](https://github.com/derekross/noscrobble). |

## Security model

- Account keys are stored in the Secret Service (gnome-keyring on Omarchy)
  **only as NIP-49 `ncryptsec`**. Omarchy's default keyring has no password,
  so the keyring alone is never enough to sign anything.
- One Opal passphrase unlocks every account. Keys are held in memory only
  while unlocked and are wiped on lock; the vault locks after a timeout and
  when the screen locks.
- Each app connection gets its own NIP-46 key and single-use secret, so apps
  do not learn your npub until they ask for it, and a leaked `bunker://` URI
  cannot be reused.

## Install

```sh
./dist/install.sh
```

This builds `opald` and `opal`, installs them to `~/.local/bin`, enables the
`opal.service` systemd user unit, registers the `nostrconnect://` link
handler, and adds the Opal gem to the Omarchy bar. Click it to import or
create your key.

Useful commands:

```sh
opal status                  # lock state, accounts, modules
opal bunker --qr             # a single-use bunker:// login for an app
opal connect 'nostrconnect://…'
opal prompts / opal approve <id> --remember 1h
opal set-status "At Nostrville" --for 4h
opal plays                   # now playing and recent listens
opal watch name@domain       # read-only notifications for anyone
omarchy-shell opal panel     # toggle the panel (bind it to a key)
omarchy-shell opal lock
```

## Layout

```
crates/opal-core     config, key import, keyring store, vault
crates/opal-signer   NIP-46 signer (protocol, URIs, permissions, request loop)
crates/opal-notify   notifications (classification, relay engine, store, links)
crates/opal-status   statuses and scrobbles (MPRIS, music tracker, auto statuses)
docs/nip-scrobble.md draft NIP for kind 1073 scrobbles
crates/opald         daemon (systemd user service)
crates/opal-cli      `opal` command
shell-plugin         Omarchy shell plugin (bar gem, panel, approval dialog)
dist                 systemd unit, link handler, install script
tests/interop        nostr-tools BunkerSigner against our signer
```

## Development

```sh
cargo test                                             # unit + end-to-end tests
cargo test -p opal-core --test keyring -- --ignored    # real Secret Service
(cd tests/interop && npm install && npm test)          # nostr-tools interop
./dist/dev-plugin.sh                                   # sync the shell plugin and reload it
```

The plugin's service is `keepLoaded`, so changes to `OpalService.qml` need
`omarchy-restart-shell`.

## License

MIT
