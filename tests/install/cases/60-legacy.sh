#!/usr/bin/env bash
source "$T/lib.sh"

# A copy as v0.3.x left it: this checkout's files plus the URL-only marker.
v03_copy() { mkdir -p "$P"; cp "$ROOT"/shell-plugin/* "$P/"; rm -f "$P/manifest.json"; (cd "$ROOT" && jq '.entryPoints |= with_entries(.value |= ltrimstr("shell-plugin/"))' manifest.json) >"$P/manifest.json"; printf 'https://github.com/derekross/opal\n' >"$P/.installed-by-opal"; }

case_ "1. a v0.3.x copy (URL marker, no record): replaced, marker removed, your extra file kept"
fresh; v03_copy; echo x >"$P/extra.txt"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "marker gone" [ ! -e "$P/.installed-by-opal" ] && expect "extra kept" [ -f "$P/extra.txt" ] &&
expect "only extra named" [ "$(grep -c keeping "$HOME/out.txt")" = 1 ] && expect "manifest verifies" manifest_verifies && ok

case_ "2. a marker with hash lines (an unreleased build) counts as a record; a marker you edited is yours"
fresh; v03_copy; (cd "$P" && { printf 'https://github.com/derekross/opal\n'; sha256sum OpalService.qml Widget.qml; } >.installed-by-opal); echo "// edited" >>"$P/Widget.qml"; cp "$P/Widget.qml" "$HOME/w"
r=$(inst); expect "exit 0" [ "$r" = 0 ] && expect "edited kept" same "$HOME/w" "$P/Widget.qml" && expect "marker gone" [ ! -e "$P/.installed-by-opal" ] && ok
fresh; v03_copy; echo "my notes" >>"$P/.installed-by-opal"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "edited marker kept" grep -q "my notes" "$P/.installed-by-opal" && said "keeping .installed-by-opal" && ok

case_ "3. v0.2.0 files (no marker at all) are recognised by their release hashes and replaced"
fresh; mkdir -p "$P"; cp "$T"/fixtures/plugin-v0.2.0/* "$P/"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "replaced" same "$ROOT/shell-plugin/Widget.qml" "$P/Widget.qml" && same "$ROOT/shell-plugin/OpalService.qml" "$P/OpalService.qml" &&
expect "manifest replaced" [ "$(jq -r .version "$P/manifest.json")" = "$(jq -r .version "$ROOT/manifest.json")" ] && expect "nothing kept" not said "keeping" && ok

case_ "4. the pre-0.2 'opal' id folder: Opal's files removed, yours kept, disabled only once gone"
fresh; mkdir -p "$OLDP"; cp "$T"/fixtures/plugin-opal-id/* "$OLDP/"; echo k >"$OLDP/keep.txt"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && said "Removing the older 'opal' plugin install" && expect "Opal's gone" [ ! -e "$OLDP/OpalService.qml" ] && [ ! -e "$OLDP/manifest.json" ] &&
expect "yours kept" [ -f "$OLDP/keep.txt" ] && expect "not disabled" not_logged "plugin disable opal" && ok
fresh; mkdir -p "$OLDP"; cp "$T"/fixtures/plugin-opal-id/* "$OLDP/"; r=$(inst)
expect "folder gone" [ ! -e "$OLDP" ] && expect "disabled" logged "plugin disable opal" && ok

case_ "5. a nested .installed-by-opal is an ordinary file of yours"
fresh; inst >/dev/null; mkdir "$P/sub"; printf 'https://github.com/derekross/opal\n' >"$P/sub/.installed-by-opal"; r=$(inst)
expect "kept" [ -f "$P/sub/.installed-by-opal" ] && ok

case_ "6. a record line pointing outside Opal's paths is ignored by uninstall"
fresh; inst >/dev/null; echo secret >"$HOME/.bashrc"; h=$(sha256sum "$HOME/.bashrc" | cut -c1-64); printf '%s\t%s\n' "$h" "$HOME/.bashrc" >>"$M"; r=$(uninst)
expect "exit 0" [ "$r" = 0 ] && expect "bashrc intact" [ "$(cat "$HOME/.bashrc")" = secret ] && said "ignoring a record line for $HOME/.bashrc" && ok

finish
