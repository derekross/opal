NIP-XX
======

Scrobbles
---------

`draft` `optional`

This NIP defines kind `1073`, a **scrobble**: a public record that the author
listened to a piece of music. Where [NIP-38](38.md) `d=music` statuses say what
is playing *right now* and expire, scrobbles are a lasting listening history
that clients can count, chart and recommend from, the way Last.fm and
ListenBrainz do.

## Event

A regular (non-replaceable) event, published once per listen.

```jsonc
{
  "kind": 1073,
  "created_at": 1790284624, // when the listen started
  "content": "",
  "tags": [
    ["title", "Schism"],
    ["artist", "Tool"],
    ["album", "Lateralus"],
    ["duration", "407"],                                   // seconds
    ["r", "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"],
    ["i", "isrc:USVJ20100069"],                            // optional, NIP-73
    ["a", "36787:<pubkey>:<d-tag>"],                       // optional, a Nostr-native track
    ["alt", "Listened to Tool - Schism"]
  ]
}
```

### Tags

| Tag | Required | Meaning |
|---|---|---|
| `title` | yes | Track title. |
| `artist` | yes, one or more | Performer. Use one tag per artist. |
| `album` | no | Album or release title. |
| `duration` | no | Track length in whole seconds. |
| `r` | no | A web page for the track (streaming service, store, artist site). |
| `i` | no | External identifiers per [NIP-73](73.md), e.g. `isrc:<code>`. |
| `a` | no | A Nostr-native track (kind `36787`) that was played. |
| `alt` | recommended | [NIP-31](31.md) human-readable summary. |

`content` SHOULD be empty. Clients MAY use it for a short comment about the
listen.

## When to publish

A listen counts once the track has played for **half its length or four
minutes, whichever comes first**, counting only time spent playing (not
paused). Tracks shorter than 30 seconds are not scrobbled. This matches the
long-standing Last.fm rule, so histories are comparable.

`created_at` is the time the listen **started**. Clients that buffer
scrobbles (for example while offline or while a signer is locked) publish
them later with the original start time.

Scrobbling reveals a lot about a person. Publishers SHOULD make it opt-in and
keep it separate from the live NIP-38 status.

## Reading

To show someone's listening:

```json
{ "kinds": [1073], "authors": ["<pubkey>"], "limit": 100 }
```

Aggregate by `artist` / `title` tags for charts. Because events are regular,
deletion ([NIP-09](09.md)) removes individual listens.

## Implementations

- [Opal](https://github.com/derekross/opal) (publisher; Omarchy/Linux, MPRIS)
