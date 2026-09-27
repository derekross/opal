#!/usr/bin/env bash
source "$T/lib.sh"

case_ "1. an unrecorded opald (a 0.2/0.3 source build): non-interactive run stops before any write"
fresh; printf 'old build\n' >"$BIN/opald"; chmod +x "$BIN/opald"; cp "$BIN/opald" "$HOME/old"; r=$(inst)
expect "exit 1" [ "$r" = 1 ] &&
expect "names it" said "$BIN/opald exists but isn't recorded" &&
expect "names the flag" said "--replace-existing=$BIN/opald" &&
expect "untouched" same "$HOME/old" "$BIN/opald" &&
expect "nothing else written" [ ! -e "$U" ] && [ ! -e "$P" ] && ok

case_ "2. --replace-existing=<path>: replaced, the old file kept as a backup, recorded"
r=$(inst "--replace-existing=$BIN/opald")
expect "exit 0" [ "$r" = 0 ] &&
expect "new binary" same "$BINS/release/opald" "$BIN/opald" &&
expect "backup exists" [ -n "$(ls "$B"/opald.* 2>/dev/null)" ] &&
expect "backup content" same "$HOME/old" "$B"/opald.* &&
expect "backup recorded" grep -q "^backup	$BIN/opald	$B/opald\." "$M" &&
expect "said where" said "moved the previous $BIN/opald to $B/opald." && ok

case_ "3. the flag consents to one path only"
fresh; printf 'old\n' >"$BIN/opald"; printf 'old\n' >"$BIN/opal"; r=$(inst "--replace-existing=$BIN/opald")
expect "stops on the other" [ "$r" = 1 ] && expect "names opal" said "$BIN/opal exists but isn't recorded" && ok

case_ "4. the interactive prompt: y replaces, n stops"
fresh; printf 'old\n' >"$BIN/opald"
if command -v script >/dev/null; then
  (cd "$ROOT" && printf 'n\n' | CARGO_TARGET_DIR="$BINS" script -qec "./dist/install.sh --no-build" /dev/null >"$HOME/out.txt" 2>&1); r=$?
  expect "declined stops" [ "$r" = 1 ] && expect "untouched" [ "$(cat "$BIN/opald")" = old ] && ok
  (cd "$ROOT" && printf 'y\n' | CARGO_TARGET_DIR="$BINS" script -qec "./dist/install.sh --no-build" /dev/null >"$HOME/out.txt" 2>&1); r=$?
  expect "accepted installs" [ "$r" = 0 ] && expect "replaced" same "$BINS/release/opald" "$BIN/opald" && expect "backup" [ -n "$(ls "$B"/opald.* 2>/dev/null)" ] && ok
else
  echo "   ok (skipped: no 'script' command)"
fi

case_ "5. a symlink or a folder at a binary path stops the install, never followed"
fresh; printf 'target\n' >"$HOME/real"; ln -s "$HOME/real" "$BIN/opald"; r=$(inst)
expect "exit 1" [ "$r" = 1 ] && expect "message" said "is a symbolic link" && expect "target untouched" [ "$(cat "$HOME/real")" = target ] && expect "still a link" [ -L "$BIN/opald" ] && ok
fresh; mkdir "$BIN/opal"; r=$(inst)
expect "exit 1" [ "$r" = 1 ] && expect "message" said "isn't a regular file" && ok

case_ "6. uninstall keeps an unrecorded binary and a link, removes ours, lists backups"
fresh; inst "--replace-existing=$BIN/opald" >/dev/null 2>&1 || true; printf 'old\n' >"$BIN/opald"; inst "--replace-existing=$BIN/opald" >/dev/null
printf 'theirs\n' >"$BIN/opal"; ln -sf /bin/true "$BIN/opald"  # both now not ours
r=$(uninst)
expect "exit 0" [ "$r" = 0 ] &&
expect "unrecorded kept" [ "$(cat "$BIN/opal")" = theirs ] && said "$BIN/opal isn't recorded" &&
expect "link kept" [ -L "$BIN/opald" ] && said "$BIN/opald is a link" &&
expect "backups listed" said "Backups Opal made are in $B" && [ -n "$(ls "$B"/opald.* 2>/dev/null)" ] && ok

case_ "7. a file that changed after the check is kept as a backup, not overwritten"
fresh; inst >/dev/null
# Simulate the race: the record says one thing, the file another.
printf 'edited after check\n' >>"$BIN/opal"
(cd "$ROOT" && source dist/lib.sh && prepare_state && load_manifest && replace_owned "$BINS/release/opal" "$BIN/opal" 755 "${MANIFEST_HASH[$BIN/opal]}" >"$HOME/out.txt" 2>&1)
expect "new file in place" same "$BINS/release/opal" "$BIN/opal" &&
expect "old kept" grep -q "edited after check" "$B"/opal.* &&
expect "said" said "changed after it was checked" && ok

finish
