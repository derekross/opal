#!/usr/bin/env bash
source "$T/lib.sh"

# The download itself needs the network; the check it must pass doesn't.
check() { (cd "$ROOT" && source dist/lib.sh && verify_pinned "$@" >"$HOME/out.txt" 2>&1); }

case_ "1. a tarball is accepted only when it matches the hash pinned in the checkout"
fresh; echo "release bytes" >"$HOME/asset.tar.gz"; h=$(sha256sum "$HOME/asset.tar.gz" | cut -c1-64)
printf '# pins\n%s\topal-v9.9.9-x86_64-linux.tar.gz\tdeadbeef\t%s\n' "$h" "$(stat -c %s "$HOME/asset.tar.gz")" >"$HOME/pins.tsv"
export OPAL_RELEASE_CHECKSUMS="$HOME/pins.tsv"
expect "matching" check "$HOME/asset.tar.gz" opal-v9.9.9-x86_64-linux.tar.gz && ok
echo "tampered" >>"$HOME/asset.tar.gz"
expect "oversized refused before hashing" not check "$HOME/asset.tar.gz" opal-v9.9.9-x86_64-linux.tar.gz && said "isn't the pinned size" && ok
printf 'release bytez\n' >"$HOME/asset.tar.gz"
expect "same size, other bytes refused" not check "$HOME/asset.tar.gz" opal-v9.9.9-x86_64-linux.tar.gz && said "doesn't match the checksum pinned" && ok
expect "unpinned asset refused" not check "$HOME/asset.tar.gz" opal-v9.9.9-aarch64-linux.tar.gz && said "no pinned checksum" && ok
unset OPAL_RELEASE_CHECKSUMS

case_ "2. every pinned tarball is well formed and names its tag's commit; the manifest's version is pinned once released"
v="$(jq -r .version "$ROOT/manifest.json")"
n=0
while IFS=$'\t' read -r h a c z; do
  [[ $h == \#* || -z $a ]] && continue
  n=$((n + 1)); tag="${a#opal-}"; tag="${tag%%-*}"
  expect "hash shape ($a)" grep -qE '^[0-9a-f]{64}$' <<<"$h" &&
  expect "size pinned ($a)" grep -qE '^[0-9]+$' <<<"$z" &&
  { ! (cd "$ROOT" && git rev-parse -q --verify "$tag^{commit}" >/dev/null 2>&1) \
    || expect "commit is the tag's ($a)" [ "$c" = "$(cd "$ROOT" && git rev-parse "$tag^{commit}")" ]; }
done <"$ROOT/dist/release-checksums.tsv"
expect "at least one pinned release" [ "$n" -gt 0 ] && ok
if grep -qF "	opal-v$v-x86_64-linux.tar.gz	" "$ROOT/dist/release-checksums.tsv"; then
  expect "both architectures pinned for $v" grep -qF "	opal-v$v-aarch64-linux.tar.gz	" "$ROOT/dist/release-checksums.tsv" && ok
else
  echo "   ok (v$v isn't pinned yet: dist/pin-release.sh runs after the release is published)"
fi

case_ "3. install --prebuilt refuses before downloading when this version isn't pinned"
fresh; export OPAL_RELEASE_CHECKSUMS="$HOME/empty.tsv"; : >"$HOME/empty.tsv"
r=$( (cd "$ROOT" && ./dist/install.sh --prebuilt </dev/null >"$HOME/out.txt" 2>&1); echo $?)
expect "stopped" [ "$r" = 1 ] && said "has no pinned checksum" && expect "nothing written" [ ! -e "$BIN/opald" ] && ok
unset OPAL_RELEASE_CHECKSUMS

finish
