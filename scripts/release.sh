#!/usr/bin/env bash
# Two-step release helper, written for a protected `main` (changes arrive by
# pull request, nobody pushes to it directly).
#
#   scripts/release.sh prepare 0.2.0   # branch + version bump + pull request
#   # ... review, let CI pass, merge the pull request ...
#   scripts/release.sh tag 0.2.0       # tag the merged commit
#   git push origin v0.2.0             # starts the release workflow
#
# A pre-release uses a numeric suffix (`0.2.0-1`; any version with a hyphen is
# one). It must be numeric because the Windows MSI installer cannot represent
# anything else, so no "-rc.1".
#
# See RELEASING.md.
set -euo pipefail

die() { echo "release.sh: $*" >&2; exit 1; }

usage() { die "usage: scripts/release.sh prepare|tag X.Y.Z[-N]"; }

[[ $# -eq 2 ]] || usage
command="$1"
version="$2"

[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9]{1,5})?$ ]] \
  || die "bad version '${version}': use X.Y.Z or X.Y.Z-N (numeric pre-release suffix only)"
[[ $version != *-* || ${version##*-} -le 65535 ]] \
  || die "pre-release suffix must be at most 65535 (MSI limit)"
tag="v$version"

cd "$(git rev-parse --show-toplevel)"
git fetch --quiet origin main
git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null && die "tag $tag already exists locally"
[[ -z "$(git ls-remote --tags origin "refs/tags/$tag")" ]] || die "tag $tag already exists on origin"

# The version in [workspace.package] of the Cargo.toml read from stdin.
workspace_version() {
  awk '/^\[workspace\.package\]/ { in_section = 1; next }
       /^\[/                     { in_section = 0 }
       in_section && /^version = / { gsub(/"/, "", $3); print $3; exit }'
}

prepare() {
  [[ -z "$(git status --porcelain)" ]] || die "the working tree has uncommitted changes"
  local branch="release/$tag"
  git rev-parse --quiet --verify "refs/heads/$branch" >/dev/null && die "branch $branch already exists"
  [[ "$(git show origin/main:Cargo.toml | workspace_version)" != "$version" ]] \
    || die "origin/main is already at version $version"

  # Branch from origin/main, not from wherever HEAD happens to be.
  git switch --quiet --create "$branch" origin/main

  # The version lives in [workspace.package] only; both crates and the Tauri
  # bundle follow it.
  sed -i.bak -E '/^\[workspace\.package\]/,/^\[/ s/^version = ".*"$/version = "'"$version"'"/' Cargo.toml
  rm -f Cargo.toml.bak
  [[ "$(workspace_version < Cargo.toml)" == "$version" ]] || die "could not set the version in Cargo.toml"

  # Refresh the workspace members' versions in Cargo.lock without touching
  # any dependency.
  cargo update --workspace --quiet

  git add Cargo.toml Cargo.lock
  git commit --quiet -m "Release $tag"
  git push --quiet --set-upstream origin "$branch"

  if command -v gh >/dev/null; then
    gh pr create --base main --head "$branch" --title "Release $tag" \
      --body "Bumps the version to $version. After this is merged, run \`scripts/release.sh tag $version\` and push the tag."
  else
    echo "Open a pull request from $branch into main."
  fi
  echo
  echo "Once it is merged:  scripts/release.sh tag $version"
}

tag_release() {
  local merged
  merged="$(git show origin/main:Cargo.toml | workspace_version)"
  [[ "$merged" == "$version" ]] \
    || die "origin/main is at version '${merged}', not $version: merge the release pull request first"

  git tag -a "$tag" origin/main -m "Release $tag"
  echo "Tagged $(git rev-parse --short "$tag^{commit}") on origin/main as $tag. Review with: git show $tag"
  echo "Then start the release with:  git push origin $tag"
}

case "$command" in
  prepare) prepare ;;
  tag) tag_release ;;
  *) usage ;;
esac
