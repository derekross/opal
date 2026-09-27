#!/usr/bin/env bash
source "$T/lib.sh"

case_ "1. the v0.2.0 unit is replaced on upgrade; a unit you edited is kept and named"
fresh; mkdir -p "$(dirname "$U")"; cp "$T/fixtures/opal.service.v0.2.0" "$U"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "replaced" same "$ROOT/dist/opal.service" "$U" && expect "no enable on an existing unit" not_logged "enable --now" && ok
echo "Environment=OPAL_LOG=debug" >>"$U"; cp "$U" "$HOME/mine"; : >"$FAKE_LOG"
FAKE_ACTIVE_RC=0 FAKE_EXECSTART="{ path=$BIN/opald }" r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "kept" same "$HOME/mine" "$U" && expect "said" said "Keeping your $U" &&
expect "reloaded" logged "daemon-reload" && expect "restarted (it runs our binary)" logged "restart" && expect "no enable" not_logged "enable" && ok

case_ "2. uninstall stops but keeps an edited unit and tells you what to do"
r=$(uninst)
expect "exit 0" [ "$r" = 0 ] && expect "kept" same "$HOME/mine" "$U" && expect "stopped only" logged "stop opal.service" && not_logged "disable" &&
expect "hint" said "systemctl --user disable opal.service" && ok

case_ "3. a unit that isn't Opal's stops the install; uninstall leaves it"
fresh; mkdir -p "$(dirname "$U")"; printf '[Unit]\nDescription=Mine\n[Service]\nExecStart=/usr/bin/true\n' >"$U"; r=$(inst)
expect "stop" [ "$r" = 1 ] && said "isn't Opal's" && expect "no binary written" [ ! -e "$BIN/opald" ] && ok
r=$(uninst); expect "left" [ -f "$U" ] && said "isn't Opal's; leaving it alone" && not_logged "stop" && ok

case_ "4. a masked or linked unit: the install stops with that message; uninstall leaves it"
fresh; mkdir -p "$(dirname "$U")"; ln -s /dev/null "$U"; r=$(inst)
expect "stop" [ "$r" = 1 ] && said "masked or linked" && expect "still the mask" [ "$(readlink "$U")" = /dev/null ] && ok
r=$(uninst); expect "left" [ -L "$U" ] && said "masked or linked" && ok
fresh; mkdir -p "$(dirname "$U")"; cp "$ROOT/dist/opal.service" "$HOME/linked.service"; ln -s "$HOME/linked.service" "$U"; r=$(inst)
expect "linked stops too" [ "$r" = 1 ] && said "masked or linked" && ok

case_ "5. a unit provided from elsewhere on systemd's path stops the install; uninstall leaves it"
fresh; FAKE_FRAGMENT="$HOME/.local/share/systemd/user/opal.service" r=$(inst)
expect "stop" [ "$r" = 1 ] && said "already provided by $HOME/.local/share/systemd/user/opal.service" && expect "nothing written" [ ! -e "$U" ] && ok
fresh; inst >/dev/null; FAKE_FRAGMENT="/etc/systemd/user/opal.service" FAKE_EXECSTART="{ path=$BIN/opald }" r=$(uninst)
expect "left, stopped because it runs our binary" [ "$r" = 0 ] && said "comes from /etc/systemd/user/opal.service" && logged "stop opal.service" && not_logged "disable" && ok

case_ "6. drop-ins are mentioned and untouched; uninstall of Opal's own unit disables and removes it"
fresh; mkdir -p "$U.d"; echo "[Service]" >"$U.d/override.conf"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && said "drop-ins are yours" && [ -f "$U.d/override.conf" ] && ok
FAKE_FRAGMENT="$U" r=$(uninst)
expect "disabled" logged "disable --now opal.service" && expect "removed" [ ! -e "$U" ] && expect "drop-in kept" [ -f "$U.d/override.conf" ] && ok

finish
