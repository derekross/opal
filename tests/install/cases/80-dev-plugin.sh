#!/usr/bin/env bash
source "$T/lib.sh"

case_ "1. dev-plugin.sh refuses a checkout and a link"
fresh; mkdir -p "$P/.git"; r=$(devsync)
expect "refused" [ "$r" = 1 ] && said "is a checkout" && ok
fresh; ln -s "$HOME" "$P"; r=$(devsync)
expect "refused" [ "$r" = 1 ] && [ -L "$P" ] && ok

case_ "2. it syncs without deleting, records what it wrote, and skips a file you changed"
fresh; inst >/dev/null; echo mine >"$P/MyTweak.qml"; echo "// edited" >>"$P/Widget.qml"; cp "$P/Widget.qml" "$HOME/w"
r=$(devsync)
expect "exit 0" [ "$r" = 0 ] && expect "added kept" [ -f "$P/MyTweak.qml" ] && expect "edited skipped" same "$HOME/w" "$P/Widget.qml" && said "keeping Widget.qml: you changed it" &&
expect "manifest verifies" manifest_verifies && ok

case_ "3. a fresh sync creates the copy and a later install replaces its files silently"
fresh; r=$(devsync)
expect "exit 0" [ "$r" = 0 ] && [ -f "$P/OpalService.qml" ] && manifest_lists "$P/OpalService.qml" && ok
r=$(inst); expect "exit 0" [ "$r" = 0 ] && expect "nothing kept" not said "keeping" && ok

finish
