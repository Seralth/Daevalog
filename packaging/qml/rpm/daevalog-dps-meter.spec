# The .rpm of Daevalog DPS Meter 2.0 (src-qml/, the Qt Quick meter) for
# Fedora. Like packaging/qml/arch/PKGBUILD it builds from a checkout, not a
# tarball:
#
#   rpmbuild -bb packaging/qml/rpm/daevalog-dps-meter.spec --define "checkout $PWD" \
#     --define "pkgversion <src-qml version>" --define "pkgrelease <commits>"
#
# DAEVALOG_REBUILD=<n> in the environment (optional): rebuild <n> of the same
# commit, for example after a Qt update. The release is then <commits>.<n>;
# none or 0 leaves it as it is.
#
# rpmbuild finds the Qt libraries the binary links against by itself. The
# QML modules are loaded at run time, so they are required here. The QML
# compiled in by qmlcachegen uses QtQml's private API, so the package asks
# for at least the Qt minor version it was built with (Fedora's rpmbuild also
# finds libQt6Qml.so.6(Qt_6.11_PRIVATE_API)): rebuild for each Qt minor
# version.

%global debug_package %{nil}

Name:           daevalog-dps-meter
# Set with --define (see above); the release is 1 when none is given.
Version:        %{?pkgversion}
%global rebuild %(echo "${DAEVALOG_REBUILD:-0}")
%if "%{rebuild}" != "0"
%if "%(echo '%{rebuild}' | tr -d 0-9)" != ""
%{error:DAEVALOG_REBUILD is not a number: %{rebuild}}
%endif
%global rebuildpart .%{rebuild}
%endif
Release:        %{?pkgrelease}%{!?pkgrelease:1}%{?rebuildpart}
Summary:        Daevalog DPS Meter - AION 2 real-time DPS overlay
# The program is AGPL-3.0-only. The bundled Pretendard font is OFL.
License:        AGPL-3.0-only AND OFL-1.1
URL:            https://github.com/Seralth/Daevalog
ExclusiveArch:  %{x86_64}
# The 2.0 test packages were daevalog-qml and daevalog-qml-dev. The A2Tools
# package of the same program (upstream's releases are 2.0.x) is replaced as
# 1.x replaced it. The 1.x meter is daevalog-dps-meter-legacy.
Obsoletes:      daevalog-qml < 2
Obsoletes:      daevalog-qml-dev < 2
Provides:       a2-tools-dps-meter = %{version}-%{release}
Obsoletes:      a2-tools-dps-meter < 3

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc-c++
# cxx-qt links with lld when it finds it. With GNU ld (bfd) the link fails
# on QApplication and QQuickWindow: Qt Quick and Widgets come before the code
# that uses them on the command line.
BuildRequires:  lld
%if 0%{?suse_version}
BuildRequires:  qt6-base-devel
BuildRequires:  qt6-declarative-devel
%else
BuildRequires:  qt6-qtbase-devel
BuildRequires:  qt6-qtdeclarative-devel
%endif

# The capture helper loads libpcap at run time, so rpmbuild cannot find it.
Requires:       libpcap.so.1()(64bit)
# The Qt minor version built with, from the QtQml development package.
%if 0%{?suse_version}
%global qt %(rpm -q --qf '%%{VERSION}' qt6-declarative-devel | cut -d. -f1,2)
%else
%global qt %(rpm -q --qf '%%{VERSION}' qt6-qtdeclarative-devel | cut -d. -f1,2)
%endif
%if "%(echo '%{qt}' | tr -d 0-9.)" != "" || "%{qt}" == ""
%{error:no Qt version from the QtQml development package: %{qt}}
%endif
%if 0%{?suse_version}
# openSUSE names: the QML modules the meter imports, the SVG image plugin
# (skill icons), and the Wayland client, which openSUSE packs on its own.
Requires:       libQt6Qml6 >= %{qt}
Requires:       qt6qmlimport(QtQuick)
Requires:       qt6qmlimport(QtQuick.Layouts)
Requires:       qt6qmlimport(Qt.labs.platform)
Requires:       qt6qmlimport(QtQml.WorkerScript)
Requires:       libQt6Svg6
Requires:       qt6-wayland
Recommends:     qt6qmlimport(org.kde.layershell)
%else
# The QML modules the meter imports. qt6-qtbase-gui, which rpmbuild adds,
# has Widgets (the tray) and the Wayland client.
Requires:       qt6-qtdeclarative%{?_isa} >= %{qt}
Requires:       qt6qml(QtQuick)
Requires:       qt6qml(QtQuick.Layouts)
Requires:       qt6qml(Qt.labs.platform)
# The SVG image plugin, which draws the skill icons.
Requires:       qt6-qtsvg
# --layer-shell: the overlay above fullscreen games on KDE Plasma, Hyprland
# and Sway.
Recommends:     qt6qml(org.kde.layershell)
%endif

%description
Shows AION 2 damage per second live, in an overlay drawn by a native Linux
program while the game itself runs under Proton. The 1.x meter is the
package daevalog-dps-meter-legacy.

%build
cd "%{checkout}/src-qml"
cargo build --release --locked --target-dir "%{_builddir}/target"
# The capture helper, which holds the capture permission so the meter holds
# none.
cargo build --release --locked --target-dir "%{_builddir}/target" -p daevalog-capture

%install
install -Dm755 "%{_builddir}/target/release/daevalog-qml" "%{buildroot}%{_bindir}/daevalog-dps-meter"
# No debuginfo package, so nothing else strips the symbols (as makepkg does).
%{__strip} --strip-unneeded "%{buildroot}%{_bindir}/daevalog-dps-meter"
# The meter starts it by this name from its own folder.
install -Dm755 "%{_builddir}/target/release/daevalog-capture" "%{buildroot}%{_bindir}/daevalog-capture"
%{__strip} --strip-unneeded "%{buildroot}%{_bindir}/daevalog-capture"
# The game data, where the program looks for it (../share/daevalog-dps-meter/data).
install -d "%{buildroot}%{_datadir}/daevalog-dps-meter"
cp -r "%{checkout}/src/data" "%{buildroot}%{_datadir}/daevalog-dps-meter/data"
# Named by the app id, so portals and taskbars find the meter.
install -Dm644 "%{checkout}/packaging/qml/daevalog-dps-meter.desktop" "%{buildroot}%{_datadir}/applications/daevalog-dps-meter.desktop"
install -Dm644 "%{checkout}/src-tauri/icons/128x128.png" "%{buildroot}%{_datadir}/icons/hicolor/128x128/apps/daevalog-dps-meter.png"
install -Dm644 "%{checkout}/LICENSE" "%{buildroot}%{_licensedir}/%{name}/LICENSE"
install -Dm644 "%{checkout}/public/vendor/fonts/pretendard/LICENSE" \
  "%{buildroot}%{_licensedir}/%{name}/LICENSE.Pretendard"

%files
%{_bindir}/daevalog-dps-meter
# In the package, not from a setcap in a package script: rpm-ostree runs
# those with most capabilities dropped.
%caps(cap_net_raw=ep) %{_bindir}/daevalog-capture
%{_datadir}/daevalog-dps-meter
%{_datadir}/applications/daevalog-dps-meter.desktop
%{_datadir}/icons/hicolor/128x128/apps/daevalog-dps-meter.png
%license %{_licensedir}/%{name}/LICENSE
%license %{_licensedir}/%{name}/LICENSE.Pretendard
