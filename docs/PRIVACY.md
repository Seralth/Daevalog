# Privacy: what Daevalog sends and keeps

Daevalog DPS Meter began as a fork of A2Tools DPS Meter and is now a separate program. The only site Daevalog talks to is a2tools.app. The A2Tools developer runs a2tools.app; Daevalog does not. Daevalog cannot see inside a2tools.app, so every statement here about what a2tools.app does is what the A2Tools project says about it.

Each statement about the meter can be checked in this repository's source. The statements about Evidence Slices and packet logs can also be checked on your own files with two tools: **See upload data** in Battle History, and the `a2t-inspect` program. See [Check it yourself](#check-it-yourself).

## In short

- The meter sends nothing about your fights unless you upload one. An upload happens when you press **Upload** in Battle History, or when a boss fight ends while **Upload boss fights automatically** is on. That switch is off until you turn it on.
- An upload holds one fight's Evidence Slice plus a list of player names for a2tools.app to display. In the slice are only those packet types the damage parser needs, and every character name in it is a code. No damage number is in the upload: a2tools.app computes the numbers itself from the slice.
- Packet logs and `debug.log` never leave your computer, and both are off until you turn them on.
- Apart from the sign-in, the account check and uploads, all listed below, the meter makes no network request. It never checks for updates and never sends usage data.

## Network requests

The meter talks to one host, `a2tools.app`, and only for your account and your uploads. This table is every request the meter makes.

| When | Request | Content |
|---|---|---|
| You press **Sign in** | `POST https://a2tools.app/api/device/code` | `{"client":"dps-meter","device_label":"Daevalog DPS Meter on Linux"}`, or `on Windows`. The label names the operating system only, never the computer. |
| While you approve the sign-in in your browser | `POST https://a2tools.app/api/device/token` | `{"device_code":"…"}`, the code a2tools.app gave out at the start. The meter asks again at the pace a2tools.app sets (between 1 and 60 seconds, 5 seconds slower each time a2tools.app asks for that), until you approve, until you decline, or until the code expires. |
| The Account page of Settings opens, a sign-in completes, or you use **Reset everything** in Settings | `GET https://a2tools.app/api/me` | Your sign-in token in an `Authorization: Bearer` header. Sent only when a token is stored and the desktop keyring hands it over. |
| An upload, by hand or automatic | `POST https://a2tools.app/api/logs` | Your sign-in token in an `Authorization: Bearer` header, and the body described in [The upload body](#the-upload-body). |

Every request uses https. The client refuses plain http before it opens a connection. Every request carries the user agent `Daevalog/<version>`, for example `Daevalog/2.0-r1`.

Some links in the meter open in your web browser: the a2tools.app approval page, a fight's log page, the a2tools.app privacy and contact pages, and the GitHub form of **Report a problem**. The browser loads those pages; the meter itself sends nothing for them.

## Your a2tools.app account

**Sign in** under **Settings > Account** opens the a2tools.app approval page in your browser and shows a code. After you approve, the meter receives a sign-in token. The meter stores the token in the desktop keyring (Secret Service, for example GNOME Keyring or KWallet) on Linux, and in Windows Credential Manager on Windows. The file `credentials.dat` in the data folder holds only the name of that keyring entry, never the token. With no working keyring the sign-in fails; the token is never written to a plain file.

**Sign out** deletes the token and `credentials.dat` on this computer. Signing out sends nothing to a2tools.app.

Without a sign-in the meter makes none of the requests above.

## Uploads

**By hand.** **Upload** on a fight in Battle History sends that one fight. **Upload again** sends it once more, for example after you deleted its log on a2tools.app. Battle History offers neither button on a training dummy fight or on a fight that is still going.

**Automatically.** **Settings > Account > Upload boss fights automatically** is off until you turn it on. While it is on and you are signed in, the meter uploads each boss fight once the fight is over, never while it is still going. Only a fight with a kept slice goes up on its own. Training dummy fights are never uploaded automatically. A fight that already has a link, or that the meter already tried, does not go up again on its own; a failed try is retried as below, also after a restart of the meter.

When an automatic upload fails because a2tools.app cannot be reached or answers with an error that may pass (too many requests, a server error), the meter tries again 1, 2, 5, 15, 30 and 60 minutes later, then gives up on that fight. Any other error ends the tries at once. While the desktop keyring is locked, the meter sends nothing and looks again every 5 minutes. Turning the switch off stops every further automatic upload and retry. Logs already on a2tools.app stay there; you manage them on a2tools.app.

## The upload body

An upload sends one line of JSON. The body holds exactly these fields:

| Field | Content |
|---|---|
| `appVersion` and `clientVersion` | The meter's version, the same in both, for example `2.0 r1`. |
| `client` | `daevalog` |
| `fightStartMs` | The start of the fight, in milliseconds since 1970. |
| `names` | Each player's actor id in the saved fight, with the name the saved fight holds: your own name in full, every other player's name masked (see [Battle History](#battle-history)). |
| `slice` | The fight's Evidence Slice, gzipped, then base64. |
| `uiLanguage` | The meter's language setting, for example `en` or `ko`. |
| `uploaderActorId` | Your actor id in the fight, or `null` when the meter did not learn it. |
| `utcOffsetMinutes` | Your computer's offset from UTC, in whole minutes. |

The time zone offset and the language are there because Korean and Taiwanese servers use the same server ids; a2tools.app tells them apart with these two fields. The body holds no damage numbers. The slice comes from `slices/` in the data folder. A fight without a kept slice gets one cut from the packet logs in the data folder, if they cover the fight; that slice is sent and not kept.

### The Evidence Slice

A slice is not a packet log. The meter builds the slice from the game's traffic around one fight and keeps only what a replay of that fight needs.

**Packet types.** A slice keeps 14 packet types, by an allowlist: `KEPT_OPCODES` in `src-tauri/src/capture/fight_tape/mod.rs`. Each entry has a name there: damage, damage or heal over time, HP and MP, summon or loot owner, zone change, map load, party scope, spawn, death, two older spawn forms, player spawn, your own character record, and the party roster. Every other packet type is dropped, chat included.

**Time window.** Damage, over-time and HP packets are kept from 1 minute before the fight starts until 15 seconds after it ends. The other allowlisted packets are also kept from up to 31 minutes before the start. Those are state packets, such as the boss's spawn, the party roster and the zone. They often arrive when you enter the room, minutes before the first hit, and without them the fight would have no boss.

**Pieces of other packets.** The game sometimes puts a damage record or a spawn record inside a packet type that is not on the list. The meter drops that packet but copies the record out of it:

- during the fight window, each damage record, from its `04 38` marker up to 160 bytes on; records closer together than that are copied as one piece;
- a spawn record, up to its own stated length (at most 4,096 bytes), or 512 bytes when its length does not read;
- during the fight window, inside a bundle, the few bytes that tie a cast to its skill.

These pieces are bytes from packets the allowlist does not name. Every piece goes through the same name blinding and the same leak check as the rest of the slice.

**Names.** The meter replaces names in three passes over every kept packet and piece:

1. every place where a record the parser reads holds a character name, at any length, one letter included;
2. every name the meter knows, wherever its bytes appear, for names of 2 bytes or more;
3. every stretch that looks like a name (a length byte, then 2 to 40 bytes of text), so names the meter never learned, such as passers-by, are replaced too.

Each name becomes a code of the same length in bytes. The code is made from a SHA-256 hash of the name, or of the player's party roster id when the meter knows one.

**Leak check.** Before the meter keeps or sends a slice, it opens every bundle in it and searches for every name it knows of 2 bytes or more, and checks every name field for every known name of any length. If it finds a name, the meter throws that slice away, and the slice is never kept or sent (`find_leak` in `src-tauri/src/capture/fight_tape/mod.rs`).

**Roster ids.** The party roster packet is in the slice, and it holds a server-assigned id for each member. The slice's own table of codes would also pair each code with that id; the meter sets every id in that table to 0 before it saves or sends a slice.

The tests in this repository build their packets by hand from the field layout. Tests that run the blinder over real captures check that no name of 3 bytes or more that the meter resolved, and no length-prefixed text of 6 bytes or more from the capture, survives in the slice. Those captures hold other players' names, so those tests stay outside this repository.

### What a2tools.app does with an upload

These statements come from the A2Tools project; Daevalog cannot check them.

- a2tools.app runs its own parser, A2Tools DPS Meter's, over the slice and publishes the numbers it works out. The upload holds no number from the meter's own count, so a changed meter cannot simply report more damage.
- a2tools.app masks the names in `names` again on its own, so a changed meter cannot publish another player's full name.
- a2tools.app keeps each uploaded slice, to compute the log again when needed, and never hands the slice out. Published logs leave the roster ids out.
- Each log is public or private: you set a default, and you can change it per log. a2tools.app lists no private log, yet a private log opens for anyone who has its link.
- Class statistics count every log, public or private, as numbers without names.

## What the meter keeps on your computer

The meter writes into one data folder:

- Linux: `~/.local/share/daevalog-dps-meter/`, or `$XDG_DATA_HOME/daevalog-dps-meter/` when `XDG_DATA_HOME` is set.
- Windows: `%APPDATA%\daevalog-dps-meter\`.

On Linux the data folder is `0700`, so only your user can open anything inside it. The folders the meter makes in it (`characters/`, `history/`, `slices/`, `share-preview/`) are `0700`, and the meter creates its files `0600`. On Windows the folder and its files take the access of `%APPDATA%`, which belongs to your user alone.

| File or folder | Content |
|---|---|
| `settings.json` | Your settings, with your character's name as the game last gave it and the network device you picked, if any. |
| `credentials.dat` | Only while you are signed in: the name of the keyring entry that holds the token, not the token. |
| `history/` | Battle History: one file per saved fight. See [Battle History](#battle-history). |
| `slices/` | Each saved fight's Evidence Slice and upload state. See [Evidence Slices on disk](#evidence-slices-on-disk). |
| `characters/` | One file per character you play: its name, server, class, level, Item Level, Combat Power, gear, stat sheet, skill levels, Specialty picks, Daevanion nodes and titles, as the game sent them. The name inside tells apart characters of the same class and level; the file itself is named with a code made from the server and the character's name. No other player's name is in it. Always written, never sent. |
| `debug.log` | Only while **Settings > Diagnostics > Enable debug logging** is on, which it is not by default. It stops growing at 5 MB. It holds the names of the players the meter met and the path of your home folder. |
| `packets_*.txt` | Only while **Enable packet logging** is on, which it is not by default. See [Packet logs](#packet-logs). |
| `report_debug.log` | Written by **Report a problem > Prepare debug log**: a copy of `debug.log` with every character name, your home folder and your user name replaced. |
| `report_packets_*.txt.gz` | Written by **Report a problem > Prepare log**: a compressed copy of one packet log with every character name replaced. Chat messages can stay in it. |
| `share-preview/` | Written by **See upload data**: the exact body an upload of that fight would send. It stays until you delete it. |
| `instance.lock`, and `instance.socket` on Linux | They let only one meter run: a second start asks the running meter to show itself, then quits. |

On its first start, Daevalog 2.0 takes over the settings of the 1.x meter and copies its saved fights, when this computer has the 1.x data folder (`com.daevalog.dps-meter`). The 1.x folder stays as it is. The sign-in does not come over.

A screenshot of the Details window goes to the clipboard. A file is written only while **Save screenshots to folder** is on in the Details window, into `Pictures/Daevalog DPS Meter` or a folder you choose, and when you start the meter with `--screenshot FILE`.

To find the game, the meter reads the list of running processes. To compare fights with the game's own Combat Analysis records, it reads the record files the game saves in its data folder (on Linux, inside the game's Proton prefix). It never writes to the game's folders.

### Battle History

The meter saves a fight against a boss or a training dummy that you or your party fought. It saves every 30 seconds while the fight goes on, and once more when the meter closes. A saved fight holds the damage, the skills and their timings, and a summary per player. For a party member it also holds the roster id, server, level, item level and combat power that the party list gave. The meter keeps the newest 500 saved fights and deletes older ones.

The meter masks every other player's name when it saves the fight, not when it shows it. `mask_name` in `src-tauri/src/entity/fight_record.rs` does this. A one-letter name stays as it is. A two-letter name keeps its first letter. A three-letter name keeps its first and last letters. A longer name keeps its first two letters and its last letter. One to four `*` stand between. A seven-letter name becomes, for example, `Ta****x`. Your own name is saved in full.

### Recent traffic in memory

The meter holds the game traffic it has just read in memory, so a saved fight gets its slice even with packet logging off. The meter keeps about 66 minutes of it, and never more than 96 MB (`src-tauri/src/share/ring.rs`). This is the traffic the meter reads to show the damage anyway. The meter never writes it to disk, and it is gone when the meter closes.

### Evidence Slices on disk

Each time the meter saves a fight while it runs, it also cuts the fight's slice from that memory. The slice goes to `slices/<fight>.a2es.gz`, beside a small `slices/<fight>.json`. The JSON file notes your actor id in the fight and, once you upload, the link, its visibility and how automatic uploads went. Deleting a fight in Battle History deletes its slice. The meter also deletes the slices of fights that have left Battle History.

### Packet logs

Packet logging is off until you turn on **Settings > Diagnostics > Enable packet logging**. Packet logs are a debugging tool. Read this section before you turn logging on.

Each line of a packet log is one payload as it arrived: the local time, the connection's ports, and the bytes in hex. A new file starts when the current one reaches 32 MB, after 10 minutes without game traffic, at midnight, and when you switch character. While logging is on, the meter keeps the newest 2 GB of packet logs and deletes older ones.

A packet log holds the whole stream the game server sends to your game client, not only combat. The game sends almost everything down that one connection. A packet log can therefore hold:

- chat, including public channels such as recruitment and trade;
- names of players who were never in your party, and legion names;
- account statistics, such as kill, death and assist counts;
- system announcements, and the other data the game sends to your client.

Before the meter has found the game's connection, it also logs payloads of other connections that carry the game's packet marker.

The meter never uploads a packet log, by hand or automatically. For a bug report, attach the copy that **Prepare log** writes, never the packet log itself. Delete packet logs when you no longer need them; the meter does not need them.

### The capture helper

On Linux the meter does not capture packets itself. A small helper program, `daevalog-capture`, holds the one permission capture needs (`cap_net_raw`); the meter holds none. The helper passes the meter only TCP payloads of connections that belong to the user who started it. It drops the traffic of other users and of system services without passing it on. The helper gives up its permission once it has opened the network devices. On Windows the meter captures with Npcap itself.

## Check it yourself

**See upload data.** In Battle History, **See upload data** on a fight writes the exact body an upload of that fight would send, `<fight>.upload-request.json`, into `share-preview/` in the data folder. It sends nothing. The file holds the body only: your sign-in token goes in a header and is not in the file. The slice in the file is the fight's kept slice from `slices/`, or one cut from the packet logs when the fight has none.

**a2t-inspect.** `a2t-inspect` prints what a slice holds. It is not in the packages. To build it, install Rust and run in a clone of this repository:

```
cd src-tauri
cargo build --release --bin a2t-inspect
```

The program is then `src-tauri/target/release/a2t-inspect`. Use it like this:

```
a2t-inspect fight.a2es.gz                  records, packet types and coded names
a2t-inspect fight.a2es.gz --strings        the same, then every readable run of text
a2t-inspect fight.a2es.gz --find NAME      the same, then a search for NAME; exits 1 when found
a2t-inspect --capture packets_….txt        packet types and readable text in a packet log
a2t-inspect --derive-capture packets_….txt every fight of a packet log, rebuilt from its slice
a2t-inspect --derive <data folder>         the saved fights, rebuilt from their kept slices
```

A slice keeps the game's LZ4-compressed bundles. A plain text search of the file finds nothing, whatever is inside. `a2t-inspect` opens every bundle before it searches.

`--derive-capture` and `--derive` rebuild each fight from its slice alone with this repository's parser (`src-tauri/src/slice_rebuild.rs`). They compare the result, by the boss, the total and every skill row, with the fight the whole packet log gives (`--derive-capture`) or with the saved fight (`--derive`). A slice that gives a different fight shows as `DIFFERENT`. This comparison is a check you can run on your own fights. It does not prove that every slice of every fight rebuilds the same. A log on a2tools.app can also differ slightly from what the meter showed during the fight, because a2tools.app works the fight out afterwards from the slice.

## What these protections do not cover

- **A masked name is not anonymous.** `Ta****x` hides part of a name. People who know whom you play with can often tell who it is.
- **The codes in a slice are not secret.** A code is made from a SHA-256 hash of a name or a roster id, with no secret added. Anyone who has the slice and a list of possible names can test which name a code stands for.
- **a2tools.app sees more than the public.** a2tools.app links each log to your account, keeps your own name in full, and keeps the slice with your party's roster ids in it.
- **The slice is built by rules, not read by a person.** The allowlist, the name passes and the leak check find what they are made to find. The pieces copied from other packets carry a limited number of bytes that the meter does not interpret.
- **Your party already saw your name.** Masking hides names from the public, not from the players you played with.
- **A shared packet log holds everything listed under [Packet logs](#packet-logs)**, other players' chat and names included, and whoever receives it can read all of it.
