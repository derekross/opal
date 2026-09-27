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

MARKER=".installed-by-opal"
UNIT="$UNITDIR/opal.service"
LAUNCHER="$APPDIR/opal-nostrconnect.desktop"

die() { echo "$*" >&2; exit 1; }

# Opal only replaces what it installed itself. Anything else at these paths
# (another program's `opal` command, your own opal.service, a plugin
# checkout) is left alone and the install stops.
our_binary() { [[ ! -e $1 ]] || grep -qa "$2" "$1"; }
our_unit() { [[ ! -e $UNIT ]] || grep -q "https://github.com/derekross/opal" "$UNIT"; }
# The link handler is ours only if it is exactly what this script writes
# (or wrote before the X-Opal-Source line was added). A launcher you edited
# keeps our mark but isn't ours to replace any more.
render_launcher() { sed "s|@BINDIR@|$BINDIR|g" dist/opal-nostrconnect.desktop; }
our_launcher() {
  [[ -f $LAUNCHER && ! -L $LAUNCHER ]] || return 1
  cmp -s "$LAUNCHER" <(render_launcher) || cmp -s "$LAUNCHER" <(render_launcher | grep -v '^X-Opal-Source=')
}
# A plugin folder this script copied: our marker, or (installs from before
# the marker) a plain folder with Opal's manifest and service file.
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

# Carry what's yours in `old` over into `staging` (the new copy), so the
# swap below loses nothing: Opal's unchanged files are the only entries
# not carried over. Every entry counts: Opal ships regular files only, so a
# folder (empty or not), a symlink or anything else is yours.
merge_plugin_copy() {
  local old=$1 staging=$2 legacy=0 rel recorded
  [[ -n "$(plugin_hashes "$old")" ]] || legacy=1
  while IFS= read -r -d '' rel; do
    [[ $rel == "$MARKER" ]] && continue
    if [[ -d $old/$rel && ! -L $old/$rel ]]; then
      mkdir -p "$staging/$rel"
      continue
    fi
    if [[ -f $old/$rel && ! -L $old/$rel ]]; then
      recorded="$(recorded_hash "$old" "$rel")"
      if [[ -n $recorded ]]; then
        [[ "$(file_hash "$old/$rel")" == "$recorded" ]] && continue   # Opal's, unchanged
        echo "  keeping $rel: you changed it (Opal's version isn't installed)"
      elif (( legacy )) && [[ -f $staging/$rel ]]; then
        continue   # a file Opal ships, from before the hash lines
      else
        echo "  keeping $rel: not Opal's"
      fi
    else
      echo "  keeping $rel: not Opal's"
    fi
    # Yours now, whatever Opal ships under that name: drop it from the list
    # of Opal's files and put yours in its place.
    sed -i "\|  ${rel//|/\\|}\$|d" "$staging/$MARKER"
    mkdir -p "$staging/$(dirname "$rel")"
    rm -rf "${staging:?}/$rel"
    cp -a "$old/$rel" "$staging/$rel"
  done < <(cd "$old" && find . -mindepth 1 -printf '%P\0')
}

# Remove the files Opal put in `dir` and still match, and the folders they
# were in once empty; keep the rest (your files, folders, links).
remove_plugin_copy() {
  local dir=$1 rel recorded
  while IFS= read -r -d '' rel; do
    [[ -f $dir/$rel && ! -L $dir/$rel ]] || continue
    recorded="$(recorded_hash "$dir" "$rel")"
    [[ -n $recorded && "$(file_hash "$dir/$rel")" == "$recorded" ]] || continue
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
  rm -rf "${dir:?}" && mkdir -p "$dir"
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

# Check everything before changing anything.
our_binary "$BINDIR/opald" "Opal daemon" \
  || die "$BINDIR/opald exists and isn't Opal's. Move it aside, then run this again."
our_binary "$BINDIR/opal" "Control the Opal Nostr signer" \
  || die "$BINDIR/opal exists and isn't Opal's. Move it aside, then run this again."
our_unit || die "$UNIT exists and isn't Opal's. Move it aside, then run this again."
INSTALL_LAUNCHER=1
if [[ -e $LAUNCHER || -L $LAUNCHER ]] && ! our_launcher; then
  INSTALL_LAUNCHER=0
  echo "Note: $LAUNCHER isn't the one this script writes (edited, or not Opal's);"
  echo "  leaving it and the nostrconnect:// handler as they are."
fi
INSTALL_PLUGIN=1
if (( FROM_PLUGIN_CHECKOUT )); then
  INSTALL_PLUGIN=0
elif [[ -e $PLUGIN_PATH || -L $PLUGIN_PATH ]] && ! our_plugin_copy "$PLUGIN_PATH" "$PLUGIN_ID"; then
  INSTALL_PLUGIN=0
  echo "Note: $PLUGIN_PATH exists and wasn't installed by this script"
  echo "  (e.g. added with 'omarchy plugin add'); leaving it as it is."
fi

echo "Installing binaries to $BINDIR"
install -Dm755 "$BIN_SRC/opald" "$BINDIR/opald"
install -Dm755 "$BIN_SRC/opal" "$BINDIR/opal"

mkdir -p -m 700 "$HOME/.local/share/opal" "$HOME/.config/opal" "$HOME/.cache/opal"

echo "Installing the systemd user service"
install -Dm644 dist/opal.service "$UNIT"
systemctl --user daemon-reload
systemctl --user enable opal.service >/dev/null
systemctl --user restart opal.service

if (( INSTALL_LAUNCHER )); then
  echo "nostrconnect:// link handler"
  mkdir -p "$APPDIR"
  # Only reached for a missing launcher or our own unchanged one (checked above).
  render_launcher >"$LAUNCHER"
  update-desktop-database "$APPDIR" 2>/dev/null || true
  current="$(xdg-mime query default x-scheme-handler/nostrconnect 2>/dev/null || true)"
  if [[ -z $current || $current == "opal-nostrconnect.desktop" ]]; then
    xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect
  elif ask "  nostrconnect:// links currently open with $current. Open them with Opal instead?"; then
    xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect
  else
    echo "  Left $current as the handler (switch any time: xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect)"
  fi
fi

# Earlier versions installed the plugin under the id "opal".
if [[ $PLUGIN_ID != "opal" ]] && our_plugin_copy "$PLUGINDIR/opal" "opal"; then
  echo "Removing the older 'opal' plugin install"
  omarchy plugin disable opal >/dev/null 2>&1 || true
  remove_plugin_copy "$PLUGINDIR/opal"
fi

if (( FROM_PLUGIN_CHECKOUT )); then
  echo "Shell plugin: installed by 'omarchy plugin add' ($PLUGIN_ID)"
elif (( INSTALL_PLUGIN )); then
  echo "Installing the Omarchy shell plugin ($PLUGIN_ID)"
  mkdir -p "$PLUGINDIR"
  # Copied, not linked: the shell's file watcher doesn't follow symlinks.
  # Built next to the destination, then swapped in.
  staging="$(mktemp -d "$PLUGINDIR/.$PLUGIN_ID.XXXXXX")"
  cp -r shell-plugin/. "$staging/"
  # The repo's single manifest points into shell-plugin/; here the files sit
  # at the top of the plugin folder.
  jq '.entryPoints |= with_entries(.value |= ltrimstr("shell-plugin/"))' manifest.json \
    >"$staging/manifest.json"
  # Record what Opal put there, before anything of yours is carried over.
  {
    echo "https://github.com/derekross/opal"
    (cd "$staging" && find . -type f ! -name "$MARKER" -printf '%P\n' | LC_ALL=C sort | xargs -d '\n' sha256sum)
  } >"$staging/$MARKER"
  chmod 755 "$staging"
  if [[ -e $PLUGIN_PATH ]]; then
    # Only reached for our own earlier copy (checked above). Everything in
    # it is either Opal's and unchanged (replaced by the new copy) or yours
    # (carried into the new copy), so the old folder can go.
    merge_plugin_copy "$PLUGIN_PATH" "$staging"
    rm -rf "${PLUGIN_PATH:?}"
  fi
  mv "$staging" "$PLUGIN_PATH"
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
