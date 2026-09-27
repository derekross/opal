#!/usr/bin/env bash
# Installer tests: each case runs dist/install.sh / dist/uninstall.sh
# against a fresh temporary HOME, with systemctl, xdg-mime, omarchy and
# secret-tool replaced by stubs that only log what they were asked.
#
#   tests/install/run.sh                 with placeholder binaries
#   OPAL_TEST_BINARIES=target/release tests/install/run.sh   with real ones
#   tests/install/run.sh 50              only cases/50-*.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export ROOT T="$ROOT/tests/install"

# Binaries to install: real ones if given, else placeholders.
BINS="$(mktemp -d)"
mkdir -p "$BINS/release"
if [[ -n ${OPAL_TEST_BINARIES:-} ]]; then
  cp -- "$OPAL_TEST_BINARIES/opald" "$OPAL_TEST_BINARIES/opal" "$BINS/release/"
else
  printf '#!/bin/sh\necho fake opald %s\n' "$RANDOM" >"$BINS/release/opald"
  printf '#!/bin/sh\necho fake opal %s\n' "$RANDOM" >"$BINS/release/opal"
  chmod +x "$BINS/release/opald" "$BINS/release/opal"
fi
export BINS
export PATH="$T/stubs:$PATH"

total=0 failed=0
for c in "$T"/cases/${1:-}*.sh; do
  [[ -f $c ]] || continue
  out="$(bash "$c" 2>&1)"; rc=$?
  printf '%s\n' "$out"
  n="$(grep -c '^   ok' <<<"$out")"; f="$(grep -c '^   FAIL' <<<"$out")"
  total=$((total + n + f)); failed=$((failed + f))
  (( rc == 0 )) || { echo "   FAIL: $(basename "$c") exited $rc"; failed=$((failed + 1)); }
done
rm -rf -- "$BINS"
echo
echo "$total checks, $failed failed"
(( failed == 0 ))
