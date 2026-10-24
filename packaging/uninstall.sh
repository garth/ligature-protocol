#!/bin/sh
# Stops the ligatured service, removes it and the binaries install.sh put in
# ~/.local/bin (LIGATURE_BIN_DIR overrides). The data directory and the logs
# are left in place: delete them by hand for a fresh identity.
set -eu

bin_dir=${LIGATURE_BIN_DIR:-$HOME/.local/bin}
case $(uname -s) in
Linux)
    if [ -e "$HOME/.config/systemd/user/ligatured.service" ]; then
        systemctl --user disable --now ligatured || true
        rm -f "$HOME/.config/systemd/user/ligatured.service"
        systemctl --user daemon-reload
        echo "removed the ligatured user service"
    fi
    ;;
Darwin)
    target=$HOME/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
    launchctl bootout "gui/$(id -u)/org.ligatureprotocol.ligatured" 2>/dev/null || true
    rm -f "$target"
    echo "removed the ligatured launchd agent"
    ;;
esac
rm -f "$bin_dir/ligatured" "$bin_dir/lpctl" "$bin_dir/lpview"
echo "removed ligatured, lpctl and lpview from $bin_dir"
