# Daevalog DPS Meter on Linux (Proton)

Daevalog runs natively on Linux while AION 2 runs under Proton. This guide is for Daevalog 2.0, the meter drawn with Qt Quick. Daevalog builds come from this repository: the package recipes for Arch, Debian and Fedora are in `packaging/qml/`, and any other system builds from source. If something does not work, your logs help: see [Logs and reporting a problem](#logs-and-reporting-a-problem).

Most people install the meter from the package repository at [packages.seralth.com](https://packages.seralth.com): Arch, Debian 13, LMDE 7, Linux Mint 22, Pop!_OS 24.04, Fedora, Bazzite and openSUSE Tumbleweed. Updates then come with the rest of the system. See [Installing the meter](#installing-the-meter).

## Requirements

- A 64-bit (x86_64) system.
- Qt 6: Qt Base (with Widgets, for the tray), Qt Declarative (the QML modules QtQuick, QtQuick.Layouts and Qt.labs.platform), Qt SVG (the skill icons) and Qt's Wayland plugin.
- libpcap, which the capture helper loads when it starts, and `setcap` (libcap), which gives the helper its capture permission.
- For the overlay above a fullscreen game on KDE Plasma, Hyprland and Sway: layer-shell-qt for Qt 6. Without it the meter is a normal window.

The oldest Qt the meter is checked on is Qt 6.4.2, the Qt of Linux Mint 22 and Pop!_OS 24.04. The meter works there, with these differences:

- On Wayland, Qt before 6.6 does not pass a menu's placement to the desktop, so a menu with no room below its control can open over the control.
- These systems ship layer-shell-qt for Qt 5 only, so `--layer-shell` and **Overlay: Wayland layer** give a normal window.
- Number columns can sit a few pixels apart: fixed-width digits need Qt 6.6.

The Arch, Debian 13 and Fedora packages build against the Qt of their system. Building needs Rust 1.89 or newer (the aes crate needs it), a C++ compiler and the lld linker: GNU ld fails to link the meter. Rust 1.90 and newer link with the lld that comes with Rust; with Rust 1.89, install the system's `lld` package.

## Contents

- [Installing the meter](#installing-the-meter): [from the package repository](#from-the-package-repository), [Steam Deck](#steam-deck-steamos), [building the package yourself](#building-the-package-yourself), [other distributions](#other-distributions)
- [Updating](#updating), and [which version you have](#which-version-you-have)
- [Starting, options and removing](#starting-options-and-removing)
- [Linux support by feature](#linux-support-by-feature)
- [Display backend](#display-backend) and [GNOME](#gnome)
- [Building from source](#building-from-source), for systems with no package
- [Logs and reporting a problem](#logs-and-reporting-a-problem)
- [Tiling window managers](#tiling-window-managers) (Hyprland, Sway, i3)
- [Fixing common problems](#fixing-common-problems)

## Installing the meter

Daevalog reads the game's network traffic, which on Linux needs the permission `cap_net_raw`. Only the small capture helper, `daevalog-capture`, receives that permission; the meter runs without it. The Arch and Debian packages run `setcap` on the helper when they install. The Fedora and openSUSE packages carry the permission inside the package, which is why the Fedora package also works on Bazzite, whose `rpm-ostree` runs install scripts with most permissions dropped. A build from source needs one `setcap` command (step 3 of [Building from source](#building-from-source)).

Every package installs the meter as `/usr/bin/daevalog-dps-meter`, the capture helper beside it as `/usr/bin/daevalog-capture`, and the game data under `/usr/share/daevalog-dps-meter`.

### From the package repository

Most systems install Daevalog from [packages.seralth.com](https://packages.seralth.com). Each system has its own repository with a build made for it, signed with the packages key. Add the repository once; the meter then updates with your normal system updates.

| System | Steps |
| --- | --- |
| Arch, CachyOS, Manjaro, EndeavourOS | [packages.seralth.com/#arch](https://packages.seralth.com/#arch) |
| Debian 13, LMDE 7 | [packages.seralth.com/#debian](https://packages.seralth.com/#debian) |
| Linux Mint 22, Pop!_OS 24.04 | [packages.seralth.com/#mint](https://packages.seralth.com/#mint) |
| Fedora | [packages.seralth.com/#fedora](https://packages.seralth.com/#fedora) |
| Bazzite and other image-based Fedoras | [packages.seralth.com/#bazzite](https://packages.seralth.com/#bazzite) |
| openSUSE Tumbleweed | [packages.seralth.com/#opensuse](https://packages.seralth.com/#opensuse) |

The overlay above a fullscreen game on KDE Plasma, Hyprland and Sway also needs layer-shell-qt. apt and dnf install it with the meter where the system has it; on Arch, install it with `sudo pacman -S --needed layer-shell-qt`.

### Steam Deck (SteamOS)

Each SteamOS update replaces the read-only system, and anything installed into it directly is lost. Daevalog therefore goes into a **distrobox**: a container with its own Arch Linux inside, which installs from the Arch repository. SteamOS 3.5 and later include distrobox. Create the box with `--root`, because a rootless container cannot read the game's network traffic.

Use **Desktop Mode** for the meter: Game Mode keeps every other program off the screen while you play.

1. Open the **Power** menu with the **Steam** button and choose **Switch to Desktop**.
2. Open **Konsole**. sudo needs a password for the `deck` user. If that user has none yet, run `passwd` in Konsole and set one.
3. Create the box, an Arch Linux container with root rights, and enter it. The first entry sets the box up, which takes a few minutes, and asks you to choose a password for your user in the box; sudo in the box asks for that password.

    ```shell
    distrobox create --root --name daevalog --image archlinux:latest
    distrobox enter --root daevalog
    ```

4. In the box, add the signing key and the repository, then install the meter:

    ```shell
    curl -fsSLO https://packages.seralth.com/daevalog.asc
    sudo pacman-key --add daevalog.asc
    sudo pacman-key --lsign-key 33F1E9F759E2887D26372C35502429F6C085461C
    printf '\n[daevalog]\nServer = https://packages.seralth.com/arch/$arch\n' | sudo tee -a /etc/pacman.conf
    sudo pacman -Syu daevalog-dps-meter layer-shell-qt
    exit
    ```

5. Start AION 2 from Steam, still in Desktop Mode, in borderless or windowed mode. Then start the meter from Konsole:

    ```shell
    distrobox enter --root daevalog -- daevalog-dps-meter
    ```

SteamOS updates do not touch the box, so the meter stays installed through them. Nobody has yet confirmed these steps on a real Steam Deck; please tell us how they work for you.

### Building the package yourself

Each package recipe is in `packaging/qml/`. Build a package on the system it is for. Keep the `Daevalog` folder, because updates are built from it.

#### Arch, CachyOS, Manjaro, EndeavourOS

`packaging/qml/arch/PKGBUILD` is the Arch recipe. `makepkg` builds the package `daevalog-dps-meter` from the clone the recipe sits in.

```shell
sudo pacman -S --needed git base-devel
git clone https://github.com/Seralth/Daevalog.git
cd Daevalog/packaging/qml/arch
makepkg -si
```

`-s` lets makepkg install what the recipe needs with pacman (Rust, Qt 6, libpcap). `-i` installs the package once it is built.

#### Debian 13, LMDE 7, Linux Mint 22, Pop!_OS 24.04

`packaging/qml/deb/build.sh` builds the .deb. A .deb built on Debian 13 does not install on Linux Mint 22 or Pop!_OS 24.04, whose Qt and C library are older. The script needs Rust 1.90 or newer, which links with its own lld (see the note at the top of `build.sh`). These systems ship an older Rust, so install Rust as [rustup.rs](https://rustup.rs) shows, then open a new terminal.

```shell
sudo apt install g++ git curl dpkg-dev qt6-base-dev qt6-declarative-dev
git clone https://github.com/Seralth/Daevalog.git
cd Daevalog
sh packaging/qml/deb/build.sh
sudo apt install ./daevalog-dps-meter_*_amd64.deb
```

`build.sh` writes the .deb into the current folder.

#### Fedora

`packaging/qml/rpm/daevalog-dps-meter.spec` builds the .rpm from a clone.

```shell
sudo dnf install git rpm-build cargo rust gcc-c++ lld qt6-qtbase-devel qt6-qtdeclarative-devel
git clone https://github.com/Seralth/Daevalog.git
cd Daevalog
rpmbuild -bb packaging/qml/rpm/daevalog-dps-meter.spec --define "checkout $PWD" \
  --define "pkgversion $(grep -m1 '^version' src-qml/Cargo.toml | cut -d'"' -f2)" \
  --define "pkgrelease $(git rev-list --count HEAD)"
sudo dnf install ~/rpmbuild/RPMS/x86_64/daevalog-dps-meter-*.rpm
```

#### openSUSE Tumbleweed

The same .rpm recipe builds there, with openSUSE's own package names. An .rpm built on Fedora does not install on Tumbleweed. The recipe asks for the system's `cargo` and `rust` packages, so `rpmbuild` refuses to start with only a Rust from rustup:

```shell
sudo zypper install git rpm-build cargo rust gcc-c++ lld qt6-base-devel qt6-declarative-devel
git clone https://github.com/Seralth/Daevalog.git
cd Daevalog
rpmbuild -bb packaging/qml/rpm/daevalog-dps-meter.spec --define "_topdir $PWD/rpmbuild" --define "checkout $PWD" \
  --define "pkgversion $(grep -m1 '^version' src-qml/Cargo.toml | cut -d'"' -f2)" \
  --define "pkgrelease $(git rev-list --count HEAD)"
sudo zypper install --allow-unsigned-rpm ./rpmbuild/RPMS/x86_64/daevalog-dps-meter-*.rpm
```

### Other distributions

NixOS and the rest: follow [Building from source](#building-from-source).

## Updating

Daevalog never updates itself and never checks for updates. Installed from the package repository, it updates with your normal system update. A package you built yourself is updated by building the newest version from the `Daevalog` folder:

| Your install | Commands |
| --- | --- |
| From packages.seralth.com | Your normal system update, for example `sudo pacman -Syu`, `sudo apt update && sudo apt upgrade`, `sudo dnf upgrade` or `sudo zypper dup` |
| Steam Deck (distrobox) | `distrobox enter --root daevalog -- sudo pacman -Syu` |
| Built yourself: Arch, CachyOS, Manjaro, EndeavourOS | `cd Daevalog && git pull && cd packaging/qml/arch && makepkg -si` |
| Built yourself: Debian 13, LMDE 7, Linux Mint 22, Pop!_OS 24.04 | Delete the older .deb files in the `Daevalog` folder, then `cd Daevalog && git pull && sh packaging/qml/deb/build.sh && sudo apt install ./daevalog-dps-meter_*_amd64.deb` |
| Built yourself: Fedora or openSUSE | `git pull` in the `Daevalog` folder, then the `rpmbuild` and install commands of [Fedora](#fedora) or [openSUSE Tumbleweed](#opensuse-tumbleweed) again |
| Built from source | `git pull`, then the two `cargo build` commands and the `setcap` command of [Building from source](#building-from-source) again |

After a Qt minor update (for example Qt 6.11 to 6.12), a meter you built yourself needs a rebuild too, and a meter from the repository needs a new package built against that Qt. The QML compiled into the meter uses Qt's private API, and that API changes between minor versions.

An update leaves your settings and saved fights as they are.

### Which version you have

Settings shows the version below its list of pages, for example "Daevalog 2.0 · r1". The number after "r" is the revision: the count of commits in this repository. A higher number is a newer build.

The package managers show the same revision; the Arch version also carries the commit:

| System | Command | Output |
| --- | --- | --- |
| Arch | `pacman -Q daevalog-dps-meter` | `daevalog-dps-meter 1:2.0.0.r1.g0123abc-1` |
| Debian | `dpkg-query -W daevalog-dps-meter` | `daevalog-dps-meter 2.0.0+r1` |
| Fedora | `rpm -q daevalog-dps-meter` | `daevalog-dps-meter-2.0.0-1.x86_64` |

## Starting, options and removing

The application menu lists the meter as **Daevalog DPS Meter**. In a distrobox, start it from a terminal as step 5 of [Steam Deck](#steam-deck-steamos) shows. To keep the meter's output in a file, which helps when you report a problem, start it from a terminal like this:

```shell
daevalog-dps-meter 2>&1 | tee ~/meter-console.log
```

Only one meter runs at a time: a second start brings the running meter forward and quits. With a system tray, closing the meter's window hides the meter to the tray; without one, closing the window quits the meter. **Quit** in the tray menu quits it.

`daevalog-dps-meter --help` lists every command-line option. An option changes a setting for that run only. Some of them:

| Option | Effect |
| --- | --- |
| `--layer-shell` | Shows the meter as a Wayland layer (see [Display backend](#display-backend)). |
| `--locked` | Starts locked, so clicks pass through the meter to the game. |
| `--mode MODE` | Starts in `boss`, `target`, `all`, `train` or `enc` mode instead of the **Default mode** setting. |
| `--language LANG` | `en`, `de`, `es`, `fr`, `ja`, `ko`, `pt` or `ru`. |
| `--capture-helper FILE` | Starts this capture helper instead of the one beside the meter. The file needs `cap_net_raw`. |

To remove the meter:

| Your install | Command |
| --- | --- |
| Arch, CachyOS, Manjaro, EndeavourOS | `sudo pacman -R daevalog-dps-meter` |
| Debian, LMDE, Linux Mint, Pop!_OS | `sudo apt remove daevalog-dps-meter` |
| Fedora | `sudo dnf remove daevalog-dps-meter` |
| openSUSE | `sudo zypper remove daevalog-dps-meter` |
| Bazzite and other image-based Fedoras | `rpm-ostree uninstall daevalog-dps-meter`, then restart |
| Steam Deck (distrobox) | `distrobox rm --root daevalog`, which removes the whole box |
| Built from source | Delete the `Daevalog` folder |

Removing the meter leaves your settings and saved fights in `~/.local/share/daevalog-dps-meter`. Delete that folder to remove them too.

## Linux support by feature

| Feature | Status on Linux |
| --- | --- |
| The meter with its DPS, Heal and Tank tabs, Details, Battle History | Supported |
| Ping | Supported. The meter times the game's own ping on the game's connection, so the value includes a VPN, if you use one, and the time the game server takes to answer. A busy server, such as a crowded world boss, can make the value jump while the network stays steady. |
| Finding the game | The meter looks for a running `AION2.exe` process, which is how the game runs under Proton. |
| a2tools.app sign-in | Supported. The sign-in is kept in the desktop keyring (KWallet, GNOME Keyring, or another Secret Service), which may ask to create or unlock a keyring the first time. |
| Updates | With your system updates when installed from the package repository, by rebuilding otherwise (see [Updating](#updating)). The meter never updates itself. |
| Class and skill icons | Supported. The icons are built into the meter. |
| Tray icon | Supported where the desktop has a system tray. GNOME needs the AppIndicator extension. |
| Global hotkey | The lock key works through the desktop's GlobalShortcuts portal, on desktops that have it (KDE Plasma, GNOME 48 and later, Hyprland). The desktop asks you to approve the key the first time. Without the portal, the tray and the meter's ⋯ menu lock the meter. |
| Click-through lock | Supported on X11, XWayland and native Wayland. A locked meter keeps its ⋯ button clickable, so you can unlock it. |
| Screenshots | Supported: to the clipboard, and to a folder when that is turned on (`~/Pictures/Daevalog DPS Meter` by default). Wayland lets no program copy the screen, so the meter draws its own windows onto its own background. |
| Auto-hide when the game loses focus | Not on Linux. The meter cannot tell which window has the focus there, so it stays visible. |

## Display backend

At its start the meter picks how it draws to fit the desktop. **Settings > Diagnostics > Report a problem** shows the result in its report info, for example "Qt 6.12.0, wayland, layer shell": the Qt version, Qt's platform, and how the meter shows.

| Desktop | Backend |
| --- | --- |
| X11 sessions (Xfce, Cinnamon) | X11, a window that stays on top |
| KDE Plasma (X11) | X11, as a window KWin does not manage, so it stays above a fullscreen game too. It has no taskbar entry |
| KDE Plasma, Hyprland, Sway (Wayland) | Native Wayland, with the overlay as a layer above every window, a fullscreen game too |
| COSMIC and other Wayland desktops with a layer shell | Native Wayland, a normal window |
| GNOME and other Wayland desktops without a layer shell | XWayland, a window that stays on top (see [GNOME](#gnome)) |

The layer needs layer-shell-qt for Qt 6 (see [Requirements](#requirements)). **Settings > Meter > Overlay: Wayland layer** turns it on or off; it takes effect at the next start, and the switch shows only where the meter runs on Wayland. With the layer off, or layer-shell-qt missing, the meter is a normal window: below a fullscreen game, and tiled on Hyprland and Sway (see [Tiling window managers](#tiling-window-managers)). On COSMIC and other Wayland desktops with a layer shell, the switch or `--layer-shell` turns the layer on.

A platform you choose yourself always wins: set `QT_QPA_PLATFORM` (`xcb` for X11 or XWayland, `wayland` for native Wayland) or pass `-platform`:

```shell
QT_QPA_PLATFORM=xcb daevalog-dps-meter
```

## GNOME

GNOME's Wayland session offers no layer shell, and keeps no normal Wayland window above a fullscreen one. So on GNOME the meter runs through XWayland as a window that stays on top, with no GNOME Shell extension needed. Only the meter uses XWayland; the desktop session stays on Wayland. The meter keeps out of the taskbar there, so showing it posts no "is ready" notice.

The tray icon needs the AppIndicator extension. Without it, use the meter's ⋯ menu.

With `QT_QPA_PLATFORM=wayland` the meter runs as a native Wayland window instead, which GNOME does not keep above other windows.

## Building from source

### Step 1: build tools and libraries

The build needs Rust 1.90 or newer (or Rust 1.89 and the system's `lld` package, see [Requirements](#requirements)), a C++ compiler, git, and Qt 6 with its development files. To run, the meter also needs libpcap, Qt SVG, the QML modules, Qt's Wayland plugin and `setcap`.

On any distribution, install Rust as [rustup.rs](https://rustup.rs) shows, then open a new terminal. Then install Qt and the other libraries for your distribution.

On Debian 13, LMDE 7, Linux Mint 22 and Pop!_OS 24.04:

```shell
sudo apt install g++ git qt6-base-dev qt6-declarative-dev qt6-wayland qml6-module-qtquick qml6-module-qtquick-layouts qml6-module-qtqml-workerscript qml6-module-qt-labs-platform libpcap0.8t64 libcap2-bin
```

Then the SVG plugin: `sudo apt install qt6-svg-plugins` on Debian 13, `sudo apt install libqt6svg6` on Linux Mint 22 and Pop!_OS 24.04. On Debian 13 also install `layer-shell-qt` and `qml6-module-org-kde-layershell`, for the overlay above a fullscreen game.

On Fedora:

```shell
sudo dnf install gcc-c++ lld git qt6-qtbase-devel qt6-qtdeclarative-devel qt6-qtsvg libpcap libcap
```

On Arch, Manjaro and EndeavourOS, to build without the package:

```shell
sudo pacman -S --needed base-devel git qt6-base qt6-declarative qt6-svg libpcap libcap layer-shell-qt
```

On openSUSE Tumbleweed:

```shell
sudo zypper install gcc-c++ lld git qt6-base-devel qt6-declarative-devel libQt6Svg6 qt6-wayland libpcap1 libcap-progs
```

NixOS has no recipe here.

**Steam Deck:** follow [Steam Deck](#steam-deck-steamos) instead.

### Step 2: get and build the code

From your home folder, or wherever you keep projects:

```shell
git clone https://github.com/Seralth/Daevalog.git
cd Daevalog/src-qml
cargo build --release --locked
cargo build --release --locked -p daevalog-capture
```

The build gives two files, which must stay together in one folder:

```shell
src-qml/target/release/daevalog-qml
src-qml/target/release/daevalog-capture
```

`daevalog-qml` is the meter; the packages install it as `daevalog-dps-meter`. `daevalog-capture` is the capture helper. The meter starts the helper from its own folder, never from your `PATH`; the helper reads the network traffic and passes the game's packets to the meter. Started from here, the meter reads the game data from the `src/data` folder of the clone.

### Step 3: give the helper its permission

Linux gives the right to capture packets only to a program that has the capability for it. Give it to the capture helper alone, from the `Daevalog` folder:

```shell
sudo setcap cap_net_raw=ep src-qml/target/release/daevalog-capture
```

- The meter itself needs no capability.
- Each build writes a new file, and a new file has no capability. Run the `setcap` command again after every build.
- Never start the meter itself with `sudo`: it would run as root and keep its settings in root's home folder.
- Some drives, such as NTFS or exFAT drives shared with Windows, do not keep capabilities. If `setcap` fails, build the meter on a drive with a Linux file system.

Start the meter from a terminal, which also saves its output:

```shell
src-qml/target/release/daevalog-qml 2>&1 | tee ~/meter-console.log
```

## Logs and reporting a problem

When the meter does not work for you, a short session with logging on usually shows why:

1. Start AION 2 from Steam.
2. Open a terminal and start the meter there, as [Starting, options and removing](#starting-options-and-removing) shows (or as step 3 of [Building from source](#building-from-source) shows for a build from source).
3. In the meter, open **Settings**. Under **Diagnostics**, turn on **Enable debug logging** and **Enable packet logging**.
4. Log in, enter the world with a character, and wait about a minute.
5. Fight a training dummy, or a few monsters, for a minute or two.
6. During the fight, write down:
    - if any damage of yours appears in the meter;
    - the meter's ping (in its footer) and the game's own ping;
    - if the meter keeps above the game window, and the game's display mode (fullscreen, borderless or windowed).
7. If you can, change zone or return to character selection once, then fight a little again.
8. Turn off **Enable packet logging**.

The logs are in the meter's data folder. **Settings > Diagnostics > Open log folder** opens it:

| File | Folder |
| --- | --- |
| `debug.log` | `~/.local/share/daevalog-dps-meter/` |
| `packets_YYYYMMDD_HHMMSS.txt` | `~/.local/share/daevalog-dps-meter/` |
| `meter-console.log` | your home folder, when you started the meter as shown above |

Then use **Settings > Diagnostics > Report a problem**. Pick the kind of problem, then **Open the form**: it opens the GitHub issue form with the report info filled in (the version, your system and desktop, the display backend, and whether packet capture works). The meter shows that text before anything opens and sends nothing itself.

Packet logs and `debug.log` hold the names of the players you met, and GitHub issues are public. Attach the prepared copies, never the files themselves:

- **Prepare log** (under **Wrong numbers**) saves a compressed copy of a packet log in the log folder, `report_` and the log's name with `.gz` added, with every character name replaced.
- **Prepare debug log** (under **Crash or won't start**) saves `report_debug.log`, with the character names, your home folder and your user name taken out.

Logs from a session that failed help as much as logs from one that worked. Add these details to the report:

- your distribution and its version (for example Linux Mint 22, Fedora 44 or CachyOS);
- your Proton version, and the game's display mode;
- if damage appeared in the meter, and about when it began;
- both pings: the meter's and the game's;
- if the meter kept above the game window;
- anything else that seemed wrong.

## Tiling window managers

On Hyprland and Sway the meter is a layer by default. A layer sits above every window and the window manager does not tile it, so the meter needs no window rule there.

A window rule is needed in three cases: on i3, on Hyprland or Sway with **Overlay: Wayland layer** off, and on Hyprland or Sway without layer-shell-qt. In those cases the meter is a normal window, and a tiling window manager tiles every new window, which squeezes the meter in beside the game. The rule must make the meter's window float, keep it on every workspace, and turn off blur and borders. On Wayland the meter's app id is `daevalog-dps-meter`.

- **Hyprland:** add a window rule for the class `daevalog-dps-meter` that sets float and pin, turns off blur, and sets the border size to 0. Hyprland's window rule syntax differs between its versions; see the Hyprland wiki for yours.
- **Sway:** `for_window [app_id="daevalog-dps-meter"] floating enable, sticky enable`.
- **i3:** the same `floating enable, sticky enable`, matched on the meter's X11 class. To see the class, run `xprop WM_CLASS` and click the meter.

## Fixing common problems

| Problem | Fix |
| --- | --- |
| The sign-in ends with "its token could not be stored securely" | The meter stores a sign-in only in a desktop keyring, and the end of that message gives the cause. "No desktop keyring is running": start one, either GNOME Keyring, KWallet, or KeePassXC with its Secret Service option on. Hyprland, Sway and i3 start no keyring themselves; add one to the commands your session starts, for example `gnome-keyring-daemon --start --components=secrets`. "The keyring stayed locked": unlock the keyring when it asks. "The keyring has no collection": let the keyring create one when it asks. Then press **Sign in** again. |
| The build cannot find Qt, or a Qt module | Run the install command for your distribution again. |
| After a Qt update the meter does not start, or its windows stay empty | The meter was built for another Qt minor version. From the repository: update once a package built against the new Qt is out, and report the problem if none is out yet. Built yourself: rebuild it as [Updating](#updating) says. |
| `debug.log` or the terminal says to install libpcap | Install libpcap: `libpcap0.8t64` on Debian 13, LMDE 7, Linux Mint 22 and Pop!_OS 24.04, `libpcap1` on openSUSE, `libpcap` elsewhere. Then start the meter again. |
| The meter says "Packet capture is not available", or `debug.log` has no `Capture active` line | The capture helper `daevalog-capture` is missing beside the meter, or it lacks its permission. For a package from the repository, install it again (`sudo pacman -S daevalog-dps-meter`, `sudo apt reinstall daevalog-dps-meter`, `sudo dnf reinstall daevalog-dps-meter` or `sudo zypper install -f daevalog-dps-meter`). For a package you built yourself, build and install it again. For a build from source, build the helper and run the `setcap` command again, since every build needs it. |
| `debug.log` says `Game not found (looking for the AION2 process)` while the game runs | The meter does not see the game's process. Add the output of `ps aux \| grep -i aion` to your report. |
| `debug.log` says `No game connection locked` with `0 marked` on every device while you fight | The meter sees traffic, but not the game's. In your report, say whether you use a VPN or a ping reducer. Pick the game's network device under **Settings > Capture**. |
| The game covers the meter | Set the game to borderless or windowed mode. On KDE Plasma, Hyprland and Sway, the meter stays above a fullscreen game only as a layer: install layer-shell-qt and turn on **Overlay: Wayland layer**. Other Wayland desktops keep a fullscreen game above every program's window. |
| A tiling window manager puts the meter beside the game | See [Tiling window managers](#tiling-window-managers). |

For any other problem, report it with the logs you have.
