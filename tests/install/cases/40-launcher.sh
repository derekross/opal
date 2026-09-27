#!/usr/bin/env bash
source "$T/lib.sh"

case_ "1. the launcher every earlier release wrote is replaced; a line you added makes it yours"
fresh; mkdir -p "$(dirname "$L")"; legacy_launcher >"$L"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "replaced" same <(rendered_launcher) "$L" && ok
echo "# mine" >>"$L"; cp "$L" "$HOME/mine"; : >"$FAKE_LOG"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "kept" same "$HOME/mine" "$L" && said "Keeping $L" && expect "handler untouched" not_logged "xdg-mime default" && ok
r=$(uninst); expect "kept on uninstall" same "$HOME/mine" "$L" && said "isn't the one install.sh wrote" && ok

case_ "2. a symlinked or foreign launcher is kept by both; the install goes on"
fresh; mkdir -p "$(dirname "$L")"; printf '[Desktop Entry]\nName=Mine\n' >"$HOME/real.desktop"; ln -s "$HOME/real.desktop" "$L"; r=$(inst)
expect "exit 0" [ "$r" = 0 ] && [ -L "$L" ] && said "it's a link" && expect "rest installed" [ -f "$BIN/opald" ] && ok
r=$(uninst); expect "still a link" [ -L "$L" ] && ok

case_ "3. a different current handler is asked about; non-interactive keeps it"
fresh; FAKE_MIME=other.desktop r=$(inst)
expect "exit 0" [ "$r" = 0 ] && expect "not set" not_logged "xdg-mime default" && said "Left other.desktop as the nostrconnect:// handler" && ok

case_ "4. uninstall takes only Opal's token out of mimeapps.list, in both sections, via XDG_CONFIG_HOME"
fresh; inst >/dev/null
export XDG_CONFIG_HOME="$HOME/cfg"; mkdir -p "$XDG_CONFIG_HOME"
mv "$HOME/.config/systemd" "$XDG_CONFIG_HOME/" 2>/dev/null || true
cat >"$XDG_CONFIG_HOME/mimeapps.list" <<EOF
[Default Applications]
x-scheme-handler/nostrconnect=foo.desktop;opal-nostrconnect.desktop;
text/plain=vim.desktop

[Added Associations]
x-scheme-handler/nostrconnect=opal-nostrconnect.desktop;
x-scheme-handler/http=firefox.desktop;
EOF
# The launcher lives under XDG_DATA_HOME (unchanged), so uninstall still finds it.
r=$(uninst)
expect "exit 0" [ "$r" = 0 ] &&
expect "default keeps foo" grep -qx 'x-scheme-handler/nostrconnect=foo.desktop;' "$XDG_CONFIG_HOME/mimeapps.list" &&
expect "other keys intact" grep -qx 'text/plain=vim.desktop' "$XDG_CONFIG_HOME/mimeapps.list" &&
expect "added association dropped when only ours" not grep -q 'nostrconnect=opal' "$XDG_CONFIG_HOME/mimeapps.list" &&
expect "http intact" grep -qx 'x-scheme-handler/http=firefox.desktop;' "$XDG_CONFIG_HOME/mimeapps.list" &&
expect "said" said "removed opal-nostrconnect.desktop from the nostrconnect:// entries" && ok
unset XDG_CONFIG_HOME

case_ "5. a symlinked mimeapps.list is not edited"
fresh; inst >/dev/null; printf '[Default Applications]\nx-scheme-handler/nostrconnect=opal-nostrconnect.desktop\n' >"$HOME/real.list"; ln -s "$HOME/real.list" "$HOME/.config/mimeapps.list"
r=$(uninst)
expect "untouched" [ -L "$HOME/.config/mimeapps.list" ] && grep -q opal "$HOME/real.list" && said "isn't a regular file" && ok

finish
