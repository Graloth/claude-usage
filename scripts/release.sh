#!/usr/bin/env bash
# Prepares a release: bumps the version, commits and tags it. Does not push.
#
#   scripts/release.sh 0.2.0     # a release
#   scripts/release.sh 0.2.0-1   # a pre-release (any version with a hyphen is)
#
# A pre-release suffix must be numeric: the Windows MSI installer cannot
# represent anything else (so no "-rc.1").
#
# Pushing the tag is what starts the release workflow, see RELEASING.md.
set -euo pipefail

die() { echo "release.sh: $*" >&2; exit 1; }

version="${1:-}"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9]{1,5})?$ ]] \
  || die "usage: scripts/release.sh X.Y.Z[-N]   (numeric pre-release suffix only; got '${version}')"
[[ $version != *-* || ${version##*-} -le 65535 ]] || die "pre-release suffix must be at most 65535 (MSI limit)"
tag="v$version"

cd "$(git rev-parse --show-toplevel)"

[[ "$(git branch --show-current)" == main ]] || die "run this on the main branch"
[[ -z "$(git status --porcelain)" ]] || die "the working tree has uncommitted changes"
git fetch --quiet origin main
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] \
  || die "main is not in sync with origin/main (pull or push first)"
git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null && die "tag $tag already exists"

# The version lives in [workspace.package] only; both crates and the Tauri
# bundle follow it.
sed -i.bak -E '/^\[workspace\.package\]/,/^\[/ s/^version = ".*"$/version = "'"$version"'"/' Cargo.toml
rm -f Cargo.toml.bak
grep -q "^version = \"$version\"$" Cargo.toml || die "could not set the version in Cargo.toml"

# Refresh the workspace members' versions in Cargo.lock without touching
# any dependency.
cargo update --workspace --quiet

git add Cargo.toml Cargo.lock
git commit --quiet -m "Release $tag"
git tag -a "$tag" -m "Release $tag"

echo "Created commit and tag $tag. Review with: git show $tag"
echo "Then publish with:  git push origin main $tag"
