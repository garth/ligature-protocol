#!/bin/sh
# Measures the Rust workspace's line coverage by its tests and fails below the
# floors (LP-163). Run from anywhere; needs cargo-llvm-cov and the
# llvm-tools-preview component:
#
#   rustup component add llvm-tools-preview
#   cargo install cargo-llvm-cov --locked
#   scripts/coverage.sh            # the tests, then the report and the floors
#   scripts/coverage.sh --report   # the report and the floors from the last run
#
# Two floors: the whole workspace, and its logic, which is everything but the
# platform glue that needs a compositor, a Mac or a phone, the entry points,
# the CLI and the viewer (GLUE below). The runs with the real devices check the
# glue; the floor on the logic is the one that guards the protocol. The HTML
# report is written to impl/target/llvm-cov/html.
set -eu

FLOOR_ALL=68
FLOOR_LOGIC=85
GLUE='(_linux|_macos)\.rs$|/ligatured/src/(main|run|control|portal_gate)\.rs$|/lpctl/src/main\.rs$|/lpview/|/lp-macos/|/lp-vectors/|/wireguard-integration/|/ligature-android/src/lib\.rs$'

cd "$(dirname "$0")/../impl"
if [ "${1:-}" != "--report" ]; then
    cargo llvm-cov --workspace --no-report
fi
cargo llvm-cov report --html

# Prints the report's total and notes a floor that was missed. The report's
# status is taken before its output is cut down: piped straight into `tail`,
# its failure would be lost and the script would pass below a floor.
missed=""
floor() {
    name=$1
    lines=$2
    shift 2
    echo "== $name (floor ${lines}%)"
    if summary=$(cargo llvm-cov report --summary-only --fail-under-lines "$lines" "$@"); then
        echo "$summary" | tail -1
    else
        echo "$summary" | tail -1
        missed="$missed $name;"
    fi
}

floor "the whole workspace" "$FLOOR_ALL"
floor "the logic, without the platform glue" "$FLOOR_LOGIC" --ignore-filename-regex "$GLUE"
if [ -n "$missed" ]; then
    echo "coverage is below its floor for:$missed" >&2
    exit 1
fi
