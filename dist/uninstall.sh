#!/usr/bin/env bash
# Remove Opal: the service, binaries, link handler and shell plugin.
# Your keys (in the keyring) and Opal's data are kept unless you pass --purge.
#
#   ./dist/uninstall.sh           remove the program, keep keys and data
#   ./dist/uninstall.sh --purge   also delete keys, settings and history
set -euo pipefail

cd "$(dirname "$0")/.."
PLUGIN_ID="$(jq -r .id manifest.json 2>/dev/null || echo derekross.opal)"
PLUGINDIR="$HOME/.config/omarchy/plugins"
UNIT="$HOME/.config/systemd/user/opal.service"
LAUNCHER="$HOME/.local/share/applications/opal-nostrconnect.desktop"
MARKER=".installed-by-opal"

# Only what Opal installed is removed (see install.sh).
our_plugin_copy() {
  local dir=$1 id=$2
  [[ -d $dir && ! -L $dir ]] || return 1
  [[ -f $dir/$MARKER ]] && return 0
  [[ ! -e $dir/.git && -f $dir/OpalService.qml ]] \
    && [[ "$(jq -r .id "$dir/manifest.json" 2>/dev/null)" == "$id" ]]
}
# The plugin copy. The marker's first line is Opal's URL; the lines after it
# are `sha256sum` lines for every file this script put there. A file is
# Opal's to replace or remove only while it still matches its line; a file
# that was edited, or added, is yours and stays. Copies from before the
# hash lines (a marker with the URL only, or none) can't be checked file by
# file: the files Opal ships are replaced, anything else is kept.
plugin_hashes() { [[ -f $1/$MARKER ]] || return 0; sed -n '2,$p' "$1/$MARKER"; }
recorded_hash() { plugin_hashes "$1" | awk -v f="$2" '$2 == f { print $1 }'; }
file_hash() { sha256sum "$1" | cut -d' ' -f1; }

# What this checkout would have installed as `rel`, for copies from before
# the hash lines: only a file identical to it is removed.
shipped_file() {
  local rel=$1
  if [[ $rel == manifest.json ]]; then
    jq '.entryPoints |= with_entries(.value |= ltrimstr("shell-plugin/"))' manifest.json 2>/dev/null
  elif [[ -f shell-plugin/$rel ]]; then
    cat "shell-plugin/$rel"
  fi
}

# Remove the files Opal put in `dir` and still match, and the folders they
# were in once empty; keep the rest (your files, folders, links).
remove_plugin_copy() {
  local dir=$1 legacy=0 rel recorded ours
  [[ -n "$(plugin_hashes "$dir")" ]] || legacy=1
  while IFS= read -r -d '' rel; do
    [[ -f $dir/$rel && ! -L $dir/$rel ]] || continue
    recorded="$(recorded_hash "$dir" "$rel")"
    ours=0
    if [[ -n $recorded ]]; then
      [[ "$(file_hash "$dir/$rel")" == "$recorded" ]] && ours=1
    elif (( legacy )) && cmp -s "$dir/$rel" <(shipped_file "$rel"); then
      ours=1
    fi
    (( ours )) || continue
    rm -f "$dir/$rel"
    prune_plugin_dirs "$dir" "$rel"
  done < <(cd "$dir" && find . -mindepth 1 ! -name "$MARKER" -printf '%P\0')
  rm -f "$dir/$MARKER"
  rmdir "$dir" 2>/dev/null || echo "  kept $dir: it holds entries that aren't Opal's (or were changed)"
}

# After removing Opal's file `rel`, remove the folders it was in if they are
# empty now; a folder you made stays even when empty.
prune_plugin_dirs() {
  local dir=$1 rel=$2
  rel="$(dirname "$rel")"
  while [[ $rel != "." ]]; do
    rmdir "$dir/$rel" 2>/dev/null || break
    rel="$(dirname "$rel")"
  done
}
# The launcher is removed only if it is exactly what install.sh writes (now,
# or before the X-Opal-Source line): an edited one is yours to keep.
render_launcher() { sed "s|@BINDIR@|$HOME/.local/bin|g" dist/opal-nostrconnect.desktop; }
our_launcher() {
  [[ -f $LAUNCHER && ! -L $LAUNCHER ]] || return 1
  cmp -s "$LAUNCHER" <(render_launcher) || cmp -s "$LAUNCHER" <(render_launcher | grep -v '^X-Opal-Source=')
}
PURGE=0
[[ "${1:-}" == "--purge" ]] && PURGE=1

if (( PURGE )); then
  echo "This deletes every Opal key from the keyring, plus settings and history."
  echo "Make sure you have a backup of your keys (ncryptsec) first."
  read -r -p "Type 'delete my keys' to continue: " reply
  [[ $reply == "delete my keys" ]] || { echo "Cancelled."; exit 1; }
fi

echo "Stopping the service"
if [[ -e $UNIT ]] && ! grep -q "https://github.com/derekross/opal" "$UNIT"; then
  echo "  $UNIT isn't Opal's; leaving it alone."
else
  systemctl --user disable --now opal.service >/dev/null 2>&1 || true
  rm -f "$UNIT"
  systemctl --user daemon-reload
fi

echo "Removing binaries and the link handler"
for bin in "opald:Opal daemon" "opal:Control the Opal Nostr signer"; do
  path="$HOME/.local/bin/${bin%%:*}"
  if [[ -e $path ]] && grep -qa "${bin#*:}" "$path"; then
    rm -f "$path"
  elif [[ -e $path ]]; then
    echo "  $path isn't Opal's; leaving it alone."
  fi
done
if our_launcher; then
  if [[ "$(xdg-mime query default x-scheme-handler/nostrconnect 2>/dev/null)" == "opal-nostrconnect.desktop" ]]; then
    # Leave no dangling default behind.
    sed -i '/x-scheme-handler\/nostrconnect=opal-nostrconnect.desktop/d' "$HOME/.config/mimeapps.list" 2>/dev/null || true
  fi
  rm -f "$LAUNCHER"
  update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
elif [[ -e $LAUNCHER || -L $LAUNCHER ]]; then
  echo "  $LAUNCHER isn't the one install.sh wrote (edited, or not Opal's); leaving it alone."
fi

echo "Removing the shell plugin"
if command -v omarchy >/dev/null; then
  omarchy plugin disable "$PLUGIN_ID" >/dev/null 2>&1 || true
fi
for dir in "$PLUGINDIR/$PLUGIN_ID:$PLUGIN_ID" "$PLUGINDIR/opal:opal"; do
  path="${dir%%:*}"
  if our_plugin_copy "$path" "${dir#*:}"; then
    remove_plugin_copy "$path"
  elif [[ -e $path && $path == "$PLUGINDIR/$PLUGIN_ID" ]]; then
    # e.g. added with `omarchy plugin add`: that command removes it.
    echo "  $path wasn't installed by install.sh; remove it with: omarchy plugin remove $PLUGIN_ID"
  fi
done
omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true

if (( PURGE )); then
  echo "Deleting keys, settings and history"
  secret-tool clear application opal 2>/dev/null || true
  rm -rf "$HOME/.local/share/opal" "$HOME/.config/opal" "$HOME/.cache/opal"
else
  echo
  echo "Kept: your keys (keyring, encrypted), ~/.local/share/opal, ~/.config/opal."
  echo "Run with --purge to delete those too."
fi
echo "Opal removed."
