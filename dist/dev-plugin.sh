#!/usr/bin/env bash
# Copy the shell plugin into place and reload it (development loop).
set -euo pipefail
cd "$(dirname "$0")/.."
dest="$HOME/.config/omarchy/plugins/opal"
mkdir -p "$dest"
rsync -a --delete shell-plugin/ "$dest/"
omarchy-shell shell rescanPlugins >/dev/null
echo "plugin synced"
