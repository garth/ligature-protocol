# Upgrading and uninstalling

## Upgrading

Install the new version on every device around the same time: while the project is alpha,
a release may change what travels between devices, and a device of the previous version may
not understand the new one.

**Linux and macOS**: `lpctl upgrade`. It finds the latest release, verifies its checksum
file against the project's release key (see
[Verifying a download](install.md#verifying-a-download)) and the package against the
checksum, then runs the package's `install.sh`, which replaces the binaries and restarts the
service; your keys and devices stay. `lpctl upgrade --check` only says whether a newer
release exists. The same by hand: unpack the new tarball and run `./install.sh` again. A
release that changes the store's layout brings it forward on the next start; the change is
one way, so a copy of the data directory (see [Where things live](install.md#where-things-live))
beforehand is the way back.

Once a day, when you run any `lpctl` command on a terminal, it asks the releases page for the
newest version and, when there is one newer than yours, ends with a line saying so. The
check is `lpctl`'s alone: the daemon never contacts anything, the request carries nothing
about you or your devices, and setting `LIGATURE_NO_UPDATE_CHECK=1` switches it off. The
result is kept in `update-check.json` in the data directory.

**macOS permissions after an upgrade**: the app is signed ad hoc, so to macOS every new
version is a new app. Accessibility, Input Monitoring and Screen Recording are asked for
again, and a switch left on in System Settings for the previous version no longer applies,
even though it still shows as on. Clear the old entries and restart the daemon, then allow
each one when macOS asks:

```sh
tccutil reset Accessibility org.ligatureprotocol.ligatured
tccutil reset PostEvent org.ligatureprotocol.ligatured
tccutil reset ListenEvent org.ligatureprotocol.ligatured
tccutil reset ScreenCapture org.ligatureprotocol.ligatured
launchctl kickstart -k gui/$(id -u)/org.ligatureprotocol.ligatured
```

**Android**: install the new APK over the old one. The release APK is signed with the
project's key, so the update keeps the app's identity and data. The version code grows with
every release, including from a pre-release to the release of the same number.

The [changelog](https://github.com/garth/ligature-protocol/blob/main/CHANGELOG.md) says what
each release changed.

## Uninstalling

**Linux and macOS**: `./uninstall.sh` from the unpacked tarball stops and removes the service
and the binaries. Your keys and store stay in the data directory and the logs where they
were; delete them by hand for a fresh identity:

```sh
rm -r ~/.local/share/ligature                           # Linux
rm -r ~/Library/Application\ Support/ligature ~/Library/Logs/ligature   # macOS
```

**Android**: uninstalling the app removes its keys and store with it; nothing is left
behind, and nothing is backed up to a cloud account or transferred to a new phone.

Before you retire a device, [revoke it](devices.md#revoking-a-device) from another: an
uninstalled device cannot announce its own departure, and your other devices would keep
expecting it.
