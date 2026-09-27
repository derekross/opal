#!/usr/bin/env bash
source "$T/lib.sh"

case_ "1. fresh install writes everything, records it, enables the service once"
fresh; r=$(inst)
expect "exit 0" [ "$r" = 0 ] &&
expect "binaries" same "$BINS/release/opald" "$BIN/opald" &&
expect "unit" same "$ROOT/dist/opal.service" "$U" &&
expect "launcher rendered" same <(rendered_launcher) "$L" &&
expect "plugin file" same "$ROOT/shell-plugin/OpalService.qml" "$P/OpalService.qml" &&
expect "no marker file" [ ! -e "$P/.installed-by-opal" ] &&
expect "manifest verifies" manifest_verifies &&
expect "manifest lists unit" manifest_lists "$U" &&
expect "manifest lists plugin manifest" manifest_lists "$P/manifest.json" &&
expect "enable --now once" [ "$(grep -c 'enable --now' "$FAKE_LOG")" = 1 ] &&
expect "no restart" not_logged "restart" &&
expect "plugin enable" logged "plugin enable derekross.opal" &&
expect "handler set" logged "xdg-mime default opal-nostrconnect.desktop" && ok

case_ "2. reinstall of an untouched install: replaced silently, service state untouched"
r=$(inst)
expect "exit 0" [ "$r" = 0 ] &&
expect "nothing kept" not said "keeping" &&
expect "no enable" [ "$(grep -c 'enable --now' "$FAKE_LOG")" = 1 ] &&
expect "no restart when not running" not_logged "restart" &&
expect "no plugin enable again" [ "$(grep -c 'plugin enable' "$FAKE_LOG")" = 1 ] &&
expect "manifest verifies" manifest_verifies && ok

case_ "3. reinstall while running our binary: restarted; running something else: told, not restarted"
FAKE_ACTIVE_RC=0 FAKE_EXECSTART="{ path=$BIN/opald ; argv[]=$BIN/opald }" r=$(inst)
expect "restart" logged "restart opal.service" && ok
: >"$FAKE_LOG"
FAKE_ACTIVE_RC=0 FAKE_EXECSTART="{ path=/opt/opald ; argv[]=/opt/opald }" r=$(inst)
expect "not restarted" not_logged "restart" &&
expect "told" said "restart it yourself" && ok

case_ "4. a stop in the checks leaves nothing behind"
fresh; mkdir -p "$(dirname "$U")"; printf '[Unit]\nDescription=Mine\n' >"$U"; r=$(inst)
expect "exit 1" [ "$r" = 1 ] &&
expect "no binaries" [ ! -e "$BIN/opald" ] &&
expect "no plugin" [ ! -e "$P" ] &&
expect "no manifest" [ ! -e "$M" ] &&
expect "no temp files" [ -z "$(find "$HOME" -name '.opal.*' -o -name '.installed.*' | head -1)" ] && ok

case_ "5. a failure after the first writes leaves a record of exactly what was written"
fresh; mkdir -p "$HOME/.local/share/applications"; chmod 555 "$HOME/.local/share/applications"; r=$(inst)
chmod 755 "$HOME/.local/share/applications"
expect "failed" [ "$r" != 0 ] &&
expect "binaries recorded" manifest_lists "$BIN/opald" &&
expect "unit recorded" manifest_lists "$U" &&
expect "launcher not recorded" not manifest_lists "$L" &&
expect "manifest verifies" manifest_verifies && ok

case_ "6. unknown options are rejected by both scripts"
fresh
expect "install" [ "$(inst --bogus)" = 2 ] &&
expect "bare --replace-existing" [ "$(inst --replace-existing)" = 1 ] &&
expect "uninstall" [ "$(uninst --purg)" = 2 ] && ok

finish
