#!/bin/sh
# Prints the project version: `version` under `[workspace.package]` in
# impl/Cargo.toml, the one version every crate and the Android app carry (LP-85).
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"$/\1/p;}' "$root/impl/Cargo.toml"
