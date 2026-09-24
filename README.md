# Opal

A modular Nostr suite for [Omarchy](https://omarchy.org): one daemon, one bar
icon, and features you switch on as you need them.

| Module | Status | What it does |
|---|---|---|
| **Signer** | in progress (P1) | NIP-46 remote signer ("bunker"). Your nsec lives in the system keyring, encrypted with your passphrase; apps log in with `bunker://` or `nostrconnect://` and you approve what they may do. Inspired by [Amber](https://github.com/greenart7c3/Amber). |
| **Notifications** | planned (P2) | Live mentions, replies, reactions and zaps in the bar and as desktop notifications. Inspired by [Omastr](https://github.com/barrydeen/omastr). |
| **Status** | planned (P3) | NIP-38 statuses: now-playing music from MPRIS, manual and automatic statuses, scrobble history. Grew out of [noscrobble](https://github.com/derekross/noscrobble). |

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

## Layout

```
crates/opal-core     config, key import, keyring store, vault
crates/opal-signer   NIP-46 signer (protocol, URIs, permissions, request loop)
crates/opald         daemon (systemd user service)
crates/opal-cli      `opal` command
tests/interop        nostr-tools BunkerSigner against our signer
```

## Development

```sh
cargo test                                             # unit + end-to-end tests
cargo test -p opal-core --test keyring -- --ignored    # real Secret Service
(cd tests/interop && npm install && npm test)          # nostr-tools interop
```

## License

MIT
