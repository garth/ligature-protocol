# ligatured as a launchd agent (macOS)

The daemon runs at login as a per-user launchd agent, without code signing or
notarisation. Logs go to `~/Library/Logs/ligature/ligatured.log` (LP-32; no rotation). The
data directory is `~/Library/Application Support/ligature` and the control socket is in
`$TMPDIR/ligature`, the platform defaults since LP-85, so `lpctl` needs no `--data-dir`.
The UDP port is fixed at 47471 so a firewall rule can let peers dial in.

## Install

From a release tarball, `./install.sh` does all of this. From a checkout:

```sh
cd impl
cargo build --release -p ligatured -p lpctl
../packaging/install.sh target/release
```

By hand, which is what the script does:

```sh
mkdir -p ~/.local/bin ~/Library/Logs/ligature ~/Library/LaunchAgents
install -m 755 target/release/ligatured target/release/lpctl ~/.local/bin/
sed "s|HOME|$HOME|g" crates/ligatured/launchd/org.ligatureprotocol.ligatured.plist \
  > ~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
```

Check it:

```sh
launchctl print gui/$(id -u)/org.ligatureprotocol.ligatured | head
tail -f ~/Library/Logs/ligature/ligatured.log
lpctl status
```

A daemon that was run by hand with `--data-dir` keeps its identity by moving that directory
to `~/Library/Application Support/ligature` before the agent is loaded.

## Notifications from other devices

The daemon shows them with AppleScript's `display notification`, which posts as Script
Editor. On a Mac where Script Editor has never posted a notification they are dropped
silently, because a command-line `osascript` never triggers the permission prompt. Once:
open Script Editor, run `display notification "Ligature" with title "LP"` and click Allow.
Script Editor then appears under System Settings → Notifications and the daemon's posts show
from then on, with no restart.

## Remove

`./uninstall.sh` from the tarball or `packaging/uninstall.sh` from a checkout, or by hand:

```sh
launchctl bootout gui/$(id -u)/org.ligatureprotocol.ligatured
rm ~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
```

The data directory and log file are left in place; delete them by hand if you want a
fresh start.
