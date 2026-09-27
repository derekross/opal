#!/usr/bin/env bash
# Pin a published release in dist/release-checksums.tsv so install.sh can
# accept its tarballs without trusting the release page.
#
#   dist/pin-release.sh v0.3.1
#
# Needs a signed-in GitHub CLI. For each tarball: downloads it, verifies its
# build-provenance attestation, checks the attestation names this
# repository's release workflow at that tag and the tag's own commit, and
# records "<sha256>\t<asset>\t<commit>". Commit the result.
set -euo pipefail
cd "$(dirname "$0")/.."
GITHUB_REPO="derekross/opal"
TABLE="dist/release-checksums.tsv"
tag="${1:?usage: $0 vX.Y.Z}"
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a release tag: $tag" >&2; exit 2; }
gh auth status >/dev/null 2>&1 || { echo "sign in first: gh auth login" >&2; exit 1; }
expected_commit="$(git rev-parse "$tag^{commit}")"

dir="$(mktemp -d)"; trap 'rm -rf -- "$dir"' EXIT
for arch in x86_64 aarch64; do
  asset="opal-$tag-$arch-linux.tar.gz"
  echo "$asset"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$dir/$asset" "https://github.com/$GITHUB_REPO/releases/download/$tag/$asset"
  json="$(gh attestation verify "$dir/$asset" --repo "$GITHUB_REPO" --format json)"
  workflow="$(jq -r '.[0].verificationResult.statement.predicate.buildDefinition.externalParameters.workflow | "\(.repository)@\(.ref)"' <<<"$json")"
  commit="$(jq -r '.[0].verificationResult.statement.predicate.buildDefinition.resolvedDependencies[0].digest.gitCommit' <<<"$json")"
  [[ $workflow == "https://github.com/$GITHUB_REPO@refs/tags/$tag" ]] || { echo "  attestation is for $workflow, not this tag" >&2; exit 1; }
  [[ $commit == "$expected_commit" ]] || { echo "  attestation names commit $commit, tag is at $expected_commit" >&2; exit 1; }
  sha="$(sha256sum -- "$dir/$asset" | cut -d' ' -f1)"
  if grep -qF -- "	$asset	" "$TABLE" 2>/dev/null; then
    grep -qF -- "$sha	$asset	$commit" "$TABLE" && { echo "  already pinned"; continue; }
    echo "  $asset is already pinned with a different hash; a release must not change" >&2; exit 1
  fi
  printf '%s\t%s\t%s\n' "$sha" "$asset" "$commit" >>"$TABLE"
  echo "  pinned $sha (built from $commit)"
done
echo "Now commit $TABLE."
