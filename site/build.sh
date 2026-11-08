#!/usr/bin/env bash
# Builds the documentation site (LP-181) into build/site and checks its links:
#
#   site/build.sh
#
# mdBook renders docs/ as docs/SUMMARY.md lists it, with the lp-book preprocessor
# (impl/crates/lp-book, built here with cargo) sending links that leave the site to
# GitHub, and lychee then checks every link and anchor of the built pages against the
# files, so a broken one fails the build, as CI and the Pages workflow run it. The tools
# are taken from PATH when they are there, and otherwise fetched once, pinned by version
# and checksum, into build/tools (Linux x86_64 only; elsewhere install them on PATH:
# `cargo install mdbook lychee`, or the platform's packages).
set -euo pipefail

MDBOOK_VERSION=0.5.4
MDBOOK_SHA256=3f28de05dafca9d0f2eab99c662116b0e37b89b1d96a08f8f430b9eeae958cd7
LYCHEE_VERSION=0.24.2
LYCHEE_SHA256=1f4e0ef7f6554a6ed33dd7ac144fb2e1bbed98598e7af973042fc5cd43951c9a

root=$(cd "$(dirname "$0")/.." && pwd)
tools=$root/build/tools
mkdir -p "$tools"

# fetch NAME URL SHA256: puts the tool at $tools/NAME.
fetch() {
    local name=$1 url=$2 sum=$3
    if [ -x "$tools/$name" ]; then
        return
    fi
    if command -v "$name" >/dev/null 2>&1; then
        ln -sf "$(command -v "$name")" "$tools/$name"
        return
    fi
    if [ "$(uname -sm)" != "Linux x86_64" ]; then
        echo "build.sh: $name is not on PATH; install it (cargo install $name)" >&2
        exit 1
    fi
    local archive=$tools/$name.tar.gz unpack=$tools/$name.unpack
    curl -sfL -o "$archive" "$url"
    echo "$sum  $archive" | sha256sum -c --quiet
    rm -rf "$unpack"
    mkdir -p "$unpack"
    tar xzf "$archive" -C "$unpack"
    local binary
    binary=$(find "$unpack" -type f -name "$name" | head -n 1)
    [ -n "$binary" ] || { echo "build.sh: no $name in $url" >&2; exit 1; }
    mv "$binary" "$tools/$name"
    chmod +x "$tools/$name"
    rm -rf "$unpack" "$archive"
}

fetch mdbook \
    "https://github.com/rust-lang/mdBook/releases/download/v$MDBOOK_VERSION/mdbook-v$MDBOOK_VERSION-x86_64-unknown-linux-gnu.tar.gz" \
    "$MDBOOK_SHA256"
fetch lychee \
    "https://github.com/lycheeverse/lychee/releases/download/lychee-v$LYCHEE_VERSION/lychee-x86_64-unknown-linux-gnu.tar.gz" \
    "$LYCHEE_SHA256"

# The preprocessor, so that mdBook's `cargo run` of it is a start, not a build.
cargo build --quiet --manifest-path "$root/impl/Cargo.toml" -p lp-book

rm -rf "$root/build/site"
"$tools/mdbook" build "$root/site"

# Every link and anchor of the built pages resolves, offline, to a file that exists.
# The 404 page is addressed from the site's root on Pages, not from this directory.
"$tools/lychee" --offline --include-fragments --no-progress \
    --root-dir "$root/build/site" \
    --exclude-path "$root/build/site/404.html" \
    "$root/build/site/**/*.html"
echo "site built in build/site"
