#!/usr/bin/env bash
# Copy the shell plugin into place and reload it (development loop).
set -euo pipefail
cd "$(dirname "$0")/.."
dest="$HOME/.config/omarchy/plugins/$(jq -r .id manifest.json)"
mkdir -p "$dest"
rsync -a --delete --exclude manifest.json shell-plugin/ "$dest/"
jq '.entryPoints |= with_entries(.value |= ltrimstr("shell-plugin/"))' manifest.json >"$dest/manifest.json"
omarchy-shell shell rescanPlugins >/dev/null
echo "plugin synced"
