#!/usr/bin/env bash
# Development: copy the shell plugin files into the installed plugin folder
# and reload the shell, without building or restarting the daemon.
#
# Follows the same rule as install.sh (dist/lib.sh): a file is replaced
# only while it is exactly what Opal wrote; a file you changed stays and is
# named; nothing is ever deleted; a checkout made by `omarchy plugin add`
# is left alone (edit it in place instead).
set -euo pipefail

cd "$(dirname "$0")/.."
source dist/lib.sh || { echo "dist/lib.sh is missing: run this from an Opal checkout" >&2; exit 1; }

prepare_state
load_manifest
load_known
load_shipped

case "$(plugin_dir_state "$PLUGIN_PATH")" in
  checkout) die "$PLUGIN_PATH is a checkout (has .git): edit it in place, the shell reloads it." ;;
  symlink | other) die "$PLUGIN_PATH isn't a plain folder; not Opal's, not touched." ;;
  missing) mkdir -p -m 755 -- "$PLUGIN_PATH" ;;
esac

n=0
for rel in "${!SHIPPED[@]}"; do
  dest="$PLUGIN_PATH/$rel"
  case "$(path_kind "$dest")" in
    missing) replace_owned "${SHIPPED[$rel]}" "$dest" 644 "" && (( n++ )) || true ;;
    file)
      if owned_file "$dest" "plugin/$rel"; then
        cmp -s -- "${SHIPPED[$rel]}" "$dest" || { replace_owned "${SHIPPED[$rel]}" "$dest" 644 "$(file_hash "$dest")" && (( n++ )) || true; }
      else
        note "keeping $rel: you changed it (not synced)"
        unrecord "$dest"
      fi ;;
    *) note "keeping $rel: not a file, so not Opal's (not synced)" ;;
  esac
done
say "synced $n file(s) into $PLUGIN_PATH"
omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
