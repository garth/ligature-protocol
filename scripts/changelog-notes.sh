#!/bin/sh
# Prints the CHANGELOG.md section of one version, for the GitHub release notes:
#
#   scripts/changelog-notes.sh 0.1.0
#
# Fails when the version has no section.
set -eu
version=$1
root=$(cd "$(dirname "$0")/.." && pwd)
notes=$(awk -v v="$version" '
    /^## \[/ { if (found) exit; if (index($0, "## [" v "]") == 1) { found = 1; next } }
    found { print }
' "$root/CHANGELOG.md")
[ -n "$notes" ] || { echo "changelog-notes.sh: no section for $version in CHANGELOG.md" >&2; exit 1; }
printf '%s\n' "$notes" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
