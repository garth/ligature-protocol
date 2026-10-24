# ligatured as a launchd agent (macOS)

The daemon runs at login as a per-user launchd agent from `~/Applications/Ligature.app`, a
minimal application bundle signed ad hoc and not notarised. macOS shows notifications only
from a process inside a bundle, so the agent runs the binary inside it (LP-94); `lpctl` and
`lpview` stay in `~/.local/bin`. Logs go to `~/Library/Logs/ligature/ligatured.log`
(LP-32; no rotation). The data directory is `~/Library/Application Support/ligature` and
the control socket is in `$TMPDIR/ligature`, the platform defaults since LP-85, so `lpctl`
needs no `--data-dir`. The UDP port is fixed at 47471 so a firewall rule can let peers
dial in.

## Install

From a release tarball, `./install.sh` does all of this. From a checkout:

```sh
cd impl
cargo build --release -p ligatured -p lpctl -p lpview
../packaging/install.sh target/release
```

By hand, which is what the script does:

```sh
mkdir -p ~/.local/bin ~/Applications ~/Library/Logs/ligature ~/Library/LaunchAgents
install -m 755 target/release/lpctl target/release/lpview ~/.local/bin/
../packaging/macos-bundle.sh target/release ~/Applications
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

On its first start from the bundle the daemon asks for permission to post notifications,
and macOS shows the prompt under the name Ligature. Once it is allowed, Ligature is listed
under System Settings → Notifications and the daemon displays your other devices'
notifications, removes them when they are dismissed elsewhere, and tells your other
devices when you close one on the Mac. Refused, it displays nothing, and the log says how
to turn it on. Run by hand from `target/`, outside the bundle, the daemon displays nothing
either.

`LIGATURE_CODESIGN_IDENTITY` names a Developer ID identity for `macos-bundle.sh` to sign
with instead of ad hoc, with the hardened runtime that notarisation needs.

## Remove

`./uninstall.sh` from the tarball or `packaging/uninstall.sh` from a checkout, or by hand:

```sh
launchctl bootout gui/$(id -u)/org.ligatureprotocol.ligatured
rm ~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
rm -rf ~/Applications/Ligature.app
```

The data directory and log file are left in place; delete them by hand if you want a
fresh start.
