# Daevalog DPS Meter

[![License](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)

Daevalog DPS Meter is a damage meter for AION 2. It sits on top of the game as a small window and shows, while you fight, how much damage you and your party deal. It keeps your boss fights so you can look at them again later, skill by skill.

Daevalog is its own meter. It started as a fork of [A2Tools DPS Meter](https://github.com/taengu/A2Tools-DPS-Meter) by taengu, which is itself a Rust port of the original [AION2-DPS-Meter](https://github.com/TK-open-public/Aion2-Dps-Meter) by TK-open-public.

Daevalog reads the game's network traffic. It does not read the game's memory and does not change game files.

## Your data

Daevalog is built around one rule: your data stays yours, and anything shared is shared only because you chose to share it.

- No telemetry. Daevalog collects no usage statistics, sends no crash reports and never checks for updates.
- Your fights, settings and logs stay in your data folder. The files the meter writes there are readable by your user only.
- Data leaves your computer only when you send it: when you upload a fight to a2tools.app (by hand, or automatically if you turn that on yourself). Nothing is turned on for you.
- The capture helper passes on only your own connections, and the meter keeps only the game's traffic.
- Daevalog makes no connection on its own. Only after you sign in to a2tools.app does it talk to a2tools.app: when you open the account page in Settings, and when it uploads a fight. Its skill icons and fonts come with it.

[docs/PRIVACY.md](docs/PRIVACY.md) lists every file the meter writes and everything an upload contains.

## Use at your own risk

Daevalog is third-party software. It is not made, supported or approved by NCSOFT. Using third-party programs alongside an online game may break the game's terms of service, and NCSOFT may act against accounts that use them, up to a ban. Daevalog only reads network traffic on your own computer and never changes the game or its files, but no one can promise how NCSOFT will treat it. You use it at your own risk.

Daevalog is provided as is, without warranty of any kind, as its licence describes.

Daevalog is not affiliated with, endorsed by or connected to NCSOFT, AION 2, or A2Tools and its developer. AION 2 and NCSOFT are trademarks of NCSOFT Corporation. All game names, data and images belong to their owners.

## Install

Daevalog is made for Linux, with AION 2 running under Proton. Daevalog 2.0 draws its windows with Qt 6 (Qt Quick).

Install from [packages.seralth.com](https://packages.seralth.com): it has the steps to add the package repository for Arch and Arch-based systems, Debian 13, LMDE 7, Linux Mint 22, Pop!_OS 24.04, Fedora, Bazzite and openSUSE Tumbleweed. Updates then come with the rest of the system. On the Steam Deck, the meter goes into a distrobox and installs from the Arch repository there, as the [Linux guide](docs/linux.md#steam-deck-steamos) describes.

To build the package yourself instead:

- Arch and Arch-based: build the package from `packaging/qml/arch` with `makepkg`, as the [Linux guide](docs/linux.md#arch-cachyos-manjaro-endeavouros) describes.
- Debian 13, LMDE 7, Linux Mint 22 and Pop!_OS 24.04: build the .deb with `packaging/qml/deb/build.sh` on the system it is for, as the [Linux guide](docs/linux.md#debian-13-lmde-7-linux-mint-22-pop_os-2404) describes.
- Fedora and openSUSE Tumbleweed: build the .rpm from `packaging/qml/rpm` on the system it is for, as the Linux guide describes for [Fedora](docs/linux.md#fedora) and [openSUSE Tumbleweed](docs/linux.md#opensuse-tumbleweed).
- Other distributions: follow [Building from source](docs/linux.md#building-from-source) in the Linux guide.

To read the game's traffic, Daevalog needs the Linux permission `cap_net_raw` (the right to capture network packets). Only a small helper program, `daevalog-capture`, gets it. The meter itself runs with no extra permission. The packages set this up for you.

Your settings and saved fights are kept in `~/.local/share/daevalog-dps-meter`; the sign-in is kept in your desktop's keyring.

[docs/linux.md](docs/linux.md) is the full Linux guide: desktops, display settings, logs and troubleshooting.

### Supported systems

Daevalog 2.0 needs Qt 6. The oldest Qt it is checked on is 6.4.2, the Qt of Linux Mint 22 and Pop!_OS 24.04; the Linux guide lists what differs there.

- Arch and Arch-based distributions (CachyOS, EndeavourOS, Manjaro): Arch package.
- Debian 13, LMDE 7, Linux Mint 22 and Pop!_OS 24.04: .deb package (one built on Debian 13, one on Ubuntu 24.04 for Mint and Pop).
- Fedora, and Bazzite and the other image-based Fedoras: .rpm package.
- openSUSE Tumbleweed: .rpm package, built on Tumbleweed.
- SteamOS (Steam Deck): the Arch package in a distrobox, as the Linux guide describes.

Other distributions: build from source. Packages for them are made on request.

Flatpak and AppImage are not supported: neither can give the capture helper the permission it needs. Requests for them will be closed.

## Status

- **Linux first.** Daevalog is developed and tested on Linux, with AION 2 running under Proton. For Windows, see [docs/windows.md](docs/windows.md).
- **Releases.** Packages are published on [packages.seralth.com](https://packages.seralth.com). Each GitHub release carries the same files as a mirror. This repository holds the source of 2.0 and later; work in progress is not published here.
- **No automatic updates.** Daevalog never updates itself and never checks for updates. A package from packages.seralth.com updates with the rest of the system; a package you built yourself is updated by building the newest version, as the [Linux guide](docs/linux.md#updating) describes.
- **Version.** Daevalog is version 2.0. Each build also has a revision number, the count of commits in this repository. Settings shows both, for example "Daevalog 2.0 · r1". The Arch package is numbered the same way, with the commit added: `2.0.0.r1.g0123abc`.
- **1.x.** The 1.x meter (Tauri and WebKitGTK) is the package `daevalog-dps-meter-legacy`, on packages.seralth.com only. It installs beside 2.0. Support for 1.x ends with 2.0.0: the legacy package (r607) is the last 1.x build and gets no more updates, apart from fixes for security problems.

## Reporting problems

Report problems with Daevalog in the [issues of this repository](https://github.com/Seralth/Daevalog/issues). Do not report them to the A2Tools DPS Meter project.

The quickest way is **Report a problem** in Settings, next to Open log folder. You choose one of three forms, and it opens on GitHub with the report info filled in: the Daevalog version, your system, desktop and session type, the Qt version and display backend, and whether packet capture works. The meter shows that text before anything opens. It sends nothing itself; you submit the form.

- **Wrong numbers.** Attach a prepared packet log of the fight and, if you have it, the record of the game's Combat Analysis (Ctrl+X).
- **Crash or won't start.** Press **Prepare debug log** and attach the `report_debug.log` it saves: `debug.log` with the character names, your home folder and your user name taken out.
- **Something else.** Any other problem, and ideas for new features.

Packet logs hold the name of every player you met, and issues are public. So for Wrong numbers, use **Prepare log**. It saves a copy in the log folder, named `report_`, the log's name and `.gz` (compressed), with every character name replaced. The copy replays to the same damage numbers. Chat messages can stay in it. Attach the copy, never a `packets_` file.

Security problems, anything that could hurt other players or users if posted in public, go to a private form instead: see [SECURITY.md](SECURITY.md). Report a problem has a button for it too.

## Uploads and a2tools.app

Daevalog can upload fights to a2tools.app, a site run by the A2Tools developer, where others can view them. To upload, you sign in with an a2tools.app account. Uploading is always your own choice: no upload option is turned on for you. Daevalog is not affiliated with a2tools.app.

Uploads tell a2tools.app they come from Daevalog and which version. a2tools.app counts them on its rankings and class statistics.

Every request the meter makes, every file it writes, and every field of an upload are listed in [docs/PRIVACY.md](docs/PRIVACY.md).

### Thanks to a2tools.app

a2tools.app is hosted and paid for by taengu, the A2Tools developer. Daevalog can upload fights there because he allows it. Thank you.

taengu accepts tips for the site and his work: [Ko-fi](https://ko-fi.com/hiddencube), [爱发电 (afdian)](https://afdian.com/a/hiddencube), [PayPal](https://www.paypal.me/taengoo), [NOWPayments](https://nowpayments.io/donation/thehiddencube). The full list is in the [A2Tools DPS Meter README](https://github.com/taengu/A2Tools-DPS-Meter#support). Daevalog takes no money.

## Coming from A2Tools DPS Meter?

This section lists what Daevalog does differently from A2Tools DPS Meter.

### Moving over
- The package `daevalog-dps-meter` replaces an installed A2Tools DPS Meter package (`a2tools-dps-meter` on Arch, `a2-tools-dps-meter` on Debian and Fedora).
- Daevalog 2.0 reads only the folder of Daevalog 1.x (`com.daevalog.dps-meter`), once, on its first start. Daevalog 1.x moved the data folder of A2Tools DPS Meter (`com.a2tools.dps-meter`) there on its own first start, so settings and saved fights that went through 1.x come over; the sign-in does not.
- Daevalog keeps reading the settings and saved fights of earlier meters where practical. This is not promised: a later version may stop reading them.
- Settings are kept in `settings.json` only, never in a copy inside a window.

### Meter
- **Encounter mode (ENC).** An encounter ends after a set time without combat by you or your party: 15 seconds by default, 5 to 300 seconds in Settings. A boss you hit keeps the encounter open while it lives, for up to 5 minutes without combat.
- **Columns.** You choose what each DPS row shows, in every mode, in Settings > Meter > Columns: DPS, Active DPS, damage share, total, crit rate, the last 10, 30 or 60 seconds, biggest hit and hits. The Heal and Tank tabs have their own columns.
- **DPS is damage over the whole fight,** as everyone uses the word. A2Tools DPS Meter calls that number ENCDPS in ENC mode. Its DPS, which runs from your first hit to your last, is called Active DPS. The ENC columns chosen in A2Tools DPS Meter or Daevalog 1.x are not taken over: pick them again in Settings.
- **Column heads in the game's own codes.** Details and the meter head their columns the way the game's Combat Analysis does: PERF, DOUB, BACK, FRNT, BLOC, MULT, ENDR, RSTO and HIT, in each language's own form (Spanish BLOQ, GOLPE…). Critical hits are CRIT (Spanish CRÍT). BLOC heads Parry: the game's Block counted a parried hit. Each head's tooltip gives the full name. A2Tools DPS Meter calls the hits column "Casts".
- The a2tools.app account note says what signing in does: Daevalog asks a2tools.app who you are and sends the fights you upload. A2Tools DPS Meter says it syncs characters and settings.
- **The game's own record.** The meter reads the records of the game's Combat Analysis (Ctrl+X in the game) and matches them to saved fights. History's Game record tab shows the meter's numbers, the game's numbers, or both side by side, and marks every difference.
- **Skill rows as in the game.** Skills are grouped the way the game's Combat Analysis groups them. Additional hits are read from the damage record, the spirits' hits included.
- **Hit results.** Details can show Shield Block, Parry, Perfect Block, Endurance, Regeneration, Miss and Resist for each skill. Hit types carry the game's names.
- A spirit's damage over time stops counting when the spirit is unsummoned, as in the game's Combat Analysis (Wind Spirit: Malicious Whirlwind).
- On a training dummy, damage over time after your last hit counts, as in the game's Combat Analysis. A2Tools DPS Meter leaves it out.
- The Game record tab compares each skill's damage, hits, critical hits, Perfect, Double and Back Attack with the game's.
- **Damage received** in Details: the damage a player took and the hits on them, in the game's own words, and a list of what hit them: each monster skill, the monster, damage, hits, and Block, Parry, Perfect Block, Endurance and Regeneration where any. The Game record tab sets the game's damage received beside the meter's. The meter's Heal and Tank tabs show each player's healing and damage taken.
- **Skill details on hover** in every mode, including the modes that show several targets.
- BOSS mode stays on the boss you and your party are fighting. Another boss takes its place once, over the last 10 seconds, it takes more than twice the damage from you and your party that the boss on screen takes, or when the boss on screen dies and you fight on. It no longer flips between two bosses or scarecrows several times a second.
- **Mode list.** The mode button opens a list of the five modes: pick one instead of clicking through them. The list can reach past the meter's edges, on the Wayland layer overlay too.
- **Details, History and the hover tooltip never wait on the meter,** and settings are saved in the background. Quitting saves your fights first.
- **Every UI string in all 8 languages.**
- DoT rows sit under their skill in Details in every language: they are matched by skill code, not by name.
- **Details on every target** shows one fight: its time counts a boss and its adds once and leaves out the gaps between pulls, as the meter does; its healing is the fight's, each tick once; every hit sits at its time in the fight on the DPS chart and the timeline. A fight with no healing shows no bars under HEAL. In BOSS it is the boss on the meter, and in TRAIN it counts your own time on the dummies, as the meter does. Its title names the target the meter follows, else the one that took the most damage, and the same one on every refresh.
- A player's class in Details and in saved fights is the class most of their hits are of, so a player who also uses a skill of another class keeps one class icon.
- A narrow Details window drops skill columns, party bar figures and damage received columns from the right end instead of cutting numbers, names or headers.
- **Display Time** in Settings: History, Details and the game record show the time of day on a 12-hour or a 24-hour clock, the system's clock until you pick one. A2Tools DPS Meter shows a 24-hour time in History and a 12-hour time in the Details title.
- Skill, boss and dungeon names change with the language in every open window, without reopening it.
- Damage and healing, per skill and in total, no longer stop or wrap around at about 2.1 billion.
- Targets you stopped fighting more than 30 seconds ago leave the meter's memory, unless the current mode still shows them. ALL without a time window keeps everything since the zone change.
- When packet capture cannot start on Linux, the meter says so in its own window, instead of the Windows prompt to download Npcap.
- **Report a problem** in Settings opens the right issue form with your version and system filled in (you see exactly what before it opens), and can prepare a packet log with every character name replaced, to attach without exposing other players.
- **Open log folder** in Settings opens the folder with `debug.log` and the packet logs, to attach to an issue here.
- **Notice tray** on the meter's top edge: an icon per kind of notice waiting, and a card that pulls up with the notices. Nothing opens over the game by itself.
- **Pet soul notices:** each soul a pet takes says how many souls it holds toward its next level and how many it needs to the top, one line per pet.
- **Power Shard warning:** a notice when your hits stop using Power Shards after using them, or when you ran out.
- **Boss countdowns:** 30, 15 and 5 minutes before an Abyss Boss, and before Gartua and the commanders once you have opened the map (the game sends their times only then); 5 minutes before a Spacetime Rift.

### Linux desktop
- **Qt Quick windows.** Daevalog 2.0 draws its windows with Qt 6 instead of WebKitGTK.
- **Tray icon,** with "Start in the tray" and "Keep out of the taskbar" (X11 only: Wayland lets no app ask). The tray menu shows, hides, locks and unlocks the meter, and opens Settings and History. Every control also stays in the meter's own window, so the meter works on a desktop without a tray.
- **Click-through lock** on X11, XWayland and native Wayland. A locked meter keeps its ⋯ button clickable, so you can unlock it.
- **Lock hotkey** through the desktop's global shortcuts (the GlobalShortcuts portal), on desktops that offer them.
- **Display backend per desktop.** At start the meter picks X11, XWayland or native Wayland to fit the desktop: on GNOME it runs through XWayland, where it can stay on top. A `QT_QPA_PLATFORM` you set yourself still wins.
- **Wayland layer overlay** on KDE Plasma, Hyprland and Sway, on by default there: the meter stays above a fullscreen game. It needs layer-shell-qt for Qt 6. Settings turns it off.
- On KDE Plasma under X11 the meter is a window KWin does not manage, so it stays above a fullscreen game too.
- Menus and dropdowns are drawn by the meter, so they look the same on every desktop, and can reach past the window that opened them.
- Details, History and Settings open where you last left them, at that size (on Wayland, the size only).
- One meter at a time: a second start shows the running meter.
- **Report a problem** names the Qt version and the display backend the meter picked at start.

### Parsing
- Packets are framed by the real length rule: payload plus 4.
- Each connection to the game server is read on its own. A TLS connection is left out whole.
- A fight keeps the dungeon it was fought in. A map load into the open world ends the dungeon. The dungeon a party queues for begins with the load into it, not when the party roster names it.
- A fight in a sealed, quest, daily, Ascension or Ascension Trial dungeon, in Nightmare, or at a Daeva Hunter recon site is filed under that dungeon, and a fight in the Abyss under its zone. A party's queued dungeon applies only at the load into it.
- A dungeon's difficulty is shown in the game's own words and language, taken from the game's dungeon table, only where the game gives one, and never repeating the name.
- Spirits are linked to their owners by the game's link records and the spawn caster field. Owners are not guessed by power scalar. An effect the game never spawned or linked goes to your party's one member of its class when no other player of that class fought where it hit; otherwise all such effects share one row, "Unattributed summons and effects".
- Fight slices keep skill ids that look like short text. They are no longer blanked out as names.
- The party roster is read to its end. Members after an empty slot keep their item level and combat power up to date.
- Each record is read within its own packet, never on into the next one. Records read across a packet's end made false monsters and players, and lost some hits and spirit links.

### Fights and identity
- The backend alone decides which player is the local player, from the game's own record of you.
- Every fight segment is saved before any reset. Only fights that you or your party fought are saved, and only once the meter knows who you are: the game says so on login and on every zone change, so a meter started in the middle of a session saves nothing until the next one.
- A fight cleared by a zone change keeps the ids, names and spirit links it had.
- A party roster never names a player by class in the open world, where each player's own spawn names them. Strangers fighting nearby are not taken for your party.
- A fight's healing is the healing done during that fight, in live Details and in the saved fight alike.
- MP and other resource restores are not counted as healing or damage. A mob healing itself is not counted as healing.
- A monster's summon is never taken for the summon of the player it attacks, so its damage on that player is not counted as healing.
- A fight's healing counts only the people in it: you, your party and whoever hit its target. Players healing nearby are not counted.
- A fight saved when the meter closes is never overwritten by an older auto-save.
- Damage done to a player counts on no damage row in any mode, and a player is never a fight's target, even one the meter has not yet seen use a class skill.
- A player's attack on another player is never counted as healing. Which skills attack comes from the game's skill table.
- A player who attacks you or your party, or whom you or your party attack, is left out with all their damage, on monsters too: no row in any mode, nothing in Details, and nothing in a fight saved from then on. This lasts until you leave the map.

### Privacy
- The sign-in and upload paths are hardened.
- The data folder, the files the meter writes in it, and its `history` and `slices` folders are readable by your user only.
- An upload is always your own choice. No upload option is turned on for you.
- Packet capture uses `cap_net_raw` only, and only this machine's own traffic (no promiscuous mode).
- Packet capture runs in a small helper program, `daevalog-capture`. Only the helper holds `cap_net_raw`, and it gives it up once the network devices are open. The meter itself runs with no capability.
- The capture helper passes on only your own connections; other users' traffic and system services are never read through it.
- Packet capture takes TCP only, and only the game server's port once the meter has found it.
- Fight slices replace a character name wherever a record holds one, one-letter names included.
- The font (Pretendard) and the skill icons come with the meter. No window loads anything from the internet.
- Links open only https addresses and the meter's own folders.
- Screenshots go only to the folder you chose in the meter, or the default folder, and never over an existing file.
- Signing in tells a2tools.app the system (Linux or Windows), not your computer's name.
- **Hide other players' names** in Settings > Appearance. Other players show as their class and a number, on the meter, in Details, History and tooltips, so a screenshot names no one. A player keeps one number for the whole fight. You keep your own name. Saved fights and uploads are not changed.

### Left out
- Guessing who an unnamed actor is from its class, except for the one party case above.
- The one-time popups that offer Discord activity and sign-in with automatic upload.
- The update check and the updater.
- Discord activity and the Discord button in Settings.
- The supporter roster (gold names for A2Tools donors), which downloaded a list from a2tools.app every few hours.

Fixes that suit A2Tools DPS Meter are offered to that project as pull requests.

## For developers

`docs/ARCHITECTURE.md` describes the design. `CONTRIBUTING.md` describes how to test a change.

## License and credits

Daevalog DPS Meter is licensed under the GNU Affero General Public License, version 3 only. See [LICENSE](LICENSE), which also lists the few parts under other licences.

Daevalog started as a fork of a fork:

- [AION2-DPS-Meter](https://github.com/TK-open-public/Aion2-Dps-Meter), Copyright (c) 2026 TK-open-public, under the MIT License, is the original meter. The way Daevalog reads the game's packets descends from it.
- [A2Tools DPS Meter](https://github.com/taengu/A2Tools-DPS-Meter), Copyright (c) 2026 taengu, ported it to Rust and Tauri and built on it. Daevalog started as a fork of A2Tools DPS Meter v2.0.44 on 2026-10-02.
- Daevalog 2.0 is Seralth's own code, written to replace the forked code.

AION 2 is a trademark of NCSOFT. Daevalog DPS Meter is not affiliated with or endorsed by NCSOFT or A2Tools (see [Use at your own risk](#use-at-your-own-risk)).

The community links of the A2Tools project are in the [A2Tools DPS Meter repository](https://github.com/taengu/A2Tools-DPS-Meter).

### Icons

The boss and Spacetime Rift notices use two [Lucide](https://lucide.dev) icons, "swords" and "orbit", and five gear slot icons are Lucide's "sword", "shield", "shirt", "hand" and "hexagon", under the ISC License (notice in [LICENSE](LICENSE)).

### Fonts

The meter comes with the Pretendard font, version 1.3.9, under the SIL Open Font License, version 1.1: [public/vendor/fonts/pretendard/LICENSE](public/vendor/fonts/pretendard/LICENSE). `public/vendor/fonts/pretendard/files/PretendardVariable.ttf` is the file from [Pretendard's own release](https://github.com/orioncactus/pretendard/releases/tag/v1.3.9), unmodified. It is a TrueType file, not WOFF2, because Qt built without WOFF2 support (Qt on Windows among them) cannot load WOFF2.
