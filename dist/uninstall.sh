#!/usr/bin/env bash
# Remove Opal: the service, binaries, link handler and shell plugin.
# Your keys (in the keyring) and Opal's data are kept unless you pass --purge.
#
#   ./dist/uninstall.sh           remove the program, keep keys and data
#   ./dist/uninstall.sh --purge   also delete keys, settings and history
#
# Only what Opal can prove it wrote is removed (the rule is in dist/lib.sh):
# a unit, launcher or plugin file you changed stays and is named, a masked
# or linked unit is left alone, folders keep anything you added, and
# backups Opal made are never deleted, not even with --purge.
set -euo pipefail

cd "$(dirname "$0")/.."
source dist/lib.sh || { echo "dist/lib.sh is missing: run this from an Opal checkout" >&2; exit 1; }

PURGE=0
for arg in "$@"; do
  case $arg in
    --purge) PURGE=1 ;;
    -h | --help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "Unknown option: $arg (only --purge)" >&2; exit 2 ;;
  esac
done

if (( PURGE )); then
  [[ -t 0 ]] || die "--purge needs a terminal to confirm on."
  say "This deletes every Opal key from the keyring, plus settings and history:"
  note "$DATADIR, $CONFIGDIR, $CACHEDIR"
  say "Make sure you have a backup of your keys (ncryptsec) first."
  read -r -p "Type 'delete my keys' to continue: " reply </dev/tty
  [[ $reply == "delete my keys" ]] || { say "Cancelled."; exit 1; }
fi

prepare_state
load_manifest
load_known
print_paths

# Only paths this script would write itself are taken from the record;
# a record that was edited or restored from elsewhere can't point it at
# anything else.
for p in "${!MANIFEST_HASH[@]}"; do
  case $p in
    "$BINDIR/opald" | "$BINDIR/opal" | "$UNIT" | "$LAUNCHER" | "$PLUGIN_PATH"/* | "$OLD_PLUGIN_PATH"/*) ;;
    *) note "ignoring a record line for $p: not a path this script writes"; unset 'MANIFEST_HASH[$p]' ;;
  esac
done

# ── The service ────────────────────────────────────────────────────────
inspect_unit
say "Stopping the service"
case $UNIT_STATE in
  owned)
    if [[ -z $UNIT_FRAGMENT || $UNIT_FRAGMENT == "$UNIT" ]]; then
      systemctl_user disable --now opal.service
    else
      systemctl_user stop opal.service
    fi
    remove_owned "$UNIT" "$(file_hash "$UNIT")"
    systemctl_user daemon-reload ;;
  edited)
    systemctl_user stop opal.service
    note "$UNIT is kept: you changed it. It starts the binary this removes, so it is stopped but not disabled;"
    note "when you're done with it: systemctl --user disable opal.service && rm $UNIT" ;;
  elsewhere)
    if unit_runs_our_binary; then systemctl_user stop opal.service; note "opal.service comes from $UNIT_FRAGMENT (not Opal's); stopped because it starts the binary this removes, otherwise left alone."
    else note "opal.service comes from $UNIT_FRAGMENT; not Opal's, leaving it alone."; fi ;;
  symlink)
    note "$UNIT is a symbolic link (masked or linked); not Opal's, leaving it alone."
    if [[ -f $UNIT ]] && grep -qF -- "$BINDIR/opald" "$UNIT" 2>/dev/null; then note "it still starts Opal's binary, which this removes: disable or fix it yourself."; fi ;;
  foreign) note "$UNIT isn't Opal's; leaving it alone." ;;
  other) note "$UNIT isn't a regular file; leaving it alone." ;;
  missing) ;;
esac
(( UNIT_DROPIN )) && note "$UNIT.d/ drop-ins are yours; not touched."

# ── Binaries ───────────────────────────────────────────────────────────
say "Removing binaries"
for bin in opald opal; do
  p="$BINDIR/$bin"
  case "$(binary_state "$p")" in
    owned) remove_owned "$p" "$(file_hash "$p")" ;;
    unrecorded) note "$p isn't recorded as installed by Opal ($(describe_file "$p")); leaving it." ;;
    symlink) note "$p is a link; not Opal's, leaving it." ;;
    other) note "$p isn't a regular file; leaving it." ;;
  esac
done

# ── The link handler ───────────────────────────────────────────────────
say "Removing the nostrconnect:// link handler"
inspect_launcher
case $LAUNCHER_STATE in
  owned)
    remove_owned "$LAUNCHER" "$(file_hash "$LAUNCHER")"
    mimeapps_remove_opal
    update-desktop-database "$APPDIR" >/dev/null 2>&1 || true ;;
  foreign) note "$LAUNCHER isn't the one install.sh wrote (you changed it, or it's another program's); leaving it and its handler entry." ;;
  other) note "$LAUNCHER is a link or not a file; not Opal's, leaving it." ;;
esac

# ── The shell plugin ───────────────────────────────────────────────────
say "Removing the shell plugin"
load_shipped
for entry in "$PLUGIN_PATH:$PLUGIN_ID" "$OLD_PLUGIN_PATH:opal"; do
  dir=${entry%%:*}; id=${entry#*:}
  [[ $id == opal && $PLUGIN_ID == opal ]] && continue
  case "$(plugin_dir_state "$dir")" in
    missing) ;;
    checkout) note "$dir is a checkout (has .git); remove it with: omarchy plugin remove $id" ;;
    symlink) note "$dir is a link; not Opal's, leaving it." ;;
    other) note "$dir isn't a folder; not Opal's, leaving it." ;;
    dir)
      if remove_plugin_copy "$dir"; then
        command -v omarchy >/dev/null && { omarchy plugin disable "$id" >/dev/null 2>&1 || true; }
      else
        note "the shell may still list it; disable it with: omarchy plugin disable $id"
      fi ;;
  esac
done
command -v omarchy-shell >/dev/null && { omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true; }

# ── The record, and --purge ────────────────────────────────────────────
# Drop record lines for files that are gone (kept files keep their lines
# only while they still match, which they don't if you changed them).
for p in "${!MANIFEST_HASH[@]}"; do
  [[ -f $p && ! -L $p && "$(file_hash "$p")" == "${MANIFEST_HASH[$p]}" ]] || unset 'MANIFEST_HASH[$p]'
done
write_manifest

if (( PURGE )); then
  say "Deleting keys, settings and history"
  # Only Opal's own item kinds, never everything tagged application=opal.
  before="$(secret-tool search --all application opal 2>/dev/null | grep -c '^\[' || true)"
  for kind in account conn-key client-key; do
    secret-tool clear application opal kind "$kind" 2>/dev/null || note "keyring: clearing '$kind' items failed; they may still be there."
  done
  after="$(secret-tool search --all application opal 2>/dev/null | grep -c '^\[' || true)"
  note "keyring: ${before:-?} Opal item(s) before, ${after:-?} left$( (( ${after:-0} > 0 )) && printf ' (not Opal'\''s kinds, or clearing failed: check with Seahorse)')"
  for d in "$DATADIR" "$CONFIGDIR" "$CACHEDIR"; do
    if [[ -L $d ]]; then note "$d is a link; not followed, not removed."
    elif [[ -d $d ]]; then rm -rf -- "$d"; note "removed $d"
    else note "$d: nothing there"; fi
  done
  rm -f -- "$MANIFEST" "$STATEDIR/.lock"
  rmdir -- "$STATEDIR" 2>/dev/null || true
else
  say
  say "Kept: your keys (keyring, encrypted), $DATADIR, $CONFIGDIR."
  say "Run with --purge to delete those too."
fi
if [[ -d $BACKUPDIR ]] && [[ -n "$(ls -A -- "$BACKUPDIR" 2>/dev/null)" ]]; then
  say "Backups Opal made are in $BACKUPDIR (Opal never deletes them):"
  for f in "$BACKUPDIR"/*; do note "$f"; done
fi
say "Opal removed."
