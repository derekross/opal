#!/usr/bin/env bash
# Build and install Opal for the current user: the opald daemon and opal CLI
# (~/.local/bin), a systemd user service, the nostrconnect:// link handler,
# and the Omarchy shell plugin.
#
#   ./dist/install.sh             build with cargo if Rust is installed,
#                                 otherwise download the release binaries
#   ./dist/install.sh --build     always build from source
#   ./dist/install.sh --prebuilt  always download the release binaries
#   ./dist/install.sh --no-build  install already-built binaries
#
# Release binaries are built by GitHub Actions from the tag matching this
# checkout's version, and checked against the release's SHA256SUMS (and its
# build attestation too, when the GitHub CLI is signed in).
#
# Run it again after `git pull` / `omarchy plugin update` to update.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO="$PWD"
BINDIR="$HOME/.local/bin"
UNITDIR="$HOME/.config/systemd/user"
APPDIR="$HOME/.local/share/applications"
PLUGINDIR="$HOME/.config/omarchy/plugins"
PLUGIN_ID="$(jq -r .id manifest.json)"
VERSION="$(jq -r .version manifest.json)"
GITHUB_REPO="derekross/opal"
PLUGIN_PATH="$PLUGINDIR/$PLUGIN_ID"

ask() {
  # ask "question" → 0 for yes. Non-interactive runs answer no.
  [[ -t 0 ]] || return 1
  read -r -p "$1 [y/N] " reply
  [[ $reply =~ ^[Yy] ]]
}

# Installed with `omarchy plugin add`, this checkout *is* the plugin. Build
# outside it: the shell reloads plugins whenever files change in there.
FROM_PLUGIN_CHECKOUT=0
if [[ "$REPO" == "$(realpath -m "$PLUGIN_PATH")" ]]; then
  FROM_PLUGIN_CHECKOUT=1
  export CARGO_TARGET_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/opal/build"
fi
TARGET="${CARGO_TARGET_DIR:-$REPO/target}"

MODE="${1:-auto}"
case $MODE in
  auto) command -v cargo >/dev/null && MODE=--build || MODE=--prebuilt ;;
  --build | --prebuilt | --no-build) ;;
  *) echo "Unknown option: $MODE (use --build, --prebuilt or --no-build)" >&2; exit 2 ;;
esac

download_release() {
  local arch name base dir
  arch="$(uname -m)"
  [[ $arch == x86_64 || $arch == aarch64 ]] || { echo "No release build for $arch; install Rust to build Opal." >&2; exit 1; }
  name="opal-v$VERSION-$arch-linux"
  base="https://github.com/$GITHUB_REPO/releases/download/v$VERSION"
  dir="${XDG_CACHE_HOME:-$HOME/.cache}/opal/release"
  rm -rf "$dir" && mkdir -p "$dir"
  echo "Downloading Opal v$VERSION ($arch)"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$dir/$name.tar.gz" "$base/$name.tar.gz"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$dir/SHA256SUMS" "$base/SHA256SUMS"
  (cd "$dir" && grep -E "  $name\.tar\.gz\$" SHA256SUMS | sha256sum --check --status) \
    || { echo "Checksum mismatch for $name.tar.gz; not installing it." >&2; exit 1; }
  echo "  Checksum OK"
  if command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then
    gh attestation verify "$dir/$name.tar.gz" --repo "$GITHUB_REPO" >/dev/null \
      || { echo "Build attestation check failed; not installing it." >&2; exit 1; }
    echo "  Built by GitHub Actions from $GITHUB_REPO (attestation verified)"
  fi
  tar -xzf "$dir/$name.tar.gz" -C "$dir"
  BIN_SRC="$dir/$name"
}

BIN_SRC="$TARGET/release"
case $MODE in
  --build)
    command -v cargo >/dev/null || {
      echo "Rust is needed to build Opal: sudo pacman -S --needed rustup && rustup default stable" >&2
      exit 1
    }
    cargo build --locked --release -p opald -p opal-cli
    ;;
  --prebuilt) download_release ;;
esac

echo "Installing binaries to $BINDIR"
install -Dm755 "$BIN_SRC/opald" "$BINDIR/opald"
install -Dm755 "$BIN_SRC/opal" "$BINDIR/opal"

mkdir -p -m 700 "$HOME/.local/share/opal" "$HOME/.config/opal" "$HOME/.cache/opal"

echo "Installing the systemd user service"
install -Dm644 dist/opal.service "$UNITDIR/opal.service"
systemctl --user daemon-reload
systemctl --user enable opal.service >/dev/null
systemctl --user restart opal.service

echo "nostrconnect:// link handler"
mkdir -p "$APPDIR"
sed "s|@BINDIR@|$BINDIR|g" dist/opal-nostrconnect.desktop >"$APPDIR/opal-nostrconnect.desktop"
update-desktop-database "$APPDIR" 2>/dev/null || true
current="$(xdg-mime query default x-scheme-handler/nostrconnect 2>/dev/null || true)"
if [[ -z $current || $current == "opal-nostrconnect.desktop" ]]; then
  xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect
elif ask "  nostrconnect:// links currently open with $current. Open them with Opal instead?"; then
  xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect
else
  echo "  Left $current as the handler (switch any time: xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect)"
fi

# Earlier versions installed the plugin under the id "opal".
if [[ $PLUGIN_ID != "opal" && -d "$PLUGINDIR/opal" && -f "$PLUGINDIR/opal/OpalService.qml" ]]; then
  echo "Removing the older 'opal' plugin install"
  omarchy plugin disable opal >/dev/null 2>&1 || true
  rm -rf "$PLUGINDIR/opal"
fi

if (( FROM_PLUGIN_CHECKOUT )); then
  echo "Shell plugin: installed by 'omarchy plugin add' ($PLUGIN_ID)"
else
  echo "Installing the Omarchy shell plugin ($PLUGIN_ID)"
  mkdir -p "$PLUGINDIR"
  # Copied, not linked: the shell's file watcher doesn't follow symlinks.
  rm -rf "$PLUGIN_PATH.new"
  cp -r shell-plugin "$PLUGIN_PATH.new"
  # The repo's single manifest points into shell-plugin/; here the files sit
  # at the top of the plugin folder.
  jq '.entryPoints |= with_entries(.value |= ltrimstr("shell-plugin/"))' manifest.json \
    >"$PLUGIN_PATH.new/manifest.json"
  rm -rf "$PLUGIN_PATH"
  mv "$PLUGIN_PATH.new" "$PLUGIN_PATH"
fi
if command -v omarchy >/dev/null; then
  omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
  omarchy plugin enable "$PLUGIN_ID" --section right --before omarchy.tray >/dev/null 2>&1 \
    || omarchy plugin enable "$PLUGIN_ID" --section right >/dev/null 2>&1 || true
fi

echo
systemctl --user --no-pager --lines=0 status opal.service | head -3
echo
echo "Done. Click the Opal gem in the bar to add a key or watch someone."
