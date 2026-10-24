#!/bin/sh
# Writes SHA256SUMS over a release's assets and signs it with the project's
# release key (LP-139, ADR 0042):
#
#   scripts/sign-release.sh ASSET_DIR VERSION [SECRET_KEY]
#
# ASSET_DIR holds the tarballs and the APK and receives SHA256SUMS (bare file
# names, so it checks from inside the directory) and SHA256SUMS.minisig.
# SECRET_KEY is the release key as an unencrypted minisign secret key file;
# without one the script makes a key for the run and warns, so a release built
# without the project's key still carries a checksum file but `lpctl upgrade`
# refuses it. The trusted comment is "ligature-protocol vVERSION": lpctl checks
# it against the version it asked for, so one release's checksums cannot stand
# in for another's. Signs with rsign (rsign2) or minisign, whichever is on PATH;
# the release workflow runs this on the collected assets.
set -eu

dir=$1
version=$2
key=${3:-}
comment="ligature-protocol v$version"

cd "$dir"
rm -f SHA256SUMS SHA256SUMS.minisig
assets=
for f in *.tar.gz *.apk; do
    [ -e "$f" ] && assets="$assets $f"
done
[ -n "$assets" ] || { echo "sign-release.sh: no tarball or APK in $dir" >&2; exit 1; }
# shellcheck disable=SC2086
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum $assets > SHA256SUMS
else
    shasum -a 256 $assets > SHA256SUMS
fi

if command -v rsign >/dev/null 2>&1; then
    tool=rsign
elif command -v minisign >/dev/null 2>&1; then
    tool=minisign
else
    echo "sign-release.sh: neither rsign nor minisign is on PATH" >&2
    exit 1
fi

if [ -z "$key" ]; then
    echo "sign-release.sh: warning: no release key given; signing with a key made for this run" >&2
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    key=$tmp/release.key
    case $tool in
        rsign) rsign generate -W -f -s "$key" -p "$tmp/release.pub" </dev/null >/dev/null ;;
        minisign) minisign -G -W -f -s "$key" -p "$tmp/release.pub" </dev/null >/dev/null ;;
    esac
fi

case $tool in
    rsign) rsign sign -W -s "$key" -x SHA256SUMS.minisig -t "$comment" SHA256SUMS </dev/null >/dev/null ;;
    minisign) minisign -S -W -s "$key" -x SHA256SUMS.minisig -t "$comment" -m SHA256SUMS </dev/null >/dev/null ;;
esac
cat SHA256SUMS
echo "signed as SHA256SUMS.minisig (trusted comment: $comment)"
