//! The player list of the replay report (`A2_REPLAY_PLAYERS=1`): every player
//! met, with home server and class and where each came from, and every Item
//! Level and Combat Power a party, party finder or legion list stated.

use std::collections::{BTreeMap, HashMap};

use crate::capture::names::{exact_name, NAME_FIELD_BYTES};
use crate::capture::opcodes::{PLAYER_SPAWN, PLAYER_SPAWN_OLD};
use crate::capture::varint::read_varint;
use crate::combat::data_storage::DataStorage;
use crate::entity::job_class::JobClass;

/// Each player the replay met, for `A2_REPLAY_PLAYERS`, by name: a player
/// gets a new entity id on every zone load, and the name is what joins them.
#[derive(Default)]
pub(crate) struct Players {
    by_name: BTreeMap<String, PlayerSeen>,
    /// Item Level and Combat Power readings in capture order, each kept only
    /// when it differs from what the same list said last about that player:
    /// the party roster is sent again on every party change.
    gear: Vec<GearSeen>,
    last_gear: HashMap<(String, &'static str), Gear>,
}

#[derive(Default)]
struct PlayerSeen {
    /// The entity id they had last.
    id: i32,
    /// Class as each source gave it, with how often.
    own: Option<String>,
    roster: Votes<String>,
    spawn: Votes<String>,
    skills: Votes<String>,
    /// Home server: the roster's, then the player records', then the one a
    /// loot or summon record gave with the name.
    roster_server: Option<u16>,
    spawn_server: Votes<u16>,
    record_server: Option<u16>,
    first_ms: i64,
    last_ms: i64,
}

type Votes<T> = BTreeMap<T, u32>;

fn top<T: Clone + Ord>(votes: &Votes<T>) -> Option<T> {
    // Ties go to the first in order, so a replay prints the same every time.
    votes.iter().rev().max_by_key(|(_, n)| **n).map(|(v, _)| v.clone())
}

fn class_name(class: JobClass) -> String {
    format!("{class:?}")
}

/// The end of the fixed block that closes a player record, eight `ff` then
/// the constant the summon spawn parser also anchors on.
const PLAYER_RECORD_TAIL: [u8; 16] =
    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80, 0x75, 0xd5, 0x2a, 0xbb, 0x03, 0x00, 0x00];

/// Server ids the game's ServerName table has: 1001-1060, 1101-1160 and so
/// on to 2501-2560.
fn is_server_id(id: u16) -> bool {
    matches!(id / 100, 10..=15 | 20..=25) && (1..=60).contains(&(id % 100))
}

/// The home server in a player record, from its bytes after the name.
///
/// Matched against the party roster's server (78 of 78 records, cross-server
/// members on 2201, 2202 and 2204 included) and the loot and summon records'
/// (529 of 530 players) over 21 captures (2026-10-04 to 10-06). A player without
/// a legion has it 9 or 10 bytes before the closing block, then a byte and
/// `10` or `11`; a legion member has zeros there and the server in the
/// legion block, `00 00 <server u16> <len> <legion name>`. About one record
/// in fifteen has neither.
fn record_server(data: &[u8], after: usize) -> Option<u16> {
    let window = data.get(after..data.len().min(after + 512))?;
    let end = window.windows(PLAYER_RECORD_TAIL.len()).position(|w| w == PLAYER_RECORD_TAIL)?;
    let u16_at = |i: usize| u16::from_le_bytes([window[i], window[i + 1]]);
    for back in [9, 10] {
        if end >= back && is_server_id(u16_at(end - back)) && matches!(window[end - back + 3], 0x10 | 0x11) {
            return Some(u16_at(end - back));
        }
    }
    (0..end.saturating_sub(5))
        .filter(|&j| window[j] == 0 && window[j + 1] == 0 && is_server_id(u16_at(j + 2)))
        .filter(|&j| {
            let len = window[j + 4] as usize;
            let name = window.get(j + 5..j + 5 + len).filter(|_| (1..=40).contains(&len) && j + 5 + len <= end);
            name.and_then(|n| std::str::from_utf8(n).ok())
                .is_some_and(|n| n.chars().all(|c| c.is_alphanumeric() || c == ' '))
        })
        .last()
        .map(|j| u16_at(j + 2))
}

/// What a party, party finder or legion list states about one member.
#[derive(Clone, PartialEq)]
struct Gear {
    server: u16,
    class: JobClass,
    level: u32,
    item_level: u32,
    combat_power: u64,
}

/// The lists that state Item Level and Combat Power, by opcode. `01 97` is
/// probably the party finder: it lists rooms, each with a title, a dungeon id
/// and its members. `0b 97` and `1f 97` each carry one member of your party.
fn gear_list(op: &[u8]) -> Option<&'static str> {
    match op {
        [0x02, 0x97] => Some("party"),
        [0x0b, 0x97] => Some("party-0b97"),
        [0x1f, 0x97] => Some("party-1f97"),
        [0x01, 0x97] => Some("finder"),
        [0x05, 0x8a] => Some("legion"),
        _ => None,
    }
}

/// The account id both kinds of list carry: a character id, two zero bytes,
/// then the home server.
fn dbid_server(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at + 8)?;
    let server = u16::from_le_bytes([b[6], b[7]]);
    (b[4] == 0 && b[5] == 0 && is_server_id(server)).then_some(server)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// The name, class and level every member record carries in that order, and
/// where the bytes after them start.
fn member_head(data: &[u8], at: usize, class_first: bool) -> Option<(String, JobClass, u32, usize)> {
    let class_at = |o: usize| JobClass::from_roster_class(u32_at(data, o)?);
    let name_at = if class_first { at + 4 } else { at };
    let len = *data.get(name_at)? as usize;
    let name = exact_name(data.get(name_at + 1..name_at + 1 + len)?)?;
    let mut o = name_at + 1 + len;
    let class = if class_first {
        class_at(at)?
    } else {
        o += 4;
        class_at(o - 4)?
    };
    let level = u32_at(data, o).filter(|l| (1..=99).contains(l))?;
    Some((name, class, level, o + 4))
}

/// A member record of the party roster's layout (see `scan_party_roster`),
/// which `01 97`, `0b 97` and `1f 97` share: mask, slot, account id, name,
/// class, level, Item Level, then the server again within ten bytes and
/// Combat Power five bytes past it. Returns where the record ends.
fn party_member(data: &[u8], at: usize) -> Option<(String, Gear, usize)> {
    let server = dbid_server(data, at + 2)?;
    let (name, class, level, o) = member_head(data, at + 10, false)?;
    let item_level = u32_at(data, o).filter(|il| *il <= 10_000)?;
    let o = o + 4;
    let anchor = (o..=o + 10).find(|&i| data.get(i..i + 2) == Some(&server.to_le_bytes()[..]))?;
    let combat_power = u64_at(data, anchor + 5).filter(|cp| *cp <= 100_000_000)?;
    Some((name, Gear { server, class, level, item_level, combat_power }, anchor + 13))
}

/// A member record of the legion list (`05 8a`):
///
/// ```text
/// record id     u64
/// account id    u64       character id, 00 00, home server
/// class         u32
/// name          str
/// level         u32
/// unnamed       u8
/// unnamed       nothing, one byte, or a text with its length then a byte
/// last seen     u64       Unix ms
/// unnamed       u32       0, or a value that looks like a map id
/// item level    u32
/// combat power  u64
/// unnamed       u8        not after the last record
/// ```
///
/// Where the u32 before the Item Level is 0, the Item Level read 400 to 900
/// above the one the same player had in the lists before and after, Combat
/// Power the same (761 of the 762 readings far off the Combat Power line, over
/// seven captures of 2026-10-04 to 10-06): those give no reading. The last-seen time
/// anchors the rest; it must fall in the two years before the capture.
fn legion_member(data: &[u8], at: usize, now_ms: i64) -> Option<(String, Option<Gear>, usize)> {
    if u32_at(data, at + 4)? != 0 {
        return None;
    }
    let server = dbid_server(data, at + 8)?;
    let (name, class, level, o) = member_head(data, at + 16, true)?;
    let seen = (now_ms - 2 * 365 * 86_400_000)..=(now_ms + 86_400_000);
    let stamp = (o + 1..o + 64).find(|&i| u64_at(data, i).is_some_and(|t| seen.contains(&(t as i64))))?;
    let item_level = u32_at(data, stamp + 12).filter(|il| *il <= 10_000)?;
    let combat_power = u64_at(data, stamp + 16).filter(|cp| *cp <= 100_000_000)?;
    let gear = (u32_at(data, stamp + 8)? != 0).then_some(Gear { server, class, level, item_level, combat_power });
    Some((name, gear, stamp + 24))
}

/// A reading for `A2_REPLAY_PLAYERS`.
struct GearSeen {
    at: i64,
    list: &'static str,
    name: String,
    gear: Gear,
}

impl Players {
    fn seen(&mut self, name: &str, id: i32, at: i64) -> &mut PlayerSeen {
        let p = self
            .by_name
            .entry(name.to_string())
            .or_insert_with(|| PlayerSeen { first_ms: at, last_ms: at, ..Default::default() });
        if id != 0 {
            p.id = id;
        }
        p.first_ms = p.first_ms.min(at);
        p.last_ms = p.last_ms.max(at);
        p
    }

    /// Player records (`45 36`) anywhere in a packet, read as the parser's
    /// `scan_masked_identity` reads them. The u32 after the name is the class
    /// in the roster's encoding, as in your own record (`33 36`), which has
    /// your server in between: it matched the class skills used in 846 of
    /// 848 players over 21 captures, the roster in 23 of 23.
    pub fn scan_spawns(&mut self, data: &[u8], at: i64) {
        for i in 0..data.len().saturating_sub(8) {
            let op = [data[i], data[i + 1]];
            if op != PLAYER_SPAWN && op != PLAYER_SPAWN_OLD {
                continue;
            }
            let id = read_varint(data, i + 2);
            if id.length <= 0 || !(1..=9_999_999).contains(&id.value) {
                continue;
            }
            let mask2 = i + 2 + id.length as usize + 4;
            if mask2 + 1 >= data.len() || data[mask2] & 0x01 == 0 {
                continue;
            }
            let len = data[mask2 + 1] as usize;
            let after = mask2 + 2 + len;
            if !NAME_FIELD_BYTES.contains(&len) || after > data.len() {
                continue;
            }
            let Some(name) = exact_name(&data[mask2 + 2..after]) else { continue };
            let class = data
                .get(after..after + 4)
                .and_then(|b| JobClass::from_roster_class(u32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            let server = record_server(data, after);
            let p = self.seen(&name, id.value, at);
            if let Some(class) = class {
                *p.spawn.entry(class_name(class)).or_default() += 1;
            }
            if let Some(server) = server {
                *p.spawn_server.entry(server).or_default() += 1;
            }
        }
    }

    /// Item Level and Combat Power from a list packet, `<varint len> <opcode>`.
    /// Every record shape in the body is read, so a record that does not read
    /// costs only itself; the party finder holds several rooms.
    pub fn scan_gear(&mut self, packet: &[u8], at: i64) {
        let len = read_varint(packet, 0);
        if len.length <= 0 {
            return;
        }
        let o = len.length as usize;
        let Some(list) = packet.get(o..o + 2).and_then(gear_list) else { return };
        let body = &packet[o + 2..];
        let mut i = 0;
        while i < body.len() {
            let read = if list == "legion" {
                legion_member(body, i, at)
            } else {
                party_member(body, i).map(|(name, gear, end)| (name, Some(gear), end))
            };
            let Some((name, gear, end)) = read else {
                i += 1;
                continue;
            };
            i = end;
            let Some(gear) = gear else { continue };
            if self.last_gear.get(&(name.clone(), list)) != Some(&gear) {
                self.last_gear.insert((name.clone(), list), gear.clone());
                self.gear.push(GearSeen { at, list, name, gear });
            }
        }
    }

    /// A class skill used by `actor` while it carried a name.
    pub fn hit(&mut self, storage: &DataStorage, actor: i32, skill: i32, at: i64) {
        let Some(class) = JobClass::convert_from_skill(skill) else { return };
        let Some(name) = storage.get_nickname(actor) else { return };
        *self.seen(&name, actor, at).skills.entry(class_name(class)).or_default() += 1;
    }

    pub fn roster_and_self(&mut self, storage: &DataStorage, at: i64) {
        for (name, m) in storage.get_party_members() {
            let p = self.seen(&name, 0, at);
            if m.server_id != 0 {
                p.roster_server = Some(m.server_id);
            }
            if let Some(class) = m.job {
                *p.roster.entry(class_name(class)).or_default() += 1;
            }
        }
        let me = storage.local_profile();
        if let (Some(name), Some(id)) = (me.name, storage.local_player_id()) {
            let p = self.seen(&name, id as i32, at);
            p.own = me.class.map(class_name).or(p.own.take());
        }
    }

    /// One line per player. The class is what the game stated (your own
    /// record, then the roster, then the player records), else the class
    /// skills used. Each source follows with its counts, `-` where it said
    /// nothing; `servers` gives the roster's, the player records' and the
    /// loot or summon records'. Players only a loot or summon record named
    /// are dated by the whole capture. Then one `gear` line per reading.
    pub fn lines(mut self, storage: &DataStorage, first_ms: i64, last_ms: i64) -> Vec<String> {
        for (name, server) in storage.player_servers() {
            let p = self.by_name.entry(name).or_insert_with_key(|name| PlayerSeen {
                id: storage.find_id_by_nickname(name).unwrap_or(0),
                first_ms,
                last_ms,
                ..Default::default()
            });
            p.record_server = Some(server);
        }
        fn votes<T: std::fmt::Display>(v: &Votes<T>) -> String {
            let all: Vec<String> = v.iter().map(|(c, n)| format!("{c}={n}")).collect();
            if all.is_empty() { "-".to_string() } else { all.join(",") }
        }
        let or_dash = |s: Option<u16>| s.map_or("-".to_string(), |s| s.to_string());
        self.by_name
            .into_iter()
            .map(|(name, p)| {
                let class = p.own.clone().or(top(&p.roster)).or(top(&p.spawn)).or(top(&p.skills));
                let server = p.roster_server.or(top(&p.spawn_server)).or(p.record_server);
                format!(
                    "player {} server {} class {} self {} roster {} spawn {} skills {} servers {}/{}/{} seen {}..{} name {name}",
                    p.id,
                    server.map_or("unknown".to_string(), |s| s.to_string()),
                    class.as_deref().unwrap_or("unknown"),
                    p.own.as_deref().unwrap_or("-"),
                    votes(&p.roster),
                    votes(&p.spawn),
                    votes(&p.skills),
                    or_dash(p.roster_server),
                    votes(&p.spawn_server),
                    or_dash(p.record_server),
                    p.first_ms,
                    p.last_ms,
                )
            })
            .chain(self.gear.into_iter().map(|g| {
                format!(
                    "gear {} {} server {} class {} level {} item {} power {} name {}",
                    g.at,
                    g.list,
                    g.gear.server,
                    class_name(g.gear.class),
                    g.gear.level,
                    g.gear.item_level,
                    g.gear.combat_power,
                    g.name,
                )
            }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::replay_report::{run, Options};
    use crate::capture::framing;

    /// Three player records in the two layouts seen in captures: a player with no
    /// legion (server before the closing block), a legion member (server in the
    /// legion block), and one whose class and server do not read.
    #[test]
    fn players_lists_each_player_with_server_and_class() {
        let frame = |body: Vec<u8>| {
            let mut out = vec![framing::length_value(body.len()) as u8];
            out.extend(body);
            out
        };
        let record = |id: [u8; 2], name: &str, class: u32, tail: &[u8]| {
            let mut b = vec![0x45, 0x36, id[0], id[1], 0, 0, 0, 0, 0x01, name.len() as u8];
            b.extend(name.as_bytes());
            b.extend(class.to_le_bytes());
            b.extend([0x01, 0x02, 0x00, 0x00]);
            b.extend(tail);
            b.extend([0xff; 8]);
            b.extend([0x80, 0x75, 0xd5, 0x2a, 0xbb, 0x03, 0x00, 0x00, 0x00, 0x00]);
            frame(b)
        };
        // 1201 = `b1 04`; class 30 is a Cleric, 6 a Gladiator.
        let plain = record([0xa9, 0x46], "Tester", 30, &[0xb1, 0x04, 0x0b, 0x11, 0x0e, 0xa1, 0xf5, 0x03, 0x07]);
        let legion = record(
            [0xaa, 0x46],
            "Other",
            6,
            &[0, 0, 0, 0, 0, 0, 0xb2, 0x04, 0x05, b'G', b'u', b'i', b'l', b'd', 0x01, 0, 0, 0, 0x0a, 0x11, 0x22, 0x81, 0xae, 0x5e, 0x09],
        );
        let blank = record([0xab, 0x46], "Third", 0, &[0x00, 0x00, 0x0b, 0x11, 0x0e, 0xa1, 0xf5, 0x03, 0x07]);
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let text = [
            format!("2026-10-06T12:00:00.000-07:00|Client:50000:13328|{}", hex(&plain)),
            format!("2026-10-06T12:00:01.000-07:00|Client:50000:13328|{}", hex(&legion)),
            format!("2026-10-06T12:00:02.000-07:00|Client:50000:13328|{}", hex(&blank)),
        ]
        .join("\n");
        let mut lines = Vec::new();
        run(&text, Options { players: true, ..Default::default() }, &mut |l| lines.push(l));
        let players: Vec<&String> = lines.iter().filter(|l| l.starts_with("player ")).collect();
        let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis();
        let (t0, t1, t2) = (at("2026-10-06T12:00:00-07:00"), at("2026-10-06T12:00:01-07:00"), at("2026-10-06T12:00:02-07:00"));
        assert_eq!(
            players,
            [
                &format!("player 9002 server 1202 class Gladiator self - roster - spawn Gladiator=1 skills - servers -/1202=1/- seen {t1}..{t1} name Other"),
                &format!("player 9001 server 1201 class Cleric self - roster - spawn Cleric=1 skills - servers -/1201=1/- seen {t0}..{t0} name Tester"),
                &format!("player 9003 server unknown class unknown self - roster - spawn - skills - servers -/-/- seen {t2}..{t2} name Third"),
            ]
        );
        // Without the option, no player lines.
        let mut lines = Vec::new();
        run(&text, Options::default(), &mut |l| lines.push(l));
        assert!(!lines.iter().any(|l| l.starts_with("player ")));
    }

    /// One reading per list record, in capture order: a party member record (the
    /// same one twice gives one reading), two party finder members around an
    /// empty slot, and legion members with a greeting, with one odd byte, with
    /// the u32 before the Item Level at 0 (no reading), and last without its
    /// closing byte.
    #[test]
    fn players_lists_item_level_and_combat_power_readings() {
        let frame = |body: Vec<u8>| {
            let mut len = framing::length_value(body.len());
            let mut out = Vec::new();
            while len >= 0x80 {
                out.push(len as u8 | 0x80);
                len >>= 7;
            }
            out.push(len as u8);
            out.extend(body);
            out
        };
        let account = |char_id: u32, server: u16| {
            let mut b = char_id.to_le_bytes().to_vec();
            b.extend([0, 0]);
            b.extend(server.to_le_bytes());
            b
        };
        let member = |slot: u8, char_id: u32, server: u16, name: &str, class: u32, level: u32, il: u32, wide: bool, cp: u64| {
            let mut b = vec![0x0c, slot];
            b.extend(account(char_id, server));
            b.push(name.len() as u8);
            b.extend(name.as_bytes());
            for v in [class, level, il] {
                b.extend(v.to_le_bytes());
            }
            if wide {
                b.push(0x01);
            }
            b.extend(server.to_le_bytes());
            b.extend(server.to_le_bytes());
            b.push(0x04);
            b.extend(cp.to_le_bytes());
            b.extend([0x01, 0x01]);
            b
        };
        let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis();
        let t = ["2026-10-06T12:00:00-07:00", "2026-10-06T12:00:01-07:00", "2026-10-06T12:00:02-07:00", "2026-10-06T12:00:03-07:00"];
        let seen = (at(t[3]) - 3_600_000) as u64;
        let legion = |id: u64, char_id: u32, name: &str, class: u32, odd: &[u8], map: u32, il: u32, cp: u64, last: bool| {
            let mut b = id.to_le_bytes().to_vec();
            b.extend(account(char_id, 1201));
            b.extend(class.to_le_bytes());
            b.push(name.len() as u8);
            b.extend(name.as_bytes());
            b.extend(45u32.to_le_bytes());
            b.push(0x02);
            b.extend(odd);
            b.extend(seen.to_le_bytes());
            b.extend(map.to_le_bytes());
            b.extend(il.to_le_bytes());
            b.extend(cp.to_le_bytes());
            if !last {
                b.push(0x00);
            }
            b
        };

        // 2201 = `99 08`; class 8 is a Gladiator, 30 a Cleric, 22 an Elementalist.
        let mut join = vec![0x0b, 0x97];
        join.extend(member(4, 0x45817, 2201, "Tester", 8, 22, 353, true, 21_250));
        let mut finder = vec![0x01, 0x97, 0x00, 0x00, 0xff, 0x27, 0x09, 0x00, 0x05, b'R', b'o', b'o', b'm', b'.'];
        finder.extend(member(1, 0x3d60d, 2201, "Other", 30, 45, 1913, false, 81_398));
        finder.extend([0x00, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0x00]);
        finder.extend(member(3, 0x50e09, 1201, "Third", 22, 45, 861, true, 45_961));
        let mut list = vec![0x05, 0x8a, 0x00, 0x00, 0x04, 0x00];
        let greeting: Vec<u8> = [&[0x05u8][..], b"Hello"].concat();
        list.extend(legion(0x808cf, 0x4b002, "Fourth", 9, &greeting, 2040, 1403, 67_893, false));
        list.extend(legion(0x80c52, 0x4bd2e, "Fifth", 29, &[0x42], 2010, 843, 37_476, false));
        list.extend(legion(0x80d00, 0x4c000, "Sixth", 13, &[], 0, 2040, 67_000, false));
        list.extend(legion(0x80e00, 0x4c100, "Seventh", 33, &[], 2064, 2116, 95_541, true));
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let text = [(t[0], &join), (t[1], &join), (t[2], &finder), (t[3], &list)]
            .iter()
            .map(|(ts, body)| format!("{}|Client:50000:13328|{}", ts.replace("-07:00", ".000-07:00"), hex(&frame((*body).clone()))))
            .collect::<Vec<_>>()
            .join("\n");
        let mut lines = Vec::new();
        run(&text, Options { players: true, ..Default::default() }, &mut |l| lines.push(l));
        let gear: Vec<&String> = lines.iter().filter(|l| l.starts_with("gear ")).collect();
        let (t0, t2, t3) = (at(t[0]), at(t[2]), at(t[3]));
        assert_eq!(
            gear,
            [
                &format!("gear {t0} party-0b97 server 2201 class Gladiator level 22 item 353 power 21250 name Tester"),
                &format!("gear {t2} finder server 2201 class Cleric level 45 item 1913 power 81398 name Other"),
                &format!("gear {t2} finder server 1201 class Elementalist level 45 item 861 power 45961 name Third"),
                &format!("gear {t3} legion server 1201 class Templar level 45 item 1403 power 67893 name Fourth"),
                &format!("gear {t3} legion server 1201 class Cleric level 45 item 843 power 37476 name Fifth"),
                &format!("gear {t3} legion server 1201 class Chanter level 45 item 2116 power 95541 name Seventh"),
            ]
        );
        let mut lines = Vec::new();
        run(&text, Options::default(), &mut |l| lines.push(l));
        assert!(!lines.iter().any(|l| l.starts_with("gear ")));
    }
}
