//! The player list of the replay report (`A2_REPLAY_PLAYERS=1`): every player
//! met, with home server and class and where each came from.

use std::collections::BTreeMap;

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
    /// are dated by the whole capture.
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
}
