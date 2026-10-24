#!/bin/sh
# Installs ligatured, lpctl and lpview for the current user and runs the daemon as a
# service: a systemd user unit on Linux, a launchd agent on macOS (LP-85).
#
#   ./install.sh [SOURCE_DIR] [--no-start]
#
# SOURCE_DIR holds the two binaries; it defaults to this script's directory,
# which is the case in a release tarball. From a checkout, point it at
# impl/target/release after `cargo build --release -p ligatured -p lpctl -p lpview`.
# lpview, the screen viewer, is installed when it is there.
# --no-start installs and enables the service without starting it, for a
# machine where a daemon is still running by hand.
#
# Binaries go to ~/.local/bin (LIGATURE_BIN_DIR overrides). State lives in the
# platform data directory and the control socket in the platform runtime
# directory, both defaults of the binaries, so lpctl needs no --data-dir. A
# daemon that was run by hand with --data-dir keeps its identity if that
# directory is moved to the platform location before the service starts:
#   Linux: mv data ~/.local/share/ligature
#   macOS: mv data "$HOME/Library/Application Support/ligature"
set -eu

here=$(cd "$(dirname "$0")" && pwd)
source_dir=$here
start=yes
for arg in "$@"; do
    case $arg in
        --no-start) start=no ;;
        --help|-h) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) source_dir=$(cd "$arg" && pwd) ;;
    esac
done

bin_dir=${LIGATURE_BIN_DIR:-$HOME/.local/bin}
for b in ligatured lpctl; do
    [ -x "$source_dir/$b" ] || { echo "install.sh: $source_dir/$b not found" >&2; exit 1; }
done

# The unit files: beside this script in a tarball, under the crate in a checkout.
unit_dir=$here
[ -e "$here/ligatured.service" ] || [ -e "$here/org.ligatureprotocol.ligatured.plist" ] \
    || unit_dir=$here/../impl/crates/ligatured

mkdir -p "$bin_dir"
install -m 755 "$source_dir/ligatured" "$source_dir/lpctl" "$bin_dir/"
installed="ligatured and lpctl"
if [ -x "$source_dir/lpview" ]; then
    install -m 755 "$source_dir/lpview" "$bin_dir/"
    installed="ligatured, lpctl and lpview"
fi
echo "installed $installed in $bin_dir"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "note: $bin_dir is not on your PATH" ;;
esac

case $(uname -s) in
Linux)
    unit=$unit_dir/ligatured.service
    [ -e "$unit" ] || unit=$unit_dir/systemd/ligatured.service
    mkdir -p "$HOME/.config/systemd/user"
    install -m 644 "$unit" "$HOME/.config/systemd/user/ligatured.service"
    systemctl --user daemon-reload
    if [ "$start" = yes ]; then
        if systemctl --user is-active --quiet ligatured; then
            systemctl --user restart ligatured
            echo "restarted the ligatured user service"
        else
            systemctl --user enable --now ligatured
            echo "enabled and started the ligatured user service"
        fi
    else
        systemctl --user enable ligatured
        echo "enabled the ligatured user service; start it with: systemctl --user start ligatured"
    fi
    echo "logs: journalctl --user -u ligatured -f"
    ;;
Darwin)
    plist=$unit_dir/org.ligatureprotocol.ligatured.plist
    [ -e "$plist" ] || plist=$unit_dir/launchd/org.ligatureprotocol.ligatured.plist
    # A downloaded binary carries the quarantine attribute, which Gatekeeper
    # would act on; the release binaries are ad hoc signed by the linker.
    xattr -d com.apple.quarantine "$bin_dir/ligatured" "$bin_dir/lpctl" "$bin_dir/lpview" 2>/dev/null || true
    mkdir -p "$HOME/Library/Logs/ligature" "$HOME/Library/LaunchAgents"
    target=$HOME/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
    sed "s|HOME|$HOME|g" "$plist" > "$target"
    domain=gui/$(id -u)
    if launchctl print "$domain/org.ligatureprotocol.ligatured" >/dev/null 2>&1; then
        launchctl bootout "$domain/org.ligatureprotocol.ligatured" || true
    fi
    if [ "$start" = yes ]; then
        launchctl bootstrap "$domain" "$target"
        echo "loaded the ligatured launchd agent"
    else
        echo "agent written to $target; load it with: launchctl bootstrap $domain $target"
    fi
    echo "logs: tail -f ~/Library/Logs/ligature/ligatured.log"
    ;;
*)
    echo "install.sh: no service for $(uname -s); the binaries are installed" >&2
    ;;
esac
echo "status: lpctl status"
