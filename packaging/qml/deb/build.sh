#!/bin/sh
# Builds the .deb of Daevalog DPS Meter 2.0 (src-qml/) for Debian 13 and newer
# from the repository this file sits in, and writes it to the folder given
# (default: the current one). Needs g++, git, dpkg-dev, qt6-base-dev,
# qt6-declarative-dev and Rust 1.90 or newer: Debian 13's rustc is 1.85, so
# take Rust from rustup. The aes crate needs 1.89, and from 1.90 Rust links
# with its own lld; GNU ld fails here (see
# packaging/qml/rpm/daevalog-dps-meter.spec).
#
# dpkg-shlibdeps writes the library half of Depends, with Debian's own
# versions. The QML modules and Qt's Wayland plugin are loaded at run time,
# so control lists them by hand. The QML compiled in by qmlcachegen uses
# QtQml's private API, so Depends asks for at least the Qt minor version the
# meter was built with.
#
# DAEVALOG_REBUILD=<n> in the environment (optional): rebuild <n> of the same
# commit, for example after a Qt update. The version is then
# <version>+r<commits>+b<n>; none or 0 gives <version>+r<commits>.
set -eu

root=$(cd "$(dirname "$0")/../../.." && pwd)
out=$(cd "${1:-.}" && pwd)
version=$(grep -m1 '^version' "$root/src-qml/Cargo.toml" | cut -d'"' -f2)
rev=$(git -C "$root" rev-list --count HEAD)
debversion=${version}+r${rev}
case ${DAEVALOG_REBUILD:-0} in
  *[!0-9]*) echo "build.sh: DAEVALOG_REBUILD is not a number: $DAEVALOG_REBUILD" >&2; exit 2 ;;
  0) ;;
  *) debversion=${debversion}+b${DAEVALOG_REBUILD} ;;
esac

cargo build --release --locked --manifest-path "$root/src-qml/Cargo.toml"
# The capture helper, which holds the capture permission so the meter holds
# none. postinst grants it.
cargo build --release --locked --manifest-path "$root/src-qml/Cargo.toml" -p daevalog-capture

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
pkg="$work/pkg"
install -Dm755 "$root/src-qml/target/release/daevalog-qml" "$pkg/usr/bin/daevalog-dps-meter"
strip --strip-unneeded "$pkg/usr/bin/daevalog-dps-meter"
# The meter starts it by this name from its own folder.
install -Dm755 "$root/src-qml/target/release/daevalog-capture" "$pkg/usr/bin/daevalog-capture"
strip --strip-unneeded "$pkg/usr/bin/daevalog-capture"
# The game data, where the program looks for it (../share/daevalog-dps-meter/data).
install -d "$pkg/usr/share/daevalog-dps-meter"
cp -r "$root/src/data" "$pkg/usr/share/daevalog-dps-meter/data"
# Named by the app id, so portals and taskbars find the meter.
install -Dm644 "$root/packaging/qml/daevalog-dps-meter.desktop" "$pkg/usr/share/applications/daevalog-dps-meter.desktop"
install -Dm644 "$root/src-tauri/icons/128x128.png" "$pkg/usr/share/icons/hicolor/128x128/apps/daevalog-dps-meter.png"
install -d "$pkg/usr/share/doc/daevalog-dps-meter"
# The program is AGPL-3.0-only; the bundled Pretendard font is OFL.
cat "$root/LICENSE" "$root/public/vendor/fonts/pretendard/LICENSE" \
  > "$pkg/usr/share/doc/daevalog-dps-meter/copyright"

# dpkg-shlibdeps reads a source package's control file.
mkdir -p "$work/debian"
printf 'Source: daevalog-dps-meter\n\nPackage: daevalog-dps-meter\nArchitecture: any\n' > "$work/debian/control"
shlibs=$(cd "$work" && dpkg-shlibdeps -O "$pkg/usr/bin/daevalog-dps-meter" "$pkg/usr/bin/daevalog-capture" | sed -n 's/^shlibs:Depends=//p')
size=$(du -sk "$pkg/usr" | cut -f1)
qt=$(dpkg-query -W -f='${Version}' qt6-declarative-dev | sed -E 's/^[0-9]+://; s/^([0-9]+\.[0-9]+).*/\1/')

install -d "$pkg/DEBIAN"
# Ubuntu 24.04 has no Qt 6 layer shell (its layer-shell-qt is the Qt 5 one),
# and apt upgrade keeps a package back over a recommendation it cannot meet,
# so the line goes where the system lacks the package.
drop=""
if ! apt-cache show qml6-module-org-kde-layershell > /dev/null 2>&1; then
  drop='/^Recommends:/d'
fi
sed -e "s#@VERSION@#${debversion}#" -e "s#@SHLIBS@#${shlibs}#" -e "s#@SIZE@#${size}#" -e "s#@QT@#${qt}#" -e "$drop" \
  "$root/packaging/qml/deb/control" > "$pkg/DEBIAN/control"
install -m755 "$root/packaging/qml/deb/postinst" "$pkg/DEBIAN/postinst"
dpkg-deb --root-owner-group -b "$pkg" "$out/daevalog-dps-meter_${debversion}_amd64.deb"
