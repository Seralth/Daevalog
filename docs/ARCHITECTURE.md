# Architecture

Daevalog DPS Meter reads the AION 2 server-to-client game traffic, rebuilds each fight, and shows damage per player. Daevalog DPS Meter does not read game memory and does not change game files.

Daevalog DPS Meter is a Tauri 2 application. The backend is Rust (`src-tauri/src`). The user interface is plain JavaScript, HTML and CSS (`public/src/js`, `index.html`), shown in the system webview.

Daevalog is its own meter. It started as a fork of A2Tools DPS Meter v2.0.44 (https://github.com/taengu/A2Tools-DPS-Meter). Uploads, sign-in and sent logs tell a2tools.app the version in `UPLOAD_COMPAT_VERSION` (`src-tauri/src/version.rs`); Settings shows the version from `src-tauri/build.rs`.

## Layers

The data moves through six layers, in this order. Each layer uses only the layer before it.

| Layer | Job | Main files |
|---|---|---|
| 1. Capture | Read packets from the network card with libpcap. On Linux a helper process (`daevalog-capture`) holds the capture permission and pipes TCP payloads to the meter. Find the game connection and lock onto its port. | `capture-helper/` (libpcap, the helper, its pipe), `capture/live.rs`, `capture/helper_process.rs`, `capture/pcap_capturer.rs`, `combat/capture_dispatcher.rs`, `capture/combat_port_detector.rs` |
| 2. Stream | Join TCP payloads into one byte stream per connection. | `capture/stream_assembler.rs`, `capture/packet_accumulator.rs` |
| 3. Framing | Cut the byte stream into game packets. Open compressed bundles. | `capture/framing.rs` |
| 4. Parsing | Read each game packet: damage, damage over time (DoT), heals, spawns, names, party roster, zone change, map load (which map a load enters, to tell an instance from the open world). | `capture/stream_processor/`, `entity/damage_packet.rs` |
| 5. Storage | Keep the fight data: damage per target and per actor, names, summon owners, the local player. | `combat/data_storage/`, `entity/summon_resolver.rs` |
| 6. Calculation | Choose the targets for the meter mode, add up damage per player, compute fight time, save fights. | `combat/dps_calculator/`, `history/fight_history.rs` |

The user interface asks layer 6 for a snapshot every 500 ms (`app/tasks.rs`, event `dps-update`) and draws the snapshot (`public/src/js/core.js`, `public/src/js/meter.js`).

## Other parts

| Part | Job | Main files |
|---|---|---|
| Application shell | Tauri commands, windows, settings, the save loop, startup and exit. | `app/` (commands in `app/commands/`, startup in `app/setup.rs`), `config/settings.rs` |
| Bridge | The JavaScript side of the Tauri commands. | `public/src/js/tauriBridge.js` |
| Fight history | Save each fight as JSON in `history/`. | `history/fight_history.rs`, `entity/fight_record.rs` |
| Sharing | Build the name-blinded packet slice of a fight and upload the slice to a2tools.app. | `capture/evidence_slice.rs`, `share/mod.rs`, `share/ring.rs` |
| Re-derivation | The code the log service runs on an uploaded slice. The same parser, compiled to WebAssembly. | `rederive.rs` |
| Game records | Read the game's own Damage Analyzer records (read only), match them to saved fights, and compare them skill by skill with a replay of each fight's slice. | `game_record/`, `public/src/js/gameRecord.js` |
| Account | Sign in to a2tools.app. The token is kept in the system keyring. | `account/mod.rs`, `account/secret.rs` |
| Platform | Code that differs per operating system: Linux, Windows, and a fallback. | `platform/` |
| Logging | `debug.log` and the optional raw packet log `packets_*.txt`. | `logging/logger.rs` |

## Windows

Every window loads the same `index.html` and runs the same `core.js`. The window kind is in `window.A2_VIEW`: `main` (the overlay), `details`, `details-<fight id>`, `settings`, `history`. Only the `main` window may change backend state at startup.

## Data folder

Linux: `~/.local/share/com.daevalog.dps-meter/`. The folder holds `settings.json`, `history/`, `slices/`, `debug.log` and, when packet logging is on, `packets_*.txt`. Packet logs contain chat and the names of other players. Do not share packet logs.

## Rules that must hold

Each rule below fixed a real fault. Do not break a rule without a test that shows the new behaviour is correct against the game.

1. **Packet length.** The length varint of a packet equals the payload length after the varint, plus 4. This is true for every varint width and for bundles. See `framing::frame_size`.
2. **Upload slices are version 1.** The a2tools.app log service reads only version 1 slices. `evidence_slice::encode` writes version 1 lengths (`downgrade_to_v1`). `decode` reads version 1 and version 2.
3. **Summon owners come from the game only.** A summon gets its owner from the spawn packet (parent key or caster field) or from a link record: skill code 1699xxxx from the summon to the owner, or 1677xxxx from the owner to the summon. The meter never guesses an owner from class or power scalar. Players share these values.
4. **Link records are not damage.** A 1699xxxx or 1677xxxx record links a summon. The record never adds damage and never creates a target.
5. **A reused entity id starts clean.** A new spawn on an entity id removes the old owner link and the old marks of that id.
6. **Only the backend decides who the local player is.** The source is the game's self record. A window never sends an id back to the backend, except when the user types an id in Settings. Party placeholder ids (90,000,000 and higher) are never the local player.
7. **No fight is lost.** Before any reset (zone change, party end, reset button, idle reset, exit), the meter saves every boss and training-dummy segment of 5 s or longer.
8. **Each fight segment is saved once per change.** A segment is keyed by target id and start time. A second run on the same target is a new segment.
9. **Training dummies.** On a training dummy, DoT ticks after the actor's last direct hit do not count. The game's own meter does the same. The dummy list survives a reset.
10. **Fight time is active time.** In modes with several targets, fight time is the union of the targets' active spans, not the longest span.

## Meter modes

| Mode | Targets | Fight time |
|---|---|---|
| TARGET | The enemy the local player or the local player's summons hit last. Nothing until the local player is known. | That target's span. |
| ALL | Every target, damage from every actor. Optional window: the last N minutes, or Off (since the zone change). | Union of active spans. |
| BOSS | The boss that the local player, the summons or the party hit. | That boss's span. |
| TRAIN | Training dummies that the local player or the summons hit. | Union of the local player's spans on those dummies. |
