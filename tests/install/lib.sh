# Sourced by every case. A case is a sequence of `case_ "…"` headings, each
# followed by `expect` checks and a final `ok`.
set -u
: "${ROOT:?}" "${T:?}" "${BINS:?}"
FAILS=0

# A new home with nothing in it. Stub knobs reset to "service not running,
# no unit known to systemd, no handler set, keyring with two Opal items".
fresh() {
  export HOME; HOME="$(mktemp -d)"
  unset XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME
  export FAKE_LOG="$HOME/calls.log" FAKE_ACTIVE_RC=3 FAKE_FRAGMENT="" FAKE_EXECSTART="" FAKE_MIME="" FAKE_SECRET_RC=0
  export FAKE_SECRETS="$HOME/fake-secrets"
  printf 'account a1\nconn-key c1\n' >"$FAKE_SECRETS"
  mkdir -p "$HOME/.config/omarchy/plugins" "$HOME/.local/bin"
  : >"$FAKE_LOG"
  P="$HOME/.config/omarchy/plugins/derekross.opal"
  OLDP="$HOME/.config/omarchy/plugins/opal"
  U="$HOME/.config/systemd/user/opal.service"
  L="$HOME/.local/share/applications/opal-nostrconnect.desktop"
  M="$HOME/.local/state/opal/installed.tsv"
  B="$HOME/.local/state/opal/backup"
  BIN="$HOME/.local/bin"
}
inst() { (cd "$ROOT" && CARGO_TARGET_DIR="$BINS" ./dist/install.sh --no-build "$@" </dev/null >"$HOME/out.txt" 2>&1); echo $?; }
uninst() { (cd "$ROOT" && ./dist/uninstall.sh "$@" </dev/null >"$HOME/out.txt" 2>&1); echo $?; }
devsync() { (cd "$ROOT" && ./dist/dev-plugin.sh </dev/null >"$HOME/out.txt" 2>&1); echo $?; }
out() { cat "$HOME/out.txt"; }
said() { grep -qF -- "$1" "$HOME/out.txt"; }
logged() { grep -qF -- "$1" "$FAKE_LOG"; }
not_logged() { ! grep -qF -- "$1" "$FAKE_LOG"; }
manifest_lists() { [[ -f $M ]] && awk -F'\t' -v p="$1" '$2 == p { found = 1 } END { exit !found }' "$M"; }
manifest_verifies() { [[ -f $M ]] && awk -F'\t' '$1 != "backup" { print $1 "  " $2 }' "$M" | (cd / && sha256sum -c --quiet --strict >/dev/null 2>&1); }
same() { cmp -s -- "$1" "$2"; }
# The launcher as install.sh renders it, and as releases before the mark did.
rendered_launcher() { sed "s|@BINDIR@|$BIN|g" "$ROOT/dist/opal-nostrconnect.desktop"; }
legacy_launcher() { rendered_launcher | grep -v '^X-Opal-Source='; }

CASE=""
case_() { CASE=$1; echo "$1"; }
ok() { echo "   ok"; }
fail() { echo "   FAIL: $*"; sed 's/^/     | /' "$HOME/out.txt" | head -40; echo "     | calls: $(tr '\n' ';' <"$FAKE_LOG" | cut -c1-300)"; FAILS=$((FAILS + 1)); }
# expect "<what>" <command...>: runs the command; a failure names it.
expect() { local what=$1; shift; if "$@"; then return 0; fi; fail "$what"; return 1; }
not() { ! "$@"; }
finish() { exit $(( FAILS > 0 )); }
