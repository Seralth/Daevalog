# A2Tools DPS Meter on Linux (Proton)

The meter runs natively on Linux while AION 2 runs under Proton. Install the package for your system below: it sets up everything, packet-capture permission included, and keeps itself up to date. Only on a distribution with no package do you need to build it yourself. Linux support is new, so your logs help: see [Sending us your logs](#sending-us-your-logs).

Every package needs a 64-bit (x86_64) system with WebKitGTK 4.1: Ubuntu 22.04, Debian 12, Fedora 39 or newer, any current Arch, Bazzite, or SteamOS through distrobox.

## Contents

[![Ubuntu](https://img.shields.io/badge/Ubuntu-E95420?logo=ubuntu&logoColor=white)](#ubuntu-debian-linux-mint-pop_os) [![Debian](https://img.shields.io/badge/Debian-A81D33?logo=debian&logoColor=white)](#ubuntu-debian-linux-mint-pop_os) [![Linux Mint](https://img.shields.io/badge/Linux_Mint-87CF3E?logo=linuxmint&logoColor=white)](#ubuntu-debian-linux-mint-pop_os) [![Pop!_OS](https://img.shields.io/badge/Pop%21__OS-48B9C7?logo=popos&logoColor=white)](#ubuntu-debian-linux-mint-pop_os) [![Fedora](https://img.shields.io/badge/Fedora-51A2DA?logo=fedora&logoColor=white)](#fedora) [![Bazzite](https://img.shields.io/badge/Bazzite-8A3FFC?logo=fedora&logoColor=white)](#bazzite-silverblue-kinoite-aurora-bluefin) [![Steam Deck](https://img.shields.io/badge/Steam_Deck-1A9FFF?logo=steamdeck&logoColor=white)](#steam-deck-steamos) [![openSUSE](https://img.shields.io/badge/openSUSE-73BA25?logo=opensuse&logoColor=white)](#opensuse) [![Arch](https://img.shields.io/badge/Arch-1793D1?logo=archlinux&logoColor=white)](#cachyos-arch-manjaro-endeavouros) [![CachyOS](https://img.shields.io/badge/CachyOS-08A88A?logo=cachyos&logoColor=white)](#cachyos-arch-manjaro-endeavouros) [![Manjaro](https://img.shields.io/badge/Manjaro-35BF5C?logo=manjaro&logoColor=white)](#cachyos-arch-manjaro-endeavouros) [![EndeavourOS](https://img.shields.io/badge/EndeavourOS-7F3FBF?logo=endeavouros&logoColor=white)](#cachyos-arch-manjaro-endeavouros)

- **[Install](#install):** [Ubuntu, Debian, Mint, Pop!_OS](#ubuntu-debian-linux-mint-pop_os) · [Fedora](#fedora) · [Bazzite and other image-based Fedoras](#bazzite-silverblue-kinoite-aurora-bluefin) · [Steam Deck (SteamOS)](#steam-deck-steamos) · [openSUSE](#opensuse) · [Arch, CachyOS, Manjaro, EndeavourOS](#cachyos-arch-manjaro-endeavouros)
- **[Update](#update):** [how each install updates](#how-each-install-updates) · [the update prompt](#the-update-prompt) · [by hand](#update-by-hand) · [which version do I have?](#which-version-do-i-have)
- **[Start and remove](#start-and-remove)**
- **[What works on Linux](#what-works-on-linux)**
- **[Build from source](#build-from-source-other-distributions)**, for distributions with no package
- **[Sending us your logs](#sending-us-your-logs)**
- **[Troubleshooting](#troubleshooting)**

## Install

Each package grants the packet-capture permission itself, so you never need `setcap`.

### Ubuntu, Debian, Linux Mint, Pop!_OS

```bash
curl -LO https://cdn.a2tools.app/linux/a2tools-dps-meter-latest_amd64.deb
```

```bash
sudo apt install ./a2tools-dps-meter-latest_amd64.deb
```

### Fedora

Add the A2Tools repository once, then install from it. Updates then arrive with your normal system updates.

```bash
sudo curl -Lo /etc/yum.repos.d/a2tools.repo https://cdn.a2tools.app/linux/a2tools.repo
```

```bash
sudo dnf install a2-tools-dps-meter
```

### Bazzite, Silverblue, Kinoite, Aurora, Bluefin

These image-based Fedoras keep the system read-only and add packages by layering them with `rpm-ostree`, which takes effect after a restart. Add the A2Tools repository, layer the meter, and restart:

```bash
sudo curl -Lo /etc/yum.repos.d/a2tools.repo https://cdn.a2tools.app/linux/a2tools.repo
```

```bash
rpm-ostree install a2-tools-dps-meter
```

```bash
systemctl reboot
```

Updates then come with your system updates: Bazzite installs them on its own, or run `rpm-ostree upgrade` and restart. On a Steam Deck running Bazzite, use **Desktop Mode**: in Game Mode, nothing can draw over the game.

### Steam Deck (SteamOS)

SteamOS replaces its read-only system with every update, so anything installed into it directly is wiped. Instead, the meter goes in a **distrobox**: a container with its own Arch Linux inside, which SteamOS 3.5 and later include. It must be created with `--root`: an ordinary (rootless) container cannot read the game's network traffic.

The overlay only works in **Desktop Mode**. In Game Mode, nothing can draw over the game.

1. Switch to Desktop Mode: press the **Steam** button, then **Power**, then **Switch to Desktop**.
2. Open **Konsole** from the application menu. If you have never set a password for the `deck` user, set one now (sudo needs it):

    ```bash
    passwd
    ```

3. Create the box (the first time takes a few minutes):

    ```bash
    distrobox create --root --name a2tools --image archlinux:latest
    ```

4. Download the meter and install it inside the box:

    ```bash
    curl -LO https://cdn.a2tools.app/linux/a2tools-dps-meter-latest-x86_64.pkg.tar.zst
    ```

    ```bash
    distrobox enter --root a2tools -- sudo pacman -Syu --noconfirm
    ```

    ```bash
    distrobox enter --root a2tools -- sudo pacman -U --noconfirm ~/a2tools-dps-meter-latest-x86_64.pkg.tar.zst
    ```

5. Start AION 2 from Steam, still in Desktop Mode, set to borderless or windowed. Then start the meter from Konsole:

    ```bash
    distrobox enter --root a2tools -- a2tools-dps-meter
    ```

SteamOS updates leave the box alone, so the meter survives them. This route is new and not yet confirmed on a real Steam Deck: please tell us how it goes.

### openSUSE

```bash
curl -LO https://cdn.a2tools.app/linux/a2tools-dps-meter-latest.x86_64.rpm
```

```bash
sudo zypper install --allow-unsigned-rpm ./a2tools-dps-meter-latest.x86_64.rpm
```

### CachyOS, Arch, Manjaro, EndeavourOS

pacman refuses unsigned packages straight from a URL, hence the download first:

```bash
curl -LO https://cdn.a2tools.app/linux/a2tools-dps-meter-latest-x86_64.pkg.tar.zst
```

```bash
sudo pacman -U a2tools-dps-meter-latest-x86_64.pkg.tar.zst
```

## Update

### How each install updates

| Installed on | Updates |
| --- | --- |
| Ubuntu, Debian, Mint, Pop!_OS | The meter offers each new version: [the update prompt](#the-update-prompt) |
| Fedora | With your system updates (`sudo dnf upgrade`); the meter also offers them |
| Bazzite and other image-based Fedoras | With your system updates; the meter does not prompt |
| Steam Deck (SteamOS) | [By hand](#update-by-hand) |
| openSUSE | The meter offers each new version |
| Arch, CachyOS, Manjaro, EndeavourOS | The meter offers each new version |

### The update prompt

When a new version is out, the meter asks "A new update is available! … Download and install now?" shortly after it starts:

1. Click **Yes**. The meter downloads the update and closes.
2. Your desktop asks for your password, the same prompt as for other system changes. Enter it.
3. The meter starts again by itself, on the new version.

If you cancel the password prompt, the meter restarts on the old version and asks again next time.

### Update by hand

Run the [Install](#install) commands for your system again: the address always serves the newest version, and installing it over the old one keeps your settings and fight history. On a Steam Deck, that is the `curl` line and the last `pacman -U` line of step 4.

Do this once if your version cannot update itself: on Arch, `2.0.30.r70.g0ac3fb6-1`, the first test package.

### Which version do I have?

Ubuntu, Debian, Mint, Pop!_OS:

```bash
dpkg -s a2-tools-dps-meter | grep Version
```

Fedora, Bazzite and other image-based Fedoras, openSUSE:

```bash
rpm -q a2-tools-dps-meter
```

Arch, CachyOS, Manjaro, EndeavourOS:

```bash
pacman -Q a2tools-dps-meter
```

Steam Deck (SteamOS):

```bash
distrobox enter --root a2tools -- pacman -Q a2tools-dps-meter
```

## Start and remove

The meter is in your application menu, as A2Tools DPS Meter; on a Steam Deck, start it from Konsole as in [step 5](#steam-deck-steamos). To start it from a terminal with its output saved, which helps if you send us logs:

```bash
a2tools-dps-meter 2>&1 | tee ~/meter-console.log
```

To remove it:

| Installed on | Command |
| --- | --- |
| Ubuntu, Debian, Mint, Pop!_OS | `sudo apt remove a2-tools-dps-meter` |
| Fedora | `sudo dnf remove a2-tools-dps-meter` |
| Bazzite and other image-based Fedoras | `rpm-ostree uninstall a2-tools-dps-meter`, then restart |
| Steam Deck (SteamOS) | `distrobox rm --root a2tools` (removes the whole box) |
| openSUSE | `sudo zypper remove a2-tools-dps-meter` |
| Arch, CachyOS, Manjaro, EndeavourOS | `sudo pacman -R a2tools-dps-meter` |

## What works on Linux

| Feature | On Linux |
| --- | --- |
| Damage meter, Details, History | Works |
| Ping | Works |
| Finding the game | Looks for the running AION2.exe process under Proton |
| A2 Tools account sign-in | Works: kept in KWallet or GNOME Keyring, which may ask to create or unlock a wallet the first time |
| Automatic updates | Works for the packages; builds from source update with `git pull` |
| Class icons | Works (missing in the 2.0.34 package and earlier; fixed in 2.0.35) |
| Global hotkeys | Not yet |
| Screenshots | Not yet |
| Auto-hide when the game loses focus | Not yet (the meter stays visible) |

## Build from source (other distributions)

### 1. Install the build tools

You need Rust, Node.js and the system libraries the app builds against, including libpcap for packet capture.

**Rust** (any distribution), then open a new terminal:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Node.js 18 or newer:** from your package manager, or from [nodejs.org](https://nodejs.org).

**System libraries**, for your distribution.

Debian, Ubuntu, Mint, Pop!_OS:

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libpcap-dev git
```

Fedora:

```bash
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file libappindicator-gtk3-devel librsvg2-devel libpcap libcap git
sudo dnf group install c-development
```

Arch, Manjaro, EndeavourOS (if you would rather build than use the package):

```bash
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module libappindicator-gtk3 librsvg libpcap git
```

**Steam Deck:** no need to build: follow the [Steam Deck instructions](#steam-deck-steamos).

### 2. Download and build the meter

The first build takes 10–15 minutes; later ones are much faster. Run these from your home folder or wherever you keep projects:

```bash
git clone https://github.com/taengu/A2Tools-DPS-Meter.git
cd A2Tools-DPS-Meter

# A fresh download lacks some data files the app reads; copy them into place
mkdir -p public/i18n public/data public/src/data
cp -r src/data/i18n/* public/i18n/
cp src/data/skill_icons.json src/data/dot_skill_ids.json public/data/
cp src/data/skill_icons.json src/data/dot_skill_ids.json public/src/data/

npm install
npx tauri build --no-bundle
```

When it finishes, the meter is this one file:

```bash
src-tauri/target/release/a2tools-dps-meter
```

To pick up fixes later: `git pull`, then run the `npx tauri build --no-bundle` line again, then redo step 3.

### 3. Allow the meter to capture packets

Reading the game's network traffic needs a capability Linux only gives on request. Grant it to the meter's file, from the `A2Tools-DPS-Meter` folder:

```bash
sudo setcap cap_net_raw,cap_net_admin=eip src-tauri/target/release/a2tools-dps-meter
```

- Redo this after every rebuild: a new build is a new file and loses the permission.
- Do not run the meter itself with `sudo`. It would run as root, keep its settings in root's home folder, and often fail to open its window.
- `setcap` does not work on some drives (for example NTFS or exFAT shared with Windows). If it fails, build the meter on a Linux-formatted drive.

Then start it from a terminal, so its output is saved too:

```bash
src-tauri/target/release/a2tools-dps-meter 2>&1 | tee ~/meter-console.log
```

## Sending us your logs

If something does not work, a short logged session tells us why:

1. Start AION 2 from Steam as you normally do.
2. Start the meter from a terminal, with the command for your install above.
3. In the meter, open **Settings**. Under **Diagnostics**, tick **Enable debug logging** and **Enable packet logging**.
4. Log in and enter the world with a character, then wait a minute.
5. Fight a training dummy or some monsters for one to two minutes.
6. While you play, note:
    - whether the meter shows your damage;
    - the ping in the meter's footer, next to the ping the game itself shows;
    - whether the meter stays visible on top of the game, and whether the game is fullscreen, borderless or windowed.
7. Optional, but very useful: change zone or go back to character select once, then fight again briefly.
8. Untick **Enable packet logging**, then close the meter.

Zip and send these files, even if nothing worked: a log of a failure is as useful as a log of success.

| File | Where |
| --- | --- |
| `debug.log` | `~/.local/share/com.a2tools.dps-meter/` |
| The newest `packets_YYYYMMDD_HHMMSS.txt` | `~/.local/share/com.a2tools.dps-meter/` |
| `meter-console.log` | your home folder |

Along with them, tell us:

- your distribution and version (for example Ubuntu 24.04, Fedora 41, CachyOS);
- whether you are on X11 or Wayland: the output of `echo $XDG_SESSION_TYPE`;
- your desktop (KDE Plasma, GNOME or other);
- your Proton version, and whether the game was fullscreen, borderless or windowed;
- whether damage showed up, and roughly when;
- the meter's ping next to the game's own ping;
- whether the meter stayed on top of the game;
- anything else that looked wrong.

The logs contain character names, yours and those of players near you, so send them only to us: on [Discord](https://discord.gg/Aion2Global) or in a [GitHub issue](https://github.com/taengu/A2Tools-DPS-Meter/issues).

## Troubleshooting

| What you see | What to do |
| --- | --- |
| No update question, though a new version is out | Updates come only to an installed package. A build from source updates with `git pull` and a rebuild. |
| The update asked for no password, or the meter did not come back | Install the current package [by hand](#update-by-hand). |
| Sign-in says the token could not be stored securely | No keyring is running. Install and enable KWallet (KDE) or GNOME Keyring, then sign in again. |
| Build fails mentioning `webkit2gtk-4.1`, `pkg-config` or a missing library | Re-run the install line for your distribution. Distributions older than Ubuntu 22.04 lack `webkit2gtk-4.1` and cannot build it. |
| `debug.log` says it failed to load libpcap | Install libpcap (`libpcap0.8` on Debian and Ubuntu, `libpcap` elsewhere), then start the meter again. |
| The meter warns it is not running as admin, or `debug.log` has no `Capture active` lines | The capture permission is missing. Package: reinstall it. Build from source: run the `setcap` line again (a rebuild loses it). |
| `debug.log` says `No AION2 window found` while the game is running | The meter did not find the game process. Send us the output of `ps aux \| grep -i aion` along with your logs. |
| `debug.log` says `Not locked yet` with `0 with game markers` while you fight | Capture sees traffic but not the game's. Tell us if you use a VPN or ping reducer. |
| The meter's window is blank or white | Start it with `WEBKIT_DISABLE_DMABUF_RENDERER=1` in front of the command (a known issue with some graphics drivers). |
| The meter goes behind the game | Run the game borderless or windowed. On Wayland an app cannot force itself on top of a fullscreen game. |

Stuck on something not listed? Send what you have so far, logs included.
