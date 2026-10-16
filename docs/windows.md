# Daevalog DPS Meter on Windows

Daevalog 2.0 runs on Windows 10 and Windows 11, 64-bit. It is the same meter as on Linux: the same Qt Quick windows and the same parser.

Windows builds are tested on recorded fights and by the build workflow, not in live play yet. Live capture on Windows has not been checked against the game. If something does not work, please report it (see [Reporting a problem](#reporting-a-problem)).

## Install

1. **Install Npcap first.** Get it from [npcap.com](https://npcap.com). The free edition works. Keep the installer's default options. Daevalog reads the game's network traffic through Npcap. Npcap's licence does not allow other installers to include it, so Daevalog's installer does not.
2. **Install Daevalog.** Run `daevalog-dps-meter-<version>-setup.exe`. It installs the meter, the Qt runtime it needs and a Start menu entry. If Npcap is missing, the installer says so and offers to open npcap.com.
3. Start **Daevalog DPS Meter** from the Start menu.

The installer is not signed yet. Windows SmartScreen may warn about an unknown publisher. Choose **More info**, then **Run anyway**.

Installers are built by the workflow `.github/workflows/windows.yml`. Each run keeps the installer as an artifact named `daevalog-dps-meter-windows-installer`.

## Remove

Use **Settings → Apps → Installed apps → Daevalog DPS Meter → Uninstall** (on Windows 10, **Apps & features**), or **Uninstall Daevalog DPS Meter** in the Start menu. Your settings and saved fights stay in the data folder. Delete that folder to remove them too.

## What works

- Live capture through Npcap, in the meter's own process. There is no capture helper on Windows. Without Npcap the meter shows a notice; click it to open npcap.com.
- The meter window: frameless, see-through and kept above other windows. A game in borderless or windowed mode stays below it. A game in exclusive fullscreen may cover it.
- The lock: the meter lets clicks through to the game. On Windows the whole meter lets clicks through, so unlock it from the tray icon or with the lock key.
- The lock key: Settings → Keybinds. The meter registers it with Windows. If another program already uses the key, the page says so; record another key.
- The tray icon and its menu: show and hide, lock, Settings, History, Quit.
- Auto-hide: the meter knows when the game's window is in front.
- Details, History, Settings, saved fights, game records (Combat Analysis), uploads and the A2 Tools account.
- **Show Details on monitor** and the window places, which Windows lets the meter set.

Not on Windows: the Wayland layer overlay, **Keep out of the taskbar** and the other Linux and X11 settings. Settings does not show them.

## Data folder

`%APPDATA%\daevalog-dps-meter`. It holds `settings.json`, `history\`, `slices\`, `debug.log` and, when packet logging is on, `packets_*.txt`. Packet logs contain chat and the names of other players. Do not share packet logs.

**Settings → Diagnostics → Open log folder** opens it.

The A2 Tools sign-in is kept in Windows Credential Manager, under a name that starts with `daevalog-dps-meter/`. `credentials.dat` in the data folder only names that entry.

## Reporting a problem

**Settings → Diagnostics → Report a problem** opens a page where you choose what went wrong. The issue form then opens on GitHub with your version and Windows version filled in. Turn on **Enable debug logging** (Settings → Diagnostics) and attach `debug.log` from the data folder.

## Build from source

The workflow `.github/workflows/windows.yml` shows every step. In short, with Visual Studio 2022 (C++ tools), Rust and Qt 6 (with Qt Shader Tools) installed:

```powershell
$env:QMAKE = "C:\Qt\6.x.y\msvc2022_64\bin\qmake.exe"
cd src-qml
cargo build --release
```

Then copy `target\release\daevalog-qml.exe` to a new folder as `daevalog-dps-meter.exe` and run `windeployqt --release --qmldir <repo>\src-qml --no-translations --no-compiler-runtime` on it. Copy into that folder the Visual C++ runtime DLLs (from `%VCToolsRedistDir%\x64\Microsoft.VC*.CRT`), `src\data` as `data`, and `src-tauri\icons\icon.ico` as `daevalog.ico`. Then `iscc /DSourceDir=<that folder> packaging\windows\daevalog-dps-meter.iss` (Inno Setup 6.3 or later) makes the installer.
