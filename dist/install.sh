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
#   --replace-existing=<path>     consent, for a non-interactive run, to
#                                 replace that one file in ~/.local/bin that
#                                 Opal has no record of (it is kept as a
#                                 backup); repeat for the other
#
# Release binaries are built by GitHub Actions from the tag matching this
# checkout's version. A download is accepted only if it matches the hash
# pinned in this checkout (dist/release-checksums.tsv, added after each
# release once its build attestation was verified), so the bytes are tied
# to a reviewed commit, not to what the release page holds today. When the
# GitHub CLI is signed in the attestation is checked again as well.
#
# Run it again after `git pull` / `omarchy plugin update` to update.
#
# What it touches, and when (the rule is in dist/lib.sh): a file is replaced
# only while it is exactly what an Opal version wrote; a file you changed
# stays and is named; a folder or link is never Opal's. Everything is
# checked before anything is written. The service is enabled only on first
# install; later runs restart it only if it is running Opal's binary.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO="$PWD"
source dist/lib.sh || { echo "dist/lib.sh is missing: run this from an Opal checkout" >&2; exit 1; }
VERSION="$(jq -r .version manifest.json)"
GITHUB_REPO="derekross/opal"

MODE=auto
declare -A CONSENT=()
for arg in "$@"; do
  case $arg in
    --build | --prebuilt | --no-build) MODE=$arg ;;
    --replace-existing=/*) CONSENT["${arg#--replace-existing=}"]=1 ;;
    --replace-existing | --replace-existing=*)
      die "--replace-existing needs the absolute path of the one file to replace, e.g. --replace-existing=$BINDIR/opald" ;;
    -h | --help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "Unknown option: $arg (see --help)" >&2; exit 2 ;;
  esac
done
[[ $MODE == auto ]] && { command -v cargo >/dev/null && MODE=--build || MODE=--prebuilt; }

# Installed with `omarchy plugin add`, this checkout *is* the plugin. Build
# outside it: the shell reloads plugins whenever files change in there.
FROM_PLUGIN_CHECKOUT=0
if [[ "$REPO" == "$(realpath -m "$PLUGIN_PATH")" ]]; then
  FROM_PLUGIN_CHECKOUT=1
  export CARGO_TARGET_DIR="$CACHEDIR/build"
fi
TARGET="${CARGO_TARGET_DIR:-$REPO/target}"

# ── 1. Build or download ───────────────────────────────────────────────
download_release() {
  local arch name base dir
  arch="$(uname -m)"
  [[ $arch == x86_64 || $arch == aarch64 ]] || die "No release build for $arch; install Rust to build Opal."
  name="opal-v$VERSION-$arch-linux"
  base="https://github.com/$GITHUB_REPO/releases/download/v$VERSION"
  dir="$CACHEDIR/release"
  local pin size
  pin="$(pinned_release "$name.tar.gz")"
  [[ -n $pin ]] \
    || die "This checkout has no pinned checksum for $name.tar.gz (a release is pinned in dist/release-checksums.tsv after it is published). Build from source with --build, or use a newer checkout."
  size="${pin##* }"
  rm -rf -- "${dir:?}" && mkdir -p -- "$dir"
  say "Downloading Opal v$VERSION ($arch, $size bytes)"
  # Bounded: 20 s to connect, 15 min in all, give up below 1 KiB/s for a
  # minute, and never take more than the pinned size (curl stops at once).
  curl -fsSL --proto '=https' --tlsv1.2 \
    --connect-timeout 20 --max-time 900 --speed-limit 1024 --speed-time 60 \
    --max-filesize "$size" -o "$dir/$name.tar.gz" "$base/$name.tar.gz" \
    || { rc=$?; rm -f -- "$dir/$name.tar.gz"; die "Download failed (curl exit $rc$( (( rc == 63 )) && printf ': larger than the pinned %s bytes' "$size")); nothing installed."; }
  verify_pinned "$dir/$name.tar.gz" "$name.tar.gz" \
    || die "Not installing $name.tar.gz: it isn't the release this checkout was reviewed with."
  note "Checksum matches the one pinned in this checkout (built from $PINNED_COMMIT)"
  if command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then
    gh attestation verify "$dir/$name.tar.gz" --repo "$GITHUB_REPO" >/dev/null \
      || die "Build attestation check failed; not installing it."
    note "Build attestation verified as well (GitHub Actions, $GITHUB_REPO)"
  fi
  tar -xzf "$dir/$name.tar.gz" -C "$dir"
  BIN_SRC="$dir/$name"
}

BIN_SRC="$TARGET/release"
case $MODE in
  --build)
    command -v cargo >/dev/null \
      || die "Rust is needed to build Opal: sudo pacman -S --needed rustup && rustup default stable"
    cargo build --locked --release -p opald -p opal-cli
    ;;
  --prebuilt) download_release ;;
esac
[[ -f $BIN_SRC/opald && -f $BIN_SRC/opal ]] || die "no built binaries in $BIN_SRC"

# ── 2. Look before touching anything ───────────────────────────────────
prepare_state
load_manifest
load_known
load_shipped
print_paths

STOPS=()
stop() { STOPS+=("$*"); }

declare -A BIN_STATE=() BIN_HASH=()
for bin in opald opal; do
  p="$BINDIR/$bin"
  BIN_STATE[$bin]="$(binary_state "$p")"
  BIN_HASH[$bin]="$(file_hash "$p")"
  case ${BIN_STATE[$bin]} in
    symlink) stop "$p is a symbolic link (to $(readlink -- "$p")); Opal doesn't follow links. Move it aside, then run this again." ;;
    other) stop "$p exists and isn't a regular file. Move it aside, then run this again." ;;
  esac
done

inspect_unit
UNIT_HASH="$(file_hash "$UNIT")"
case $UNIT_STATE in
  elsewhere) stop "opal.service is already provided by $UNIT_FRAGMENT; installing Opal's unit would shadow it. Remove or rename that unit first." ;;
  symlink) stop "$UNIT is a symbolic link: the unit is masked or linked (systemctl --user mask/link). Opal won't replace it; unmask or unlink it, then run this again." ;;
  other) stop "$UNIT exists and isn't a regular file. Move it aside, then run this again." ;;
  foreign) stop "$UNIT exists and isn't Opal's (no Opal mark in it). Move it aside, then run this again." ;;
esac

inspect_launcher
LAUNCHER_HASH="$(file_hash "$LAUNCHER")"
HANDLER_NOW="$(xdg-mime query default x-scheme-handler/nostrconnect 2>/dev/null || true)"

PLUGIN_STATE="$(plugin_dir_state "$PLUGIN_PATH")"
(( FROM_PLUGIN_CHECKOUT )) && PLUGIN_STATE=checkout
OLD_PLUGIN_STATE=absent
[[ $PLUGIN_ID != opal ]] && OLD_PLUGIN_STATE="$(plugin_dir_state "$OLD_PLUGIN_PATH")"

for d in "$DATADIR" "$CONFIGDIR" "$CACHEDIR" "$BINDIR" "$UNITDIR" "$APPDIR" "$PLUGINDIR"; do
  if [[ -e $d && ! -d $d ]]; then stop "$d exists and isn't a folder. Move it aside, then run this again."; fi
done

if (( ${#STOPS[@]} )); then
  say "Not installing: nothing was changed."
  for s in "${STOPS[@]}"; do note "$s"; done
  exit 1
fi

# ── 3. Questions ───────────────────────────────────────────────────────
declare -A REPLACE=()
for bin in opald opal; do
  p="$BINDIR/$bin"
  [[ ${BIN_STATE[$bin]} == unrecorded ]] || continue
  if [[ -n ${CONSENT[$p]:-} ]]; then REPLACE[$bin]=1; continue; fi
  say "$p exists but isn't recorded as installed by Opal ($(describe_file "$p"))."
  say "  It may be an earlier Opal you built from source (0.2 and 0.3 installs kept no record),"
  say "  or another program. If replaced, it is moved to $BACKUPDIR/ and Opal never deletes it."
  if ask "  Replace $p?"; then
    REPLACE[$bin]=1
  else
    say "Nothing was changed. Run again with --replace-existing=$p to replace it, or move it aside yourself."
    exit 1
  fi
done

SET_HANDLER=0
if [[ $LAUNCHER_STATE == missing || $LAUNCHER_STATE == owned ]]; then
  if [[ -z $HANDLER_NOW || $HANDLER_NOW == opal-nostrconnect.desktop ]]; then
    SET_HANDLER=1
  elif ask "nostrconnect:// links currently open with $HANDLER_NOW. Open them with Opal instead?"; then
    SET_HANDLER=1
  fi
fi

# ── 4. Write ───────────────────────────────────────────────────────────
say "Installing binaries to $BINDIR"
mkdir -p -- "$BINDIR"
for bin in opald opal; do
  p="$BINDIR/$bin"
  case ${BIN_STATE[$bin]} in
    missing) replace_owned "$BIN_SRC/$bin" "$p" 755 "" ;;
    owned) replace_owned "$BIN_SRC/$bin" "$p" 755 "${BIN_HASH[$bin]}" ;;
    unrecorded)
      backup_file "$p" "$bin"
      note "moved the previous $p to $BACKUP_DEST"
      replace_owned "$BIN_SRC/$bin" "$p" 755 "" ;;
  esac
done

for d in "$DATADIR" "$CONFIGDIR" "$CACHEDIR"; do
  [[ -d $d ]] || mkdir -p -m 700 -- "$d"
done

case $UNIT_STATE in
  missing)
    say "Installing the systemd user service"
    replace_owned dist/opal.service "$UNIT" 644 "" ;;
  owned)
    say "Updating the systemd user service"
    replace_owned dist/opal.service "$UNIT" 644 "$UNIT_HASH" ;;
  edited)
    say "Keeping your $UNIT (you changed it; Opal's version isn't installed)" ;;
esac
(( UNIT_DROPIN )) && note "$UNIT.d/ drop-ins are yours; not touched."

LAUNCHER_WRITTEN=0
case $LAUNCHER_STATE in
  missing | owned)
    say "nostrconnect:// link handler"
    tmp="$(mktemp)"; TEMPS+=("$tmp")
    render_launcher >"$tmp"
    if [[ $LAUNCHER_STATE == missing ]]; then replace_owned "$tmp" "$LAUNCHER" 644 ""
    else replace_owned "$tmp" "$LAUNCHER" 644 "$LAUNCHER_HASH"; fi
    LAUNCHER_WRITTEN=1 ;;
  other) say "Keeping $LAUNCHER: it's a link or not a file, so not Opal's. The nostrconnect:// handler is left as it is." ;;
  foreign) say "Keeping $LAUNCHER: it isn't the one this script writes (you changed it, or it's another program's). The nostrconnect:// handler is left as it is." ;;
esac

OLD_PLUGIN_REMOVED=0
if [[ $OLD_PLUGIN_STATE == dir ]]; then
  say "Removing the older 'opal' plugin install ($OLD_PLUGIN_PATH)"
  remove_plugin_copy "$OLD_PLUGIN_PATH" && OLD_PLUGIN_REMOVED=1
fi

PLUGIN_CREATED=0 PLUGIN_TOUCHED=0
case $PLUGIN_STATE in
  checkout)
    if (( FROM_PLUGIN_CHECKOUT )); then say "Shell plugin: this checkout, installed by 'omarchy plugin add' ($PLUGIN_ID)"
    else say "Keeping $PLUGIN_PATH: it's a checkout (has .git); update it with: omarchy plugin update $PLUGIN_ID"; fi ;;
  missing)
    say "Installing the Omarchy shell plugin ($PLUGIN_ID)"
    create_plugin_copy "$PLUGIN_PATH"
    PLUGIN_CREATED=1 PLUGIN_TOUCHED=1 ;;
  dir)
    say "Updating the Omarchy shell plugin ($PLUGIN_ID)"
    update_plugin_copy "$PLUGIN_PATH" && PLUGIN_TOUCHED=1 ;;
  symlink) say "Keeping $PLUGIN_PATH: it's a link, so not Opal's; the plugin isn't installed." ;;
  other) say "Keeping $PLUGIN_PATH: it isn't a folder, so not Opal's; the plugin isn't installed." ;;
esac

# ── 5. Services and the shell ──────────────────────────────────────────
case $UNIT_STATE in
  missing)
    systemctl_user daemon-reload
    systemctl_user enable --now opal.service
    say "opal.service enabled and started." ;;
  owned | edited)
    systemctl_user daemon-reload
    if unit_is_active && unit_runs_our_binary; then
      systemctl_user restart opal.service
      say "opal.service restarted with the new build."
    elif unit_is_active; then
      say "opal.service is running but doesn't start $BINDIR/opald; restart it yourself if you want the new build."
    else
      say "opal.service isn't running; not started (start it with: systemctl --user start opal.service)."
    fi ;;
esac

if (( LAUNCHER_WRITTEN )); then
  update-desktop-database "$APPDIR" >/dev/null 2>&1 || true
  if (( SET_HANDLER )); then
    if xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect 2>/dev/null; then
      note "nostrconnect:// links open with Opal."
    else
      note "couldn't set the nostrconnect:// handler (xdg-mime failed); set it with: xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect"
    fi
  else
    note "Left $HANDLER_NOW as the nostrconnect:// handler (switch any time: xdg-mime default opal-nostrconnect.desktop x-scheme-handler/nostrconnect)"
  fi
fi

if command -v omarchy >/dev/null; then
  if (( OLD_PLUGIN_REMOVED )); then omarchy plugin disable opal >/dev/null 2>&1 || true; fi
  if (( PLUGIN_TOUCHED || OLD_PLUGIN_REMOVED )); then omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true; fi
  if (( PLUGIN_CREATED )); then
    omarchy plugin enable "$PLUGIN_ID" --section right --before omarchy.tray >/dev/null 2>&1 \
      || omarchy plugin enable "$PLUGIN_ID" --section right >/dev/null 2>&1 || true
  fi
fi

say
{ systemctl --user --no-pager --lines=0 status opal.service 2>/dev/null | head -3; } || true
say
say "Done. Click the Opal gem in the bar to add a key or watch someone."
