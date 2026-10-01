# A2Tools DPS Meter on Linux (Proton)

The meter runs natively on Linux while AION 2 runs under Proton. On Arch and its derivatives (CachyOS, Manjaro, EndeavourOS) install the ready-made package; on any other distribution, build it from source. Linux support is new, so your logs help: see [Sending us your logs](#sending-us-your-logs).

## Install on CachyOS, Arch, Manjaro or EndeavourOS

Download the latest package first (pacman refuses unsigned packages straight from a URL), then install the file:

```bash
curl -LO https://cdn.a2tools.app/linux/a2tools-dps-meter-latest-x86_64.pkg.tar.zst
```

```bash
sudo pacman -U a2tools-dps-meter-latest-x86_64.pkg.tar.zst
```

The install should end with "A2Tools DPS Meter may now capture packets": the package grants the packet-capture permission itself, so you never need `setcap`. The program is `/usr/bin/a2tools-dps-meter`, and it is in your application menu. To start it from a terminal with its output saved:

```bash
a2tools-dps-meter 2>&1 | tee ~/meter-console.log
```

To remove it:

```bash
sudo pacman -R a2tools-dps-meter
```

## Updates

From 2.0.33 the package updates itself. When a new version is out, the meter asks "A new update is available! … Download and install now?" shortly after it starts:

1. Click **Yes**. The meter downloads the update and closes.
2. Your desktop asks for your password, the same prompt as for other system changes. Enter it.
3. The meter starts again by itself, on the new version.

If you cancel the password prompt, the meter restarts on the old version and asks again next time. To check which version you have:

```bash
pacman -Q a2tools-dps-meter
```

**If it says `2.0.30.r70.g0ac3fb6-1`** (the first test package): that version cannot update itself. Install the current package by hand once, with the two commands under [Install](#install-on-cachyos-arch-manjaro-or-endeavouros); every later version then arrives on its own.

## What works on Linux

| Feature | On Linux |
| --- | --- |
| Damage meter, Details, History | Works |
| Ping | Works |
| Finding the game | Looks for the running AION2.exe process under Proton |
| A2 Tools account sign-in | Works in the package from 2.0.33: kept in KWallet or GNOME Keyring, which may ask to create or unlock a wallet the first time |
| Automatic updates | Works in the package from 2.0.33; builds from source update with `git pull` |
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

**Steam Deck:** SteamOS is read-only, so building directly on it is hard. If that is your setup, tell us before you start and we will work out a way.

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
| No update question, though a new version is out | Updates come only to the package installed by pacman. A build from source updates with `git pull` and a rebuild. |
| The update asked for no password, or the meter did not come back | Install the current package by hand with the commands under [Install](#install-on-cachyos-arch-manjaro-or-endeavouros). |
| Sign-in says the token could not be stored securely | No keyring is running. Install and enable KWallet (KDE) or GNOME Keyring, then sign in again. |
| Build fails mentioning `webkit2gtk-4.1`, `pkg-config` or a missing library | Re-run the install line for your distribution. Distributions older than Ubuntu 22.04 lack `webkit2gtk-4.1` and cannot build it. |
| `debug.log` says it failed to load `libpcap.so.1` | Install libpcap, then start the meter again. |
| The meter warns it is not running as admin, or `debug.log` has no `Capture active` lines | The capture permission is missing. Package: reinstall it. Build from source: run the `setcap` line again (a rebuild loses it). |
| `debug.log` says `No AION2 window found` while the game is running | The meter did not find the game process. Send us the output of `ps aux \| grep -i aion` along with your logs. |
| `debug.log` says `Not locked yet` with `0 with game markers` while you fight | Capture sees traffic but not the game's. Tell us if you use a VPN or ping reducer. |
| The meter's window is blank or white | Start it with `WEBKIT_DISABLE_DMABUF_RENDERER=1` in front of the command (a known issue with some graphics drivers). |
| The meter goes behind the game | Run the game borderless or windowed. On Wayland an app cannot force itself on top of a fullscreen game. |

Stuck on something not listed? Send what you have so far, logs included.
