# Daevalog DPS Meter on Linux (Proton)

Daevalog runs natively on Linux while AION 2 runs under Proton. Daevalog builds come from this repository. No ready-made packages are published yet, so you build Daevalog on your own computer: on Arch and related systems as a package, elsewhere from source. If something does not work, your logs help: see [Sending us your logs](#sending-us-your-logs).

Daevalog needs a 64-bit (x86_64) system with WebKitGTK 4.1: Ubuntu 22.04, Debian 12, Fedora 39 or newer, any current Arch, or Bazzite and SteamOS through distrobox.

## Contents

[![Ubuntu](https://img.shields.io/badge/Ubuntu-E95420?logo=ubuntu&logoColor=white)](#other-distributions) [![Debian](https://img.shields.io/badge/Debian-A81D33?logo=debian&logoColor=white)](#other-distributions) [![Linux Mint](https://img.shields.io/badge/Linux_Mint-87CF3E?logo=linuxmint&logoColor=white)](#other-distributions) [![Pop!_OS](https://img.shields.io/badge/Pop%21__OS-48B9C7?logo=popos&logoColor=white)](#other-distributions) [![Fedora](https://img.shields.io/badge/Fedora-51A2DA?logo=fedora&logoColor=white)](#other-distributions) [![Bazzite](https://img.shields.io/badge/Bazzite-8A3FFC?logo=fedora&logoColor=white)](#steam-deck-bazzite-and-other-read-only-systems) [![Steam Deck](https://img.shields.io/badge/Steam_Deck-1A9FFF?logo=steamdeck&logoColor=white)](#steam-deck-bazzite-and-other-read-only-systems) [![openSUSE](https://img.shields.io/badge/openSUSE-73BA25?logo=opensuse&logoColor=white)](#other-distributions) [![Arch](https://img.shields.io/badge/Arch-1793D1?logo=archlinux&logoColor=white)](#cachyos-arch-manjaro-endeavouros) [![CachyOS](https://img.shields.io/badge/CachyOS-08A88A?logo=cachyos&logoColor=white)](#cachyos-arch-manjaro-endeavouros) [![Manjaro](https://img.shields.io/badge/Manjaro-35BF5C?logo=manjaro&logoColor=white)](#cachyos-arch-manjaro-endeavouros) [![EndeavourOS](https://img.shields.io/badge/EndeavourOS-7F3FBF?logo=endeavouros&logoColor=white)](#cachyos-arch-manjaro-endeavouros)

- **[Install](#install):** [Arch, CachyOS, Manjaro, EndeavourOS](#cachyos-arch-manjaro-endeavouros) · [Steam Deck, Bazzite and other read-only systems](#steam-deck-bazzite-and-other-read-only-systems) · [Other distributions](#other-distributions) · [Coming from A2Tools DPS Meter](#coming-from-a2tools-dps-meter)
- **[Update](#update):** [how to update](#how-to-update) · [your version](#your-version)
- **[Start and remove](#start-and-remove)**
- **[What works on Linux](#what-works-on-linux)**
- **[Display backend](#display-backend)**
- **[GNOME: keep the meter above other windows](#gnome-keep-the-meter-above-other-windows)**
- **[Build from source](#build-from-source-other-distributions)**, for distributions with no package
- **[Sending us your logs](#sending-us-your-logs)**
- **[Tiling desktops](#tiling-desktops-hyprland-sway-i3)** (Hyprland, Sway, i3)
- **[Troubleshooting](#troubleshooting)**

## Install

To read the game's network traffic, Daevalog needs the Linux permission `cap_net_raw` (the right to capture packets). Only the small helper program `daevalog-capture` gets it; the meter itself runs with no extra permission. The packages grant it when they install. A build from source needs one `setcap` command, shown in its step 3.

Signed packages for Arch, Debian 13, Linux Mint, Pop!_OS, Fedora, Bazzite and openSUSE Tumbleweed are at **https://packages.seralth.com**, with the steps to add the repository on each. Once it is added, the meter updates with the rest of the system.

### CachyOS, Arch, Manjaro, EndeavourOS

Use the pacman repository from https://packages.seralth.com.

To build the package yourself instead: the repository holds an Arch package recipe (`packaging/arch/PKGBUILD`). `makepkg` builds the package `daevalog-dps-meter` from it and installs it. The first build takes 10–15 minutes.

```bash
sudo pacman -S --needed git base-devel
```

```bash
git clone https://github.com/Seralth/Daevalog.git
cd Daevalog/packaging/arch
makepkg -si
```

`makepkg -s` installs the build tools the recipe lists (Rust, Node.js) with pacman, and `-i` installs the finished package. Keep the `Daevalog` folder: updates are built from it.

### Steam Deck, Bazzite and other read-only systems

Bazzite and the other image-based Fedoras: use the rpm repository from https://packages.seralth.com (`rpm-ostree install`, then a restart). No distrobox is needed.

SteamOS replaces its read-only system with every update, so anything installed into it directly is wiped. Instead, Daevalog goes in a **distrobox**: a container with its own Arch Linux inside, which SteamOS 3.5 and later include. It must be created with `--root`: an ordinary (rootless) container cannot read the game's network traffic.

The overlay only works in **Desktop Mode**. In Game Mode, nothing can draw over the game.

1. Switch to Desktop Mode: on a Steam Deck, press the **Steam** button, then **Power**, then **Switch to Desktop**.
2. Open a terminal (**Konsole** on a Steam Deck). On a Steam Deck where you have never set a password for the `deck` user, set one now (sudo needs it):

    ```bash
    passwd
    ```

3. Create the box:

    ```bash
    distrobox create --root --name daevalog --image archlinux:latest
    ```

    The first time you enter it (the next step), it takes a few minutes to set up and asks you to choose a password for your user inside the box. Any password will do; sudo inside the box asks for it.

4. Build and install Daevalog inside the box. The box shares your home folder, so the `Daevalog` folder lands there. The build takes 10–15 minutes.

    ```bash
    distrobox enter --root daevalog -- sudo pacman -Syu --noconfirm --needed git base-devel
    ```

    ```bash
    distrobox enter --root daevalog -- sh -c 'cd ~ && git clone https://github.com/Seralth/Daevalog.git && cd Daevalog/packaging/arch && makepkg -si --noconfirm'
    ```

5. Start AION 2 from Steam, still in Desktop Mode, set to borderless or windowed. Then start the meter from the terminal:

    ```bash
    distrobox enter --root daevalog -- daevalog-dps-meter
    ```

System updates leave the box alone, so the meter survives them. This route is new and not yet confirmed on a real Steam Deck: please tell us how it goes.

### Other distributions

Debian 13, Linux Mint, Pop!_OS, Fedora and openSUSE Tumbleweed: use the repository for your system from https://packages.seralth.com.

Others: follow [Build from source](#build-from-source-other-distributions).

### Coming from A2Tools DPS Meter

- Your settings, saved fights and sign-in come with you: on its first start, Daevalog moves the folder of A2Tools DPS Meter (`~/.local/share/com.a2tools.dps-meter`) to its own, `~/.local/share/com.daevalog.dps-meter`.
- The Arch package `daevalog-dps-meter` replaces an installed `a2tools-dps-meter` package.
- On other systems, remove the A2Tools DPS Meter package first, so you do not start the old meter by mistake: `sudo apt remove a2-tools-dps-meter` (Ubuntu, Debian, Mint, Pop!_OS), `sudo dnf remove a2-tools-dps-meter` (Fedora), `rpm-ostree uninstall a2-tools-dps-meter` and a restart (Bazzite and other image-based Fedoras), `sudo zypper remove a2-tools-dps-meter` (openSUSE).

## Update

### How to update

Daevalog never updates itself and never checks for updates. Installed from a package repository, it updates with the rest of the system. Built yourself, you update it by building the newest version from the `Daevalog` folder.

| Installed with | Update with |
| --- | --- |
| A package repository | Your normal system update: `sudo pacman -Syu`, `sudo apt upgrade`, `sudo dnf upgrade`, `rpm-ostree upgrade`, `sudo zypper dup` |
| makepkg (Arch, CachyOS, Manjaro, EndeavourOS) | `cd Daevalog && git pull && cd packaging/arch && makepkg -si` |
| Steam Deck (distrobox) | `distrobox enter --root daevalog -- sh -c 'cd ~/Daevalog && git pull && cd packaging/arch && makepkg -si --noconfirm'` |
| Built from source | `git pull`, then the build lines and the `setcap` line again, as [Build from source](#build-from-source-other-distributions) says |

Your settings and fight history stay as they are.

### Your version

Settings shows the version at the top, for example "Daevalog 1.0 · r250". The number after the "r" is the revision: the higher, the newer.

The Arch package shows the same, with the commit added:

```bash
pacman -Q daevalog-dps-meter
```

## Start and remove

The meter is in your application menu, as Daevalog DPS Meter; in a distrobox, start it from a terminal as in [step 5](#steam-deck-bazzite-and-other-read-only-systems). To start it from a terminal with its output saved, which helps if you send us logs:

```bash
daevalog-dps-meter 2>&1 | tee ~/meter-console.log
```

To remove it:

| Installed on | Command |
| --- | --- |
| Arch, CachyOS, Manjaro, EndeavourOS | `sudo pacman -R daevalog-dps-meter` |
| Steam Deck, Bazzite (distrobox) | `distrobox rm --root daevalog` (removes the whole box) |
| Built from source | Delete the `Daevalog` folder |

Your settings and fights stay in `~/.local/share/com.daevalog.dps-meter`. Delete that folder too to remove them.

## What works on Linux

| Feature | On Linux |
| --- | --- |
| Damage meter, Details, History | Works |
| Ping | Works. It is the game's own ping, timed on the game's connection, so it includes a VPN if you use one and the time the game server takes to answer. A busy server (a crowded world boss) makes it jump while the network itself stays steady |
| Finding the game | Looks for the running AION2.exe process under Proton |
| a2tools.app sign-in | Works: kept in KWallet or GNOME Keyring, which may ask to create or unlock a wallet the first time |
| Updates | From a package repository, with your normal system update; built yourself, by rebuilding (see [Update](#update)). The meter never updates itself |
| Class icons | Works |
| Global hotkeys | The lock hotkey, through the desktop's global shortcuts (the GlobalShortcuts portal), on desktops that offer them |
| Click-through lock | Works on X11, XWayland and native Wayland. A locked meter always shows its lock button |
| Screenshots | Works, to the clipboard and a folder (`~/Pictures/Daevalog DPS Meter` by default). On Linux the meter pictures itself on a plain background, since Wayland lets no app copy the screen |
| Auto-hide when the game loses focus | Not yet (the meter stays visible) |

## Display backend

The meter picks how it draws on each desktop when it starts. The log says which: `display backend: ...`.

| Desktop | Backend |
| --- | --- |
| X11 sessions (Xfce, Cinnamon, Plasma X11) | X11 |
| GNOME | XWayland, see below |
| KDE Plasma, Hyprland, Sway (Wayland) | Native Wayland, with the overlay as a layer above every window, a fullscreen game too |
| Other Wayland desktops | Native Wayland |

The layer needs `gtk-layer-shell`. **Settings > Overlay: Wayland layer** turns it off; it takes effect at the next start. With the layer off or `gtk-layer-shell` missing, KDE Plasma runs the meter through XWayland, where KWin keeps it on top, and Hyprland and Sway open a normal window (see [Tiling desktops](#tiling-desktops-hyprland-sway-i3)). On other Wayland desktops the switch turns the layer on, when the desktop offers one.

A `GDK_BACKEND` you set yourself always wins. A launcher with `env GDK_BACKEND=x11` is no longer needed on KDE Plasma.

## GNOME: keep the meter above other windows

In a GNOME Wayland session, the meter automatically prefers XWayland when an X11 display is available. This lets its always-on-top request work without installing a GNOME Shell component. Only the meter uses XWayland; the desktop session stays on Wayland.

An explicit `GDK_BACKEND` takes precedence. To select XWayland manually:

```sh
GDK_BACKEND=x11 daevalog-dps-meter
```

Depending on GNOME's fractional-scaling configuration, XWayland text may look softer at 125% or 150%. If text looks blurry, try native Wayland:

```sh
GDK_BACKEND=wayland daevalog-dps-meter
```

If XWayland is unavailable, the meter also falls back to native Wayland. GTK's native Wayland keep-above request has no effect in GNOME: focus the meter, press **Alt+Space**, and select **Always on Top**. Repeat for Details or other meter windows as needed. Close the focused window with **Alt+F4**, or use **Settings > Quit** to exit the meter.

### Resizing on GNOME

Drag the meter's bottom-right resize handle, or the edges of a tool window. On GNOME with X11, XWayland or native Wayland, the compositor resizes the actual window instead of temporarily expanding a transparent viewport. This avoids the expansion moving the meter back onto the screen. The application detects its actual display backend, including XWayland inside a Wayland session.

KDE Plasma, Hyprland, Sway, i3 and other desktops keep the overlay viewport resizing and tool-window edge resizing. The GNOME path is not enabled on those desktops.

The meter's minimum height is measured from its current content, so shrinking removes empty space without letting the frame overlap the header, rows or footer.

Pausing with the mouse button held does not end the resize. If you release outside the window, move the pointer back over it to resume automatic sizing to the meter's content.

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

Arch, Manjaro, EndeavourOS (to build without the package):

```bash
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module libappindicator-gtk3 librsvg libpcap git
```

**Steam Deck, Bazzite:** follow [Steam Deck, Bazzite and other read-only systems](#steam-deck-bazzite-and-other-read-only-systems) instead.

### 2. Download and build the meter

The first build takes 10–15 minutes; later ones are much faster. Run these from your home folder or wherever you keep projects:

```bash
git clone https://github.com/Seralth/Daevalog.git
cd Daevalog

# A fresh download lacks some data files the app reads; copy them into place
mkdir -p public/i18n public/data public/src/data
cp -r src/data/i18n/* public/i18n/
cp src/data/skill_icons.json src/data/dot_skill_ids.json public/data/
cp src/data/skill_icons.json src/data/dot_skill_ids.json public/src/data/

npm install
npx tauri build --no-bundle
cargo build --release -p daevalog-capture --manifest-path src-tauri/Cargo.toml
```

When it finishes, the meter is these two files, which stay together in one folder:

```bash
src-tauri/target/release/daevalog-dps-meter
src-tauri/target/release/daevalog-capture
```

`daevalog-capture` is the capture helper. The meter starts it from its own folder; it reads the network traffic and passes the game's packets to the meter.

To pick up fixes later: `git pull`, then run the `npx tauri build --no-bundle` and `cargo build` lines again, then redo step 3.

### 3. Allow the capture helper to capture packets

Reading the game's network traffic needs a capability Linux only gives on request. Grant it to the capture helper only, from the `Daevalog` folder:

```bash
sudo setcap cap_net_raw=ep src-tauri/target/release/daevalog-capture
```

- The meter itself needs no capability. If an older build of the meter has one, remove it: `sudo setcap -r src-tauri/target/release/daevalog-dps-meter`.
- Redo this after every rebuild: a new build is a new file and loses the permission.
- Older versions of this guide also granted `cap_net_admin`. The line above replaces the whole list, so running it again removes that.
- Do not run the meter itself with `sudo`. It would run as root, keep its settings in root's home folder, and often fail to open its window.
- `setcap` does not work on some drives (for example NTFS or exFAT shared with Windows). If it fails, build the meter on a Linux-formatted drive.

Then start it from a terminal, so its output is saved too:

```bash
src-tauri/target/release/daevalog-dps-meter 2>&1 | tee ~/meter-console.log
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
| `debug.log` | `~/.local/share/com.daevalog.dps-meter/` |
| The newest `packets_YYYYMMDD_HHMMSS.txt` | `~/.local/share/com.daevalog.dps-meter/` |
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

The logs contain character names, yours and those of players near you, so send them only to us, in a [GitHub issue](https://github.com/Seralth/Daevalog/issues).

## Tiling desktops (Hyprland, Sway, i3)

A tiling window manager tiles every new window unless told otherwise, which breaks an overlay: the meter is squeezed beside the game, and the tooltips on its damage bars appear in the wrong place. Give the meter's window (class `daevalog-dps-meter`) a rule that makes it float, keeps it on every workspace, and turns off blur and borders.

On Hyprland, a player reported this rule works:

```lua
hl.window_rule({
    name = "aion-dps-meter",
    match = {
        class = "^(daevalog\\-dps\\-meter)$",
    },
    float = true,
    pin = true,
    no_blur = true,
    border_size = 0,
})
```

That is Hyprland's Lua configuration. If you use `hyprland.conf` instead, set the same four things (float, pin, no blur, no border) for that window class in its window-rule syntax. On Sway and i3, the equivalent is `floating enable` and `sticky enable` for `app_id`/`class` `daevalog-dps-meter`.

## Troubleshooting

| What you see | What to do |
| --- | --- |
| Sign-in says the token could not be stored securely | The meter keeps your sign-in in the desktop keyring and never in a plain file. The message says what went wrong. **No desktop keyring is running**: install and start GNOME Keyring (`gnome-keyring`) or KWallet, or turn on Secret Service in KeePassXC; on Hyprland, Sway or i3 start the keyring with your session (for example `exec-once = gnome-keyring-daemon --start --components=secrets`). **Stayed locked**: accept the keyring's unlock prompt. **No collection**: accept the prompt to create a keyring (the meter asks for one). Then sign in again. |
| Build fails mentioning `webkit2gtk-4.1`, `pkg-config` or a missing library | Re-run the install line for your distribution. Distributions older than Ubuntu 22.04 lack `webkit2gtk-4.1` and cannot build it. |
| `debug.log` says it failed to load libpcap | Install libpcap (`libpcap0.8` on Debian and Ubuntu, `libpcap` elsewhere), then start the meter again. |
| The meter warns it is not running as admin, or `debug.log` has no `Capture active` lines, or says `Packet capture is off` | The capture helper `daevalog-capture` is missing from the meter's folder, or lacks the capture permission. Arch package: build and install it again. Build from source: build the helper and run the `setcap` line again (a rebuild loses it). |
| `debug.log` says `No AION2 window found` while the game is running | The meter did not find the game process. Send us the output of `ps aux \| grep -i aion` along with your logs. |
| `debug.log` says `Not locked yet` with `0 with game markers` while you fight | Capture sees traffic but not the game's. Tell us if you use a VPN or ping reducer. |
| The window never opens, and the terminal says `Error 71 (Protocol error) dispatching to Wayland display`; or the window is blank or white | WebKit handed its frames to the compositor as GPU buffers, which some setups reject (NVIDIA drivers especially). The meter now has WebKit hand them over in shared memory instead (`WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`). Remove `WEBKIT_DISABLE_DMABUF_RENDERER=1` if you added it to a launcher: on WebKitGTK 2.54 it leaves the window mostly blank. If the window is still wrong, try `GDK_BACKEND=x11 daevalog-dps-meter`, which runs it through XWayland, and tell us. |
| The meter goes behind the game | Run the game borderless or windowed. On GNOME, see [automatic pinning through XWayland and the native Wayland workaround](#gnome-keep-the-meter-above-other-windows). On Wayland an app cannot force itself on top of a fullscreen game. On KDE Plasma, KWin can count a borderless game as fullscreen anyway: add a window rule for the meter (System Settings → Window Management → Window Rules) with **Layer** set to **Overlay**, forced. Thanks to Seralth for this. |
| After a WebKitGTK update the meter's window draws only in pieces, or only while you hover or drag it | WebKitGTK 2.54 no longer draws the transparent overlay fully without its DMA-BUF renderer, which older versions of the meter turned off. Update the meter, or start an older version with `WEBKIT_DISABLE_DMABUF_RENDERER=0 WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 daevalog-dps-meter`. |
| On a tiling desktop the meter is tiled beside the game, or its tooltips appear in the wrong place | See [Tiling desktops](#tiling-desktops-hyprland-sway-i3). |

Stuck on something not listed? Send what you have so far, logs included.
