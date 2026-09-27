#!/usr/bin/env bash
source "$T/lib.sh"

case_ "1. --purge without a terminal is refused; nothing is deleted"
fresh; inst >/dev/null; r=$(uninst --purge)
expect "refused" [ "$r" = 1 ] && said "needs a terminal" && expect "binary still there" [ -f "$BIN/opald" ] && ok

case_ "2. without --purge, keys and data stay and the resolved paths are named"
r=$(uninst)
expect "exit 0" [ "$r" = 0 ] && said "Kept: your keys" && said "$HOME/.local/share/opal" && ok

case_ "3. --purge: XDG-resolved folders removed, a decoy untouched, a linked folder not followed, backups kept"
fresh; printf 'old\n' >"$BIN/opald"; inst "--replace-existing=$BIN/opald" >/dev/null
export XDG_DATA_HOME="$HOME/xdg-data" XDG_CONFIG_HOME="$HOME/xdg-cfg" XDG_CACHE_HOME="$HOME/xdg-cache" XDG_STATE_HOME="$HOME/xdg-state"
mkdir -p "$XDG_DATA_HOME/opal" "$XDG_CONFIG_HOME/opal" "$HOME/.local/share/opal" "$HOME/real-cache" "$XDG_CACHE_HOME"
echo db >"$XDG_DATA_HOME/opal/opal.db"; echo decoy >"$HOME/.local/share/opal/decoy"; ln -s "$HOME/real-cache" "$XDG_CACHE_HOME/opal"
mkdir -p "$XDG_STATE_HOME/opal/backup"; echo b >"$XDG_STATE_HOME/opal/backup/opald.20200101T000000"
if command -v script >/dev/null; then
  (cd "$ROOT" && printf 'delete my keys\n' | script -qec "./dist/uninstall.sh --purge" /dev/null >"$HOME/out.txt" 2>&1); r=$?
  expect "exit 0" [ "$r" = 0 ] &&
  expect "data removed" [ ! -e "$XDG_DATA_HOME/opal" ] && said "removed $XDG_DATA_HOME/opal" &&
  expect "config removed" [ ! -e "$XDG_CONFIG_HOME/opal" ] &&
  expect "decoy untouched" [ -f "$HOME/.local/share/opal/decoy" ] &&
  expect "link not followed" [ -L "$XDG_CACHE_HOME/opal" ] && [ -d "$HOME/real-cache" ] && said "is a link; not followed" &&
  expect "backup kept" [ -f "$XDG_STATE_HOME/opal/backup/opald.20200101T000000" ] && said "Backups Opal made are in" &&
  expect "keyring cleared per kind" logged "clear application opal kind account" && logged "clear application opal kind conn-key" && [ ! -s "$FAKE_SECRETS" ] &&
  expect "reported" said "2 Opal item(s) before, 0 left" && ok
else
  echo "   ok (skipped: no 'script' command)"
fi
unset XDG_DATA_HOME XDG_CONFIG_HOME XDG_CACHE_HOME XDG_STATE_HOME

case_ "4. a keyring that refuses is reported, not hidden"
fresh; inst >/dev/null
if command -v script >/dev/null; then
  (cd "$ROOT" && printf 'delete my keys\n' | FAKE_SECRET_RC=1 script -qec "./dist/uninstall.sh --purge" /dev/null >"$HOME/out.txt" 2>&1); r=$?
  expect "exit 0" [ "$r" = 0 ] && said "clearing 'account' items failed" && said "2 left" && ok
else
  echo "   ok (skipped: no 'script' command)"
fi

case_ "5. the typed phrase is required"
fresh; inst >/dev/null
if command -v script >/dev/null; then
  (cd "$ROOT" && printf 'yes\n' | script -qec "./dist/uninstall.sh --purge" /dev/null >"$HOME/out.txt" 2>&1); r=$?
  expect "cancelled" [ "$r" = 1 ] && said "Cancelled" && [ -f "$BIN/opald" ] && ok
else
  echo "   ok (skipped: no 'script' command)"
fi

finish
