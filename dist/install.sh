#!/usr/bin/env bash
# Build and install Opal for the current user.
#   ./dist/install.sh            build (release) and install everything
#   ./dist/install.sh --no-build install already-built binaries
set -euo pipefail

cd "$(dirname "$0")/.."
BINDIR="$HOME/.local/bin"
UNITDIR="$HOME/.config/systemd/user"
APPDIR="$HOME/.local/share/applications"
PLUGINDIR="$HOME/.config/omarchy/plugins"

if [[ "${1:-}" != "--no-build" ]]; then
  cargo build --release -p opald -p opal-cli
fi

echo "Installing binaries to $BINDIR"
install -Dm755 target/release/opald "$BINDIR/opald"
install -Dm755 target/release/opal "$BINDIR/opal"

mkdir -p -m 700 "$HOME/.local/share/opal" "$HOME/.config/opal"

echo "Installing systemd user service"
install -Dm644 dist/opal.service "$UNITDIR/opal.service"
systemctl --user daemon-reload
systemctl --user enable opal.service >/dev/null
systemctl --user restart opal.service

echo "Registering the nostrconnect:// handler"
mkdir -p "$APPDIR"
sed "s|@BINDIR@|$BINDIR|g" dist/opal-nostrconnect.desktop >"$APPDIR/opal-nostrconnect.desktop"
update-desktop-database "$APPDIR" 2>/dev/null || true
xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect

if [[ -d shell-plugin ]]; then
  echo "Installing the Omarchy shell plugin"
  mkdir -p "$PLUGINDIR"
  # Copied, not linked: the shell's file watcher doesn't follow symlinks and
  # `omarchy plugin validate` rejects them.
  rm -rf "$PLUGINDIR/opal.new"
  cp -r shell-plugin "$PLUGINDIR/opal.new"
  rm -rf "$PLUGINDIR/opal"
  mv "$PLUGINDIR/opal.new" "$PLUGINDIR/opal"
  if command -v omarchy >/dev/null; then
    omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
    omarchy plugin enable opal --section right --before omarchy.tray >/dev/null 2>&1 \
      || omarchy plugin enable opal --section right >/dev/null 2>&1 || true
  fi
fi

echo
systemctl --user --no-pager --lines=0 status opal.service | head -3
echo
echo "Done. Next: 'opal account add' (or open the Opal panel in the bar)."
