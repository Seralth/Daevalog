# Daevalog DPS Meter

[![License](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](LICENSE)

Daevalog DPS Meter is a damage meter for AION 2. It sits on top of the game as a small window and shows, while you fight, how much damage you and your party deal. It keeps your boss fights so you can look at them again later, skill by skill.

Daevalog is its own meter. It started as a fork of [A2Tools DPS Meter](https://github.com/taengu/A2Tools-DPS-Meter) by taengu, which is itself a Rust port of the original [AION2-DPS-Meter](https://github.com/TK-open-public/Aion2-Dps-Meter) by TK-open-public.

Daevalog reads the game's network traffic. It does not read the game's memory and does not change game files.

## Your data

Daevalog is built around one rule: your data stays yours, and anything shared is shared only because you chose to share it.

- No telemetry. Daevalog collects no usage statistics, sends no crash reports and never checks for updates.
- Your fights, settings and logs stay in your data folder, readable by your user only.
- Data leaves your computer only when you send it: when you upload a fight to a2tools.app (by hand, or automatically if you turn that on yourself), or when you use Send logs to a2tools.app. Nothing is turned on for you.
- The capture helper passes on only your own connections, and the meter keeps only the game's traffic.
- The one connection Daevalog makes on its own loads skill icons from the game's own image server, the same images the game shows. They are kept on your computer after the first time.

[docs/PRIVACY.md](docs/PRIVACY.md) lists every file the meter writes and everything an upload contains.

## Use at your own risk

Daevalog is third-party software. It is not made, supported or approved by NCSOFT. Using third-party programs alongside an online game may break the game's terms of service, and NCSOFT may act against accounts that use them, up to a ban. Daevalog only reads network traffic on your own computer and never changes the game or its files, but no one can promise how NCSOFT will treat it. You use it at your own risk.

Daevalog is provided as is, without warranty of any kind, as the GNU General Public License describes.

Daevalog is not affiliated with, endorsed by or connected to NCSOFT, AION 2, or A2Tools and its developer. AION 2 and NCSOFT are trademarks of NCSOFT Corporation. All game names, data and images belong to their owners.

## Install

Daevalog is made for Linux, with AION 2 running under Proton. No ready-made packages are published yet: you build Daevalog yourself from this repository.

- Arch, CachyOS, Manjaro, EndeavourOS: `cd packaging/arch && makepkg -si`. This builds the package `daevalog-dps-meter` and installs it.
- Other distributions: follow [Build from source](docs/linux.md#build-from-source-other-distributions) in the Linux guide.

To read the game's traffic, Daevalog needs the Linux permission `cap_net_raw` (the right to capture network packets). Only a small helper program, `daevalog-capture`, gets it. The meter itself runs with no extra permission. The Arch package sets this up for you.

Your settings, saved fights and sign-in are kept in `~/.local/share/com.daevalog.dps-meter`.

[docs/linux.md](docs/linux.md) is the full Linux guide: desktops, display settings, logs and troubleshooting.

### Supported systems

- Arch and Arch-based distributions (CachyOS, EndeavourOS, Manjaro): Arch package.
- Ubuntu-based distributions (Linux Mint, Pop!_OS) and Debian: .deb package.
- Fedora, Bazzite and openSUSE Tumbleweed: .rpm package.
- NixOS: a flake is planned.
- SteamOS (Steam Deck): through distrobox, as the Linux guide describes.

Other distributions: build from source. Packages for them are made on request.

Flatpak and AppImage are not supported: neither can give the capture helper the permission it needs. Requests for them will be closed.

## Status

- **Linux first.** Daevalog is developed and tested on Linux, with AION 2 running under Proton. The Windows code is still in the source, but Daevalog is not built, tested or supported on Windows.
- **No releases yet.** No packages are published. Build from source, as described above.
- **No automatic updates.** Daevalog never updates itself and never checks for updates. The package manager that installed it updates it.
- **Version.** Daevalog is version 1.0. Each build also has a revision number, the count of commits in this repository. Settings shows both, for example "Daevalog 1.0 · r250". The Arch package is numbered the same way, with the commit added: `1.0.r250.g8e3c962`.

## Reporting problems

Report problems with Daevalog in the [issues of this repository](https://github.com/Seralth/Daevalog/issues). Do not report them to the A2Tools DPS Meter project.

The quickest way is **Report a problem** in Settings, next to Open log folder. You choose one of three forms, and it opens on GitHub with the report info filled in: the Daevalog version, your system and desktop, the display backend, and whether packet capture works. The meter shows that text before anything opens. It sends nothing itself; you submit the form.

- **Wrong numbers.** Attach a prepared packet log of the fight and, if you have it, the record of the game's Combat Analysis (Ctrl+X).
- **Crash or won't start.** Attach `debug.log` from Open log folder.
- **Something else.** Any other problem, and ideas for new features.

Packet logs hold the name of every player you met, and issues are public. So for Wrong numbers, use **Prepare log**. It saves a copy in the log folder, named `report_` and the log's name, with every character name replaced. The copy replays to the same damage numbers. Chat messages can stay in it. Attach the copy, never a `packets_` file.

Security problems, anything that could hurt other players or users if posted in public, go to a private form instead: see [SECURITY.md](SECURITY.md). Report a problem has a button for it too.

## Uploads and a2tools.app

Daevalog can upload fights to a2tools.app, a site run by the A2Tools developer, where others can view them. To upload, you sign in with an a2tools.app account. Uploading is always your own choice: no upload option is turned on for you. Daevalog is not affiliated with a2tools.app.

Uploads tell a2tools.app they are compatible with A2Tools DPS Meter 2.0.44.

[docs/PRIVACY.md](docs/PRIVACY.md) says what the meter sends, what it keeps, and what an upload contains.

### Thanks to a2tools.app

a2tools.app is hosted and paid for by taengu, the A2Tools developer. Daevalog can upload fights and send logs there because he allows it. Thank you.

taengu accepts tips for the site and his work: [Ko-fi](https://ko-fi.com/hiddencube), [爱发电 (afdian)](https://afdian.com/a/hiddencube), [PayPal](https://www.paypal.me/taengoo), [NOWPayments](https://nowpayments.io/donation/thehiddencube). The full list is in the [A2Tools DPS Meter README](https://github.com/taengu/A2Tools-DPS-Meter#support). Daevalog takes no money.

## Coming from A2Tools DPS Meter?

This section lists what Daevalog does differently from A2Tools DPS Meter.

### Moving over
- The package `daevalog-dps-meter` replaces an installed `a2tools-dps-meter` package.
- On its first start, Daevalog moves the data folder of A2Tools DPS Meter (`com.a2tools.dps-meter`) to its own folder. Your settings, saved fights and sign-in come with it.
- Daevalog keeps reading A2Tools DPS Meter's settings and saved fights where practical. This is not promised: a later version may stop reading them.

### Meter
- **Encounter mode (ENC).** An encounter ends after a set time without combat by you or your party: 15 seconds by default, 5 to 300 seconds in Settings. A boss you hit keeps the encounter open while it lives. You choose what each row shows: ENCDPS, DPS over your own active time, damage share, total, crit rate, the last 10, 30 or 60 seconds, biggest hit and hits.
- **The game's own record.** The meter reads the records of the game's Damage Analyzer (Ctrl+X in the game) and matches them to saved fights. History and Details show the meter's numbers, the game's numbers, or both side by side, and mark every difference.
- **Skill rows as in the game.** Skills are grouped the way the game's Damage Analyzer groups them. Additional hits are read from the damage record, the spirits' hits included.
- **Hit results.** Details can show Shield Block, Parry, Perfect Block, Endurance, Regeneration, Miss and Resist for each skill. Hit types carry the game's names.
- **Skill details on hover** in every mode, including the modes that show several targets.
- **Details, History and the hover tooltip never wait on the meter,** and settings are saved in the background (from mazixs's upstream PR #29). Closing the meter window quits the app, even with Settings open, and saves your fights first.
- **Every UI string in all 10 languages.**
- DoT rows sit under their skill in Details in every language: they are matched by skill code, not by name.
- Damage and healing, per skill and in total, no longer stop or wrap around at about 2.1 billion.
- Targets you stopped fighting more than 30 seconds ago leave the meter's memory, unless the current mode still shows them. ALL without a time window keeps everything since the zone change.
- When packet capture cannot start on Linux, the meter says so in its own window, instead of the Windows prompt to download Npcap.
- The version in Settings opens the releases of this repository.
- **Open log folder** in Settings opens the folder with `debug.log` and the packet logs, to attach to an issue here. "Send logs to a2tools.app" says where the logs go: to the A2Tools developer, where Daevalog cannot help with them.

### Linux desktop
- **Tray icon,** with "Start in the tray" and "Keep out of the taskbar". The tray menu shows, hides, locks and unlocks the meter. Every control also stays in the meter's own window, so the meter works on a desktop without a tray.
- **Click-through lock** on X11 and on native Wayland. A locked meter always shows its lock button.
- **Lock hotkey** through the desktop's global shortcuts (the GlobalShortcuts portal), on desktops that offer them.
- **Display backend per desktop.** At start the meter picks X11, XWayland or native Wayland to fit the desktop, so KDE Plasma needs no `GDK_BACKEND=x11` launcher. A `GDK_BACKEND` you set yourself still wins.
- **Wayland layer overlay** on KDE Plasma, Hyprland and Sway, on by default there: the meter stays above a fullscreen game. Settings turns it off.
- Windows behave correctly under KDE Plasma (KWin), and the overlay draws on WebKitGTK 2.54.

### Parsing
- Packets are framed by the real length rule: payload plus 4.
- Each connection to the game server is read on its own. A TLS connection is left out whole.
- A fight keeps the dungeon it was fought in. A map load into the open world ends the dungeon.
- Spirits are linked to their owners by the game's link records and the spawn caster field. Owners are not guessed by power scalar or class.
- Fight slices keep skill ids that look like short text. They are no longer blanked out as names.

### Fights and identity
- The backend alone decides which player is the local player, from the game's own record of you.
- Every fight segment is saved before any reset. Only fights that you or your party fought are saved.
- A fight cleared by a zone change keeps the ids, names and spirit links it had.
- A fight's healing is the healing done during that fight, in live Details and in the saved fight alike.
- A fight saved when the meter closes is never overwritten by an older auto-save.

### Privacy
- The sign-in, upload and webview paths are hardened.
- The data folder and the files the meter writes there are readable by your user only.
- An upload is always your own choice. No upload option is turned on for you.
- Packet capture uses `cap_net_raw` only, and only this machine's own traffic (no promiscuous mode).
- Packet capture runs in a small helper program, `daevalog-capture`. Only the helper holds `cap_net_raw`, and it gives it up once the network devices are open. The meter itself runs with no capability.
- The capture helper passes on only your own connections; other users' traffic and system services are never read through it.
- Packet capture takes TCP only, and only the game server's port once the meter has found it.
- The fonts come with the meter, with Pretendard as the main font, as the stylesheet always intended. A window no longer loads fonts from unpkg.com.
- Links open only https addresses and the meter's own folders.
- A packet replay reads only files in the meter's data folder.
- Screenshots go only to the folder you chose in the meter, or the default folder, and never over an existing file.
- Signing in tells a2tools.app the system (Linux or Windows), not your computer's name.

### Left out
- Guessing who an unnamed actor is from its class.
- The one-time popups that offer Discord activity and sign-in with automatic upload.
- The update check and the updater.
- Discord activity and the Discord button in Settings.
- The supporter roster (gold names for A2Tools donors), which downloaded a list from a2tools.app every few hours.

Fixes that suit A2Tools DPS Meter are offered to that project as pull requests.

## For developers

`docs/ARCHITECTURE.md` describes the design. `CONTRIBUTING.md` describes how to test a change.

## License and credits

Daevalog DPS Meter is licensed under the GNU General Public License, version 3. See [LICENSE](LICENSE).

Daevalog is a fork of a fork:

- [AION2-DPS-Meter](https://github.com/TK-open-public/Aion2-Dps-Meter), Copyright (c) 2026 TK-open-public, under the MIT License, is the original meter. The way Daevalog reads the game's packets descends from it. The full MIT text is in [LICENSE](LICENSE).
- [A2Tools DPS Meter](https://github.com/taengu/A2Tools-DPS-Meter), Copyright (c) 2026 taengu, ported it to Rust and Tauri and built on it. Daevalog started as a fork of A2Tools DPS Meter v2.0.44.
- Modifications: Seralth, since 2026-10-02.

AION 2 is a trademark of NCSOFT. Daevalog DPS Meter is not affiliated with or endorsed by NCSOFT or A2Tools (see [Use at your own risk](#use-at-your-own-risk)).

The README of A2Tools DPS Meter, with Windows instructions and the community links of the A2Tools project, is in the [A2Tools DPS Meter repository](https://github.com/taengu/A2Tools-DPS-Meter).

### Fonts

The meter comes with these fonts. Each is under the SIL Open Font License, version 1.1.

- Noto Sans SC: [public/vendor/fonts/noto-sans-sc/LICENSE](public/vendor/fonts/noto-sans-sc/LICENSE)
- Noto Sans TC: [public/vendor/fonts/noto-sans-tc/LICENSE](public/vendor/fonts/noto-sans-tc/LICENSE)
- Pretendard: [public/vendor/fonts/pretendard/LICENSE](public/vendor/fonts/pretendard/LICENSE)
