#!/bin/sh
# Packages the desktop binaries as a release tarball (LP-85):
#
#   packaging/package.sh OS ARCH VERSION BIN_DIR [OUT_DIR]
#
# OS is linux or macos, ARCH is x86_64, aarch64 or universal, BIN_DIR holds
# ligatured, lpctl and lpview. The result is OUT_DIR/ligature-VERSION-OS-ARCH.tar.gz
# (OUT_DIR defaults to dist/ under the repository) and unpacks to one directory
# with the binaries, the service file for the platform, install.sh,
# uninstall.sh, a README and the licence. On macOS the daemon is shipped inside
# Ligature.app, made by macos-bundle.sh, instead of as a bare binary (LP-94).
# The release workflow runs this on each platform; it runs the same way by
# hand.
set -eu

os=$1; arch=$2; version=$3; bin_dir=$4
root=$(cd "$(dirname "$0")/.." && pwd)
out_dir=${5:-$root/dist}
name=ligature-$version-$os-$arch
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
dir=$stage/$name
mkdir -p "$dir" "$out_dir"

install -m 755 "$bin_dir/lpctl" "$bin_dir/lpview" "$dir/"
install -m 755 "$root/packaging/install.sh" "$root/packaging/uninstall.sh" "$dir/"
install -m 644 "$root/LICENSE" "$dir/LICENSE"
case $os in
linux)
    install -m 755 "$bin_dir/ligatured" "$dir/"
    install -m 644 "$root/impl/crates/ligatured/systemd/ligatured.service" "$dir/"
    ;;
macos)
    "$root/packaging/macos-bundle.sh" "$bin_dir" "$dir" >/dev/null
    install -m 755 "$root/packaging/macos-bundle.sh" "$dir/"
    install -m 644 "$root/impl/crates/ligatured/launchd/org.ligatureprotocol.ligatured.plist" "$dir/"
    ;;
*) echo "package.sh: unknown OS $os" >&2; exit 1 ;;
esac

cat > "$dir/README.md" <<EOF
# Ligature Protocol $version ($os, $arch)

\`./install.sh\` puts \`lpctl\` and \`lpview\` in \`~/.local/bin\`, and the daemon \`ligatured\` there too
on Linux or in \`~/Applications/Ligature.app\` on macOS, and runs it as a user service that
starts at login. \`./uninstall.sh\` removes both and leaves your identity
in place. Then \`lpctl init --name "Your Name"\` on your first device, or \`lpctl pair\` with
an offer from a device that already has your keys; \`lpctl status\` shows the daemon.
The daemon listens on UDP port 47471: allow it through the firewall so your other
devices can dial in. Documentation and source: https://github.com/ligature-protocol/ligature-protocol
EOF

tar -C "$stage" -czf "$out_dir/$name.tar.gz" "$name"
echo "$out_dir/$name.tar.gz"
