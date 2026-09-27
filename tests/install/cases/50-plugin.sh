#!/usr/bin/env bash
source "$T/lib.sh"

case_ "1. everything you add to the plugin copy survives reinstall and uninstall, and is named"
fresh; inst >/dev/null
echo mine >"$P/MyTweak.qml"; mkdir -p "$P/notes/deep" "$P/empty"; echo n >"$P/notes/deep/a.txt"
ln -s /etc/hostname "$P/link"; ln -s /nowhere "$P/dangling"; rm "$P/Widget.qml"; ln -s /tmp/my-widget.qml "$P/Widget.qml"
echo "// edited" >>"$P/AppsView.qml"; cp "$P/AppsView.qml" "$HOME/apps.qml"
: >"$FAKE_LOG"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] &&
expect "added file" [ "$(cat "$P/MyTweak.qml")" = mine ] && said "keeping MyTweak.qml" &&
expect "nested folder" [ -f "$P/notes/deep/a.txt" ] && said "keeping notes/" &&
expect "empty folder" [ -d "$P/empty" ] &&
expect "links" [ "$(readlink "$P/link")" = /etc/hostname ] && [ "$(readlink "$P/dangling")" = /nowhere ] &&
expect "link in place of a shipped file" [ -L "$P/Widget.qml" ] && said "Opal's Widget.qml isn't installed" &&
expect "edited shipped file" same "$HOME/apps.qml" "$P/AppsView.qml" && said "keeping AppsView.qml: you changed it" &&
expect "edited file unlisted" not manifest_lists "$P/AppsView.qml" &&
expect "untouched shipped file replaced" same "$ROOT/shell-plugin/OpalService.qml" "$P/OpalService.qml" &&
expect "no plugin enable" not_logged "plugin enable" &&
expect "manifest verifies" manifest_verifies && ok
: >"$FAKE_LOG"; r=$(uninst)
expect "exit 0" [ "$r" = 0 ] &&
expect "yours kept" [ "$(cat "$P/MyTweak.qml")" = mine ] && [ -f "$P/notes/deep/a.txt" ] && [ -d "$P/empty" ] && [ -L "$P/link" ] && [ -L "$P/dangling" ] && [ -L "$P/Widget.qml" ] && same "$HOME/apps.qml" "$P/AppsView.qml" &&
expect "Opal's gone" [ ! -e "$P/OpalService.qml" ] && [ ! -e "$P/manifest.json" ] &&
expect "folder kept and named" [ -d "$P" ] && said "kept $P" &&
expect "disable not run, hint given" not_logged "plugin disable derekross.opal" && said "omarchy plugin disable derekross.opal" &&
expect "manifest gone" [ ! -e "$M" ] && ok

case_ "2. an untouched copy: uninstall removes the folder and disables the plugin"
fresh; inst >/dev/null; : >"$FAKE_LOG"; r=$(uninst)
expect "exit 0" [ "$r" = 0 ] && expect "gone" [ ! -e "$P" ] && expect "disabled" logged "plugin disable derekross.opal" && ok

case_ "3. a git checkout and a symlinked folder are never touched"
fresh; mkdir -p "$P/.git"; echo theirs >"$P/OpalService.qml"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && [ "$(cat "$P/OpalService.qml")" = theirs ] && said "it's a checkout" && ok
r=$(uninst); expect "left" [ -d "$P/.git" ] && said "omarchy plugin remove derekross.opal" && ok
fresh; mkdir "$HOME/elsewhere"; ln -s "$HOME/elsewhere" "$P"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && [ -L "$P" ] && [ -z "$(ls -A "$HOME/elsewhere")" ] && said "it's a link" && ok

case_ "4. a folder that can't be read fully: nothing in it is touched"
fresh; inst >/dev/null; mkdir "$P/private"; echo s >"$P/private/x"; chmod 000 "$P/private"; cp -r "$P" "$HOME/before" 2>/dev/null || true
r=$(inst); chmod 755 "$P/private"
expect "exit 0" [ "$r" = 0 ] && said "couldn't read all of $P" && said "leaving $P as it is" && expect "still old" [ -f "$P/private/x" ] && ok
chmod 000 "$P/private"; r=$(uninst); chmod 755 "$P/private"
expect "uninstall leaves it" [ -f "$P/OpalService.qml" ] && said "leaving $P as it is" && ok

case_ "5. a file Opal no longer ships is removed when it still matches its record"
fresh; inst >/dev/null; echo gone >"$P/Gone.qml"
h=$(sha256sum "$P/Gone.qml" | cut -c1-64); printf '%s\t%s\n' "$h" "$P/Gone.qml" >>"$M"
r=$(inst); expect "removed" [ ! -e "$P/Gone.qml" ] && said "removing Gone.qml: Opal no longer ships it" && ok

case_ "6. a folder where Opal ships a file: kept, Opal's file not installed"
fresh; inst >/dev/null; rm "$P/util.js"; mkdir "$P/util.js"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && [ -d "$P/util.js" ] && said "a folder where Opal ships a file" && ok

case_ "7. names that would be unsafe in a pattern are handled literally"
fresh; inst >/dev/null; echo x >"$P/.*"; echo y >"$P/a[b"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && [ -f "$P/.*" ] && [ -f "$P/a[b" ] && expect "manifest intact" manifest_lists "$P/OpalService.qml" && manifest_verifies && ok

finish
