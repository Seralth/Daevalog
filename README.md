# Daevalog DPS Meter

[![License](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](LICENSE)

Daevalog DPS Meter is a real-time damage meter overlay for AION 2. Daevalog DPS Meter is a fork of [A2Tools DPS Meter](https://github.com/taengu/A2Tools-DPS-Meter) by taengu, based on A2Tools DPS Meter v2.0.44.

Daevalog DPS Meter reads the game's network traffic. Daevalog DPS Meter does not read game memory and does not change game files.

## Status of this fork

- **Linux first.** Daevalog DPS Meter is developed and tested on Linux, with AION 2 running under Proton. The Windows code from A2Tools DPS Meter is still in the source, but Daevalog DPS Meter is not built, tested or supported on Windows.
- **Differences from A2Tools DPS Meter.** Daevalog DPS Meter may differ from A2Tools DPS Meter in features, behaviour and security handling.
- **No compatibility promise.** Compatibility with A2Tools DPS Meter is kept where practical: settings, saved fights, the upload format and the a2tools.app log service. No compatibility is promised. A future version may drop compatibility without warning or notice.
- **No releases yet.** No packages are published. Build from source, as described below.
- **No automatic updates.** Daevalog DPS Meter never updates itself. The update check of A2Tools DPS Meter is switched off.

## Reporting problems

Report problems with Daevalog DPS Meter in the issues of this repository. Do not report problems with Daevalog DPS Meter to the A2Tools DPS Meter project.

## Changes from A2Tools DPS Meter

- Packets are framed by the real length rule: payload plus 4.
- Spirits are linked to their owners by the game's link records and the spawn caster field. Owners are not guessed by power scalar or class.
- Every fight segment is saved before any reset. Only fights that the player or the party fought are saved.
- The backend alone decides which player is the local player.
- The sign-in, upload and webview paths are hardened.
- Windows behave correctly under KDE Plasma (KWin), and the overlay draws on WebKitGTK 2.54.

Fixes that suit A2Tools DPS Meter are offered to that project as issues and pull requests.

## Build from source

- Arch-based systems: `cd packaging/arch && makepkg -si`.
- Other distributions: follow the steps in `build()` in `packaging/arch/PKGBUILD`. Packet capture needs `cap_net_raw` and `cap_net_admin` on the binary. The Arch package sets both.

`docs/ARCHITECTURE.md` describes the design. `CONTRIBUTING.md` describes how to test a change.

## Uploads and a2tools.app

Daevalog DPS Meter can upload fights to the a2tools.app log service, which the A2Tools project runs. An account on a2tools.app is an A2Tools account. Daevalog DPS Meter is not affiliated with A2Tools or a2tools.app.

## Support A2Tools

Daevalog DPS Meter is built on the work of taengu and the A2Tools project. If Daevalog DPS Meter is useful to you, please consider supporting A2Tools:

- [Buy me a Coffee (Ko-fi)](https://ko-fi.com/hiddencube)
- [爱发电 (afdian)](https://afdian.com/a/hiddencube)
- [PayPal](https://www.paypal.me/taengoo)
- [Donate with crypto (NOWPayments)](https://nowpayments.io/donation/thehiddencube)

The full and current list of A2Tools donation options, including WeChat and wallet addresses, is in the [A2Tools DPS Meter README](https://github.com/taengu/A2Tools-DPS-Meter#support). Daevalog DPS Meter does not take donations on behalf of A2Tools.

## License and credits

Daevalog DPS Meter is licensed under the GNU General Public License, version 3. See [LICENSE](LICENSE).

- A2Tools DPS Meter: Copyright (c) 2026 taengu.
- Parts are derived from AION2-DPS-Meter, Copyright (c) 2026 TK-open-public, under the MIT License. The full MIT text is in [LICENSE](LICENSE).
- Modifications: Seralth, since 2026-10-02.

AION 2 is a trademark of NCSOFT. Daevalog DPS Meter is not affiliated with or endorsed by NCSOFT.

The README of A2Tools DPS Meter, with Windows instructions and the community links of the A2Tools project, is in the [A2Tools DPS Meter repository](https://github.com/taengu/A2Tools-DPS-Meter).
