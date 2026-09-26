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
MARKER=".installed-by-opal"

# Only what Opal installed is removed (see install.sh).
our_plugin_copy() {
  local dir=$1 id=$2
  [[ -d $dir && ! -L $dir ]] || return 1
  [[ -f $dir/$MARKER ]] && return 0
  [[ ! -e $dir/.git && -f $dir/OpalService.qml ]] \
    && [[ "$(jq -r .id "$dir/manifest.json" 2>/dev/null)" == "$id" ]]
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
if [[ "$(xdg-mime query default x-scheme-handler/nostrconnect 2>/dev/null)" == "opal-nostrconnect.desktop" ]]; then
  # Leave no dangling default behind.
  sed -i '/x-scheme-handler\/nostrconnect=opal-nostrconnect.desktop/d' "$HOME/.config/mimeapps.list" 2>/dev/null || true
fi
rm -f "$HOME/.local/share/applications/opal-nostrconnect.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true

echo "Removing the shell plugin"
if command -v omarchy >/dev/null; then
  omarchy plugin disable "$PLUGIN_ID" >/dev/null 2>&1 || true
fi
for dir in "$PLUGINDIR/$PLUGIN_ID:$PLUGIN_ID" "$PLUGINDIR/opal:opal"; do
  path="${dir%%:*}"
  if our_plugin_copy "$path" "${dir#*:}"; then
    rm -rf "${path:?}"
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
