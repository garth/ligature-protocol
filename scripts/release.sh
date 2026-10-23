#!/usr/bin/env bash
# Cuts a release (LP-85):
#
#   scripts/release.sh LP-NNN X.Y.Z
#
# Sets the version under [workspace.package] in impl/Cargo.toml (every crate and
# the Android app take theirs from it), refreshes Cargo.lock, moves the
# Unreleased section of CHANGELOG.md under the version with today's date,
# commits as "LP-NNN Release vX.Y.Z" and tags vX.Y.Z. It never pushes: pushing
# the tag starts the release workflow, which builds the installers for every
# platform and publishes the GitHub release with the changelog section as its
# notes.
#
# Needs a clean working tree and a non-empty Unreleased section.
set -euo pipefail

usage() { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
[ $# -eq 2 ] || usage
ticket=$1
version=$2
[[ $ticket =~ ^LP-[0-9]+$ ]] || { echo "release.sh: $ticket is not a ticket key" >&2; exit 2; }
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] \
    || { echo "release.sh: $version is not X.Y.Z or X.Y.Z-pre" >&2; exit 2; }

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "release.sh: the working tree has uncommitted changes" >&2
    exit 1
fi
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
    echo "release.sh: tag v$version exists" >&2
    exit 1
fi
old=$(scripts/version.sh)
[ "$old" != "$version" ] || { echo "release.sh: the version is already $version" >&2; exit 1; }

unreleased=$(awk '/^## \[Unreleased\]/ { found = 1; next } /^## \[/ { if (found) exit } found && NF { print }' CHANGELOG.md)
[ -n "$unreleased" ] || { echo "release.sh: the Unreleased section of CHANGELOG.md is empty" >&2; exit 1; }

cargo=(cargo)
command -v cargo >/dev/null 2>&1 && cargo --version >/dev/null 2>&1 || cargo=(mise exec rust@stable -- cargo)

# The workspace version and the version each path dependency names.
perl -pi -e 's/^(version = )"\Q'"$old"'\E"$/$1"'"$version"'"/ if $in; $in = 1 if /^\[workspace\.package\]/; $in = 0 if /^\[workspace\.dependencies\]/' impl/Cargo.toml
perl -pi -e 's/(path = "crates\/[a-z-]+", version = )"\Q'"$old"'\E"/$1"'"$version"'"/' impl/Cargo.toml
[ "$(scripts/version.sh)" = "$version" ] || { echo "release.sh: failed to set the version" >&2; exit 1; }
(cd impl && "${cargo[@]}" update --workspace --offline --quiet)

today=$(date -u +%F)
perl -0pi -e 's/^## \[Unreleased\]\n/## [Unreleased]\n\n## ['"$version"'] - '"$today"'\n/m' CHANGELOG.md

git add impl/Cargo.toml impl/Cargo.lock CHANGELOG.md
git commit -q -m "$ticket Release v$version"
git tag -a "v$version" -m "Ligature Protocol v$version"
echo "committed and tagged v$version; publish it with: git push origin $(git branch --show-current) v$version"
