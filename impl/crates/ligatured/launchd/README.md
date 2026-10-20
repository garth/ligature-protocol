# ligatured as a launchd agent (macOS)

Milestone 1 runs a locally built daemon at login as a per-user launchd agent, without code
signing or notarisation. Logs go to `~/Library/Logs/ligature/ligatured.log` (LP-32; no
rotation in this milestone). The data directory is `~/Library/Application Support/ligature`,
named explicitly because an agent has no checkout to resolve the milestone 1 `data/` default
against; point `lpctl` at the same place with `--data-dir` or `LIGATURE_DATA_DIR`.

## Install

```sh
cd impl
cargo install --path crates/ligatured --path crates/lpctl
mkdir -p ~/Library/Logs/ligature ~/Library/LaunchAgents
sed "s|HOME|$HOME|g" crates/ligatured/launchd/org.ligatureprotocol.ligatured.plist \
  > ~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
```

Check it:

```sh
launchctl print gui/$(id -u)/org.ligatureprotocol.ligatured | head
tail -f ~/Library/Logs/ligature/ligatured.log
lpctl --data-dir "$HOME/Library/Application Support/ligature" status
```

## Remove

```sh
launchctl bootout gui/$(id -u)/org.ligatureprotocol.ligatured
rm ~/Library/LaunchAgents/org.ligatureprotocol.ligatured.plist
```

The data directory and log file are left in place; delete them by hand if you want a
fresh start.
