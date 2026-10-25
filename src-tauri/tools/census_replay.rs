//! `census-replay`: replays one packet capture through the meter's own
//! parser and writes what it states about the players met, for a census of
//! classes and gear: a separate program that reads nothing else from the
//! meter.
//!
//! ```text
//! census-replay <packets_*.txt>
//! ```
//!
//! One JSON object per line, schema 1, in this order:
//!
//! ```text
//! {"type":"census-replay","schema":1}
//! {"type":"player", "id", "name", "server", "class", "first_ms", "last_ms"}
//!     one per name, by name; server and class null when nothing stated them
//! {"type":"reading", "at", "list", "name", "server", "account", "class",
//!  "level", "item_level", "combat_power"}
//!     each Item Level and Combat Power a list stated, in capture order;
//!     list: party, party-0b97, party-1f97, finder or legion
//! {"type":"rooms", "at", "dungeon", "mine", "rooms"}
//!     each party finder room list, in capture order; mine: a room or null
//! {"type":"end", "payloads"}
//!     always last: a run without it did not finish
//!
//! room  {"number", "title", "dungeon", "stage", "faction", "leader",
//!        "public", "join", "state", "slots"}
//! slot  {"slot", "role"}, and for a member "name", "account", "class",
//!       "level", "item_level", "home", "current", "combat_power"
//! ```
//!
//! Times are capture Unix ms. A field that changes meaning or goes away
//! raises the schema; a new field does not.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use a2tools_dps_meter_lib::capture::byte_queue::ByteQueue;
use a2tools_dps_meter_lib::capture::framing;
use a2tools_dps_meter_lib::capture::packet_log::{LogLine, PacketLogReplay, RepeatFilter};
use a2tools_dps_meter_lib::capture::player_facts::{self, ListKind, ListMember, Room, RoomList};
use a2tools_dps_meter_lib::combat::data_storage::DataStorage;
use a2tools_dps_meter_lib::entity::job_class::JobClass;
use a2tools_dps_meter_lib::i18n::lookup::{NpcLookup, SkillLookup};
use serde_json::{json, Value};

const USAGE: &str = "usage: census-replay <packets_*.txt>\n";

/// The output's schema (see the module header).
const SCHEMA: u32 = 1;

/// The parser's tables, from the repo's `src/data` wherever the tool was built.
const TABLE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/data");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path] = args.as_slice() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let run = || -> Result<(), String> {
        let tables = Tables::load()?;
        let bytes = fs::read(path).map_err(|why| format!("cannot read {path}: {why}"))?;
        let mut out = BufWriter::new(io::stdout().lock());
        let written = replay(&String::from_utf8_lossy(&bytes), &tables).iter().try_for_each(|record| writeln!(out, "{record}"));
        written.and_then(|()| out.flush()).map_err(|why| format!("cannot write: {why}"))
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("census-replay: {why}");
            ExitCode::FAILURE
        }
    }
}

/// The parser's lookups: skill and NPC names, and the skills that tick.
struct Tables {
    skills: Arc<SkillLookup>,
    npcs: Arc<NpcLookup>,
    ticks: HashSet<i32>,
}

impl Tables {
    /// A table that is missing or does not parse stops the run: the census
    /// must not count a capture with half a parser.
    fn load() -> Result<Self, String> {
        let read = |rel: &str| {
            let path = Path::new(TABLE_DIR).join(rel);
            fs::read_to_string(&path).map_err(|why| format!("cannot read the table {}: {why}", path.display()))
        };
        let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        skills.load_from_json(&read("i18n/skills/en.json")?);
        npcs.load_from_json(&read("i18n/npcs/en.json")?);
        let ticks = serde_json::from_str(&read("dot_skill_ids.json")?).map_err(|why| format!("dot_skill_ids.json: {why}"))?;
        Ok(Tables { skills, npcs, ticks })
    }
}

/// The records for one capture, in output order. Each line goes through
/// the parser first; then its whole packets, bundles opened, go to the
/// player readers; then the class skills its hits added are counted.
fn replay(text: &str, tables: &Tables) -> Vec<Value> {
    let storage = Arc::new(DataStorage::new());
    let mut parser = PacketLogReplay::new(Arc::clone(&storage), Arc::clone(&tables.skills), Arc::clone(&tables.npcs), tables.ticks.clone());
    let mut held: HashMap<String, ByteQueue> = HashMap::new();
    let mut repeats = RepeatFilter::default();
    let mut tally = Tally::default();
    let mut before = storage.copy_targets_brief();
    let (mut payloads, mut first_ms, mut last_ms) = (0usize, None::<i64>, 0i64);
    for line in text.lines().filter_map(LogLine::parse) {
        let ms = line.at_ms;
        payloads += 1;
        first_ms.get_or_insert(ms);
        last_ms = ms;
        parser.feed(&line);
        if !repeats.is_repeat(&line) {
            let packets = framing::take_packets(held.entry(line.stream.to_string()).or_default(), &line.bytes);
            for p in &packets {
                tally.read_records(p, ms);
                tally.read_list(p, ms);
                for inner in framing::embedded_bundles(p) {
                    tally.read_records(&inner.data, ms);
                    framing::visit_packets(&inner.data, 1, &mut |q| tally.read_list(q, ms));
                }
            }
        }
        tally.roster_and_self(&storage, ms);
        let now = storage.copy_targets_brief();
        let mut targets: Vec<&i32> = now.keys().collect();
        targets.sort();
        for target in targets {
            for ((actor, skill, _), (hits, _)) in now[target].row_changes(before.get(target)) {
                if hits > 0 {
                    tally.hit(&storage, actor, skill, ms);
                }
            }
        }
        before = now;
    }
    let mut records = vec![json!({ "type": "census-replay", "schema": SCHEMA })];
    records.extend(tally.records(&storage, first_ms.unwrap_or(0), last_ms));
    records.push(json!({ "type": "end", "payloads": payloads }));
    records
}

/// Each player met, by name: a player gets a new entity id on every zone
/// load, and the name is what joins them.
#[derive(Default)]
struct Tally {
    by_name: BTreeMap<String, Seen>,
    /// Item Level and Combat Power readings in capture order. A searched list
    /// keeps a reading only when it differs from what the same list said last
    /// about that player: the party roster is sent again on every party change.
    readings: Vec<(i64, ListKind, ListMember)>,
    last: HashMap<(String, ListKind), ListMember>,
    rooms: Vec<(i64, RoomList)>,
}

/// What each source said about one player, with how often.
#[derive(Default)]
struct Seen {
    /// The entity id they had last.
    id: i32,
    /// Classes by `class_vote`.
    own: Option<String>,
    roster: Votes<String>,
    records: Votes<String>,
    skills: Votes<String>,
    roster_server: Option<u16>,
    record_servers: Votes<u16>,
    /// The server a loot or summon record gave with the name.
    loot_server: Option<u16>,
    first_ms: i64,
    last_ms: i64,
}

type Votes<T> = BTreeMap<T, u32>;

/// The value most votes went to; a tie goes to the first in order.
fn top<T: Clone + Ord>(votes: &Votes<T>) -> Option<T> {
    votes.iter().rev().max_by_key(|(_, n)| **n).map(|(v, _)| v.clone())
}

impl Tally {
    fn seen(&mut self, name: &str, id: i32, at: i64) -> &mut Seen {
        let p = self.by_name.entry(name.to_string()).or_insert_with(|| Seen { first_ms: at, last_ms: at, ..Default::default() });
        if id != 0 {
            p.id = id;
        }
        p.first_ms = p.first_ms.min(at);
        p.last_ms = p.last_ms.max(at);
        p
    }

    fn read_records(&mut self, data: &[u8], at: i64) {
        for r in player_facts::player_records(data) {
            let p = self.seen(&r.name, r.id, at);
            if let Some(class) = r.class {
                *p.records.entry(class_vote(class)).or_default() += 1;
            }
            if let Some(server) = r.server {
                *p.record_servers.entry(server).or_default() += 1;
            }
        }
    }

    fn read_list(&mut self, packet: &[u8], at: i64) {
        let Some(list) = player_facts::read_list(packet, at) else { return };
        let searched = list.rooms.is_none();
        for m in list.members {
            if searched {
                let key = (m.name.clone(), list.kind);
                if self.last.get(&key) == Some(&m) {
                    continue;
                }
                self.last.insert(key, m.clone());
            }
            self.readings.push((at, list.kind, m));
        }
        if let Some(rooms) = list.rooms {
            self.rooms.push((at, rooms));
        }
    }

    /// A class skill used by `actor` while it carried a name.
    fn hit(&mut self, storage: &DataStorage, actor: i32, skill: i32, at: i64) {
        let Some(class) = JobClass::job_of_skill(skill) else { return };
        let Some(name) = storage.stored_name(actor) else { return };
        *self.seen(&name, actor, at).skills.entry(class_vote(class)).or_default() += 1;
    }

    /// Your party's roster and your own record, as the parser holds them now.
    fn roster_and_self(&mut self, storage: &DataStorage, at: i64) {
        for (name, m) in storage.roster_entries() {
            let p = self.seen(&name, 0, at);
            if m.server_id != 0 {
                p.roster_server = Some(m.server_id);
            }
            if let Some(class) = m.job {
                *p.roster.entry(class_vote(class)).or_default() += 1;
            }
        }
        let me = storage.self_card();
        if let (Some(name), Some(id)) = (me.name, storage.local_player_id()) {
            let p = self.seen(&name, id as i32, at);
            p.own = me.class.map(class_vote).or(p.own.take());
        }
    }

    /// The player, reading and room records. A player's class is what the
    /// game stated (your own record, then the roster, then the player
    /// records), else the class skills used; the server is the roster's, then
    /// the player records', then the one a loot or summon record gave with the
    /// name. Players only such a record named are dated by the whole capture.
    fn records(mut self, storage: &DataStorage, first_ms: i64, last_ms: i64) -> Vec<Value> {
        for (name, server) in storage.player_servers() {
            let p = self.by_name.entry(name).or_insert_with_key(|name| Seen {
                id: storage.id_for_name(name).unwrap_or(0),
                first_ms,
                last_ms,
                ..Default::default()
            });
            p.loot_server = Some(server);
        }
        let players = self.by_name.into_iter().map(|(name, p)| {
            let class = p.own.or_else(|| top(&p.roster)).or_else(|| top(&p.records)).or_else(|| top(&p.skills));
            let server = p.roster_server.or(top(&p.record_servers)).or(p.loot_server);
            json!({
                "type": "player", "id": p.id, "name": name, "server": server, "class": class,
                "first_ms": p.first_ms, "last_ms": p.last_ms,
            })
        });
        let readings = self.readings.into_iter().map(|(at, kind, m)| {
            json!({
                "type": "reading", "at": at, "list": list_name(kind), "name": m.name, "server": m.world, "account": m.account,
                "class": m.class, "level": m.level, "item_level": m.item_level, "combat_power": m.combat_power,
            })
        });
        let rooms = self.rooms.into_iter().map(|(at, l)| {
            json!({
                "type": "rooms", "at": at, "dungeon": l.dungeon, "mine": l.mine.as_ref().map(room_json),
                "rooms": l.rooms.iter().map(room_json).collect::<Vec<_>>(),
            })
        });
        players.chain(readings).chain(rooms).collect()
    }
}

/// A class as votes keep it: its name, so a tie goes to the first name in
/// order and a replay writes the same every time.
fn class_vote(class: JobClass) -> String {
    format!("{class:?}")
}

/// A list's name in the output (see the module header).
fn list_name(kind: ListKind) -> &'static str {
    match kind {
        ListKind::Party => "party",
        ListKind::PartyMember0b => "party-0b97",
        ListKind::PartyMember1f => "party-1f97",
        ListKind::Finder => "finder",
        ListKind::Legion => "legion",
    }
}

fn room_json(r: &Room) -> Value {
    let slots: Vec<Value> = r
        .slots
        .iter()
        .map(|s| match &s.member {
            None => json!({ "slot": s.number, "role": s.role }),
            Some(m) => json!({
                "slot": s.number, "role": s.role, "name": m.name, "account": m.account, "class": m.class,
                "level": m.level, "item_level": m.item_level, "home": m.home, "current": m.current,
                "combat_power": m.combat_power,
            }),
        })
        .collect();
    json!({
        "number": r.number, "title": r.title, "dungeon": r.dungeon, "stage": r.stage, "faction": r.faction,
        "leader": r.leader, "public": r.public, "join": r.join, "state": r.state, "slots": slots,
    })
}

// The crate's record builders (`src/built.rs`) name the library through
// `crate::`; this leads there.
#[cfg(test)]
use a2tools_dps_meter_lib::{capture, entity};

#[cfg(test)]
#[allow(dead_code)]
#[path = "../src/built.rs"]
mod built;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{account, frame, player_record, roster_member, to_hex, two_rooms, Member};

    /// A capture line: `packet` on one client stream, at second `second`.
    fn line(second: u32, packet: &[u8]) -> String {
        format!("2026-10-15T04:20:{second:02}.000-07:00|Client:50000:13328|{}", to_hex(packet))
    }

    /// A made-up capture: a player record (Tester, Cleric, 1201), one party
    /// member update sent twice (Other, made on 2201, now on 2202), and the
    /// builders' party finder list with two rooms.
    fn sample() -> String {
        let after_name = [
            &30u32.to_le_bytes()[..],
            &[0x01, 0x02, 0x00, 0x00, 0xb1, 0x04, 0x22, 0x11, 0x01, 0x02, 0x03, 0x04, 0x05],
            &[0xff; 8],
            &4_102_412_400_000i64.to_le_bytes(),
            &[0x00, 0x00],
        ]
        .concat();
        let other = Member {
            mask: 0x0c,
            slot: 2,
            dbid: account(0x3d60d, 2201),
            name: "Other",
            class: 30,
            level: 45,
            gear: 1913,
            extra: &[],
            tag: 0x04,
            combat_power: 81_398,
            tail: &[0x01, 0x01],
        };
        let update = frame(&[&[0x0b, 0x97][..], &roster_member(&other, 2202)].concat());
        [
            line(0, &player_record(9001, Some("Tester"), &after_name)),
            line(1, &update),
            line(2, &update),
            line(3, &frame(&two_rooms())),
        ]
        .join("\n")
    }

    fn tables() -> Tables {
        Tables::load().expect("the repo's tables")
    }

    /// The schema the census reads, written out in full for the sample: every
    /// record type but `rooms`' empty own room, every key. The census's own
    /// test reads this same text (its `tests/fixtures/census-replay-sample.jsonl`).
    #[test]
    fn the_sample_capture_writes_schema_1() {
        let out: Vec<String> = replay(&sample(), &tables()).iter().map(Value::to_string).collect();
        let want = [
            r#"{"schema":1,"type":"census-replay"}"#,
            r#"{"class":"Cleric","first_ms":1792063200000,"id":9001,"last_ms":1792063200000,"name":"Tester","server":1201,"type":"player"}"#,
            r#"{"account":619526423740405261,"at":1792063201000,"class":"Cleric","combat_power":81398,"item_level":1913,"level":45,"list":"party-0b97","name":"Other","server":2202,"type":"reading"}"#,
            r#"{"account":619526423740405261,"at":1792063203000,"class":"Cleric","combat_power":81398,"item_level":1913,"level":45,"list":"finder","name":"Other","server":2201,"type":"reading"}"#,
            r#"{"account":619526423740488243,"at":1792063203000,"class":"Cleric","combat_power":60123,"item_level":1500,"level":45,"list":"finder","name":"Moved","server":2202,"type":"reading"}"#,
            r#"{"account":338051447029829129,"at":1792063203000,"class":"Elementalist","combat_power":45961,"item_level":861,"level":45,"list":"finder","name":"Third","server":1201,"type":"reading"}"#,
            r#"{"account":338051447029805058,"at":1792063203000,"class":"Gladiator","combat_power":67893,"item_level":1403,"level":45,"list":"finder","name":"Fourth","server":1201,"type":"reading"}"#,
            r#"{"at":1792063203000,"dungeon":600012,"mine":null,"rooms":[{"dungeon":600012,"faction":3,"join":1,"leader":619526423740405261,"number":101,"public":true,"slots":[{"account":619526423740405261,"class":"Cleric","combat_power":81398,"current":2201,"home":2201,"item_level":1913,"level":45,"name":"Other","role":3,"slot":1},{"account":619526423740488243,"class":"Cleric","combat_power":60123,"current":2202,"home":2202,"item_level":1500,"level":45,"name":"Moved","role":2,"slot":2},{"role":3,"slot":3},{"role":4,"slot":4},{"role":4,"slot":5}],"stage":0,"state":0,"title":"75K+ 3B"},{"dungeon":600012,"faction":3,"join":2,"leader":338051447029829129,"number":102,"public":true,"slots":[{"account":338051447029829129,"class":"Elementalist","combat_power":45961,"current":null,"home":1201,"item_level":861,"level":45,"name":"Third","role":4,"slot":1},{"account":338051447029805058,"class":"Gladiator","combat_power":67893,"current":null,"home":1201,"item_level":1403,"level":45,"name":"Fourth","role":1,"slot":2},{"role":4,"slot":3},{"role":4,"slot":4},{"role":4,"slot":5}],"stage":0,"state":2,"title":"Leaving now."}],"type":"rooms"}"#,
            r#"{"payloads":4,"type":"end"}"#,
        ];
        assert_eq!(out, want);
    }

    #[test]
    fn an_empty_capture_writes_the_head_and_the_end_only() {
        let out: Vec<String> = replay("", &tables()).iter().map(Value::to_string).collect();
        assert_eq!(out, [r#"{"schema":1,"type":"census-replay"}"#, r#"{"payloads":0,"type":"end"}"#]);
    }
}
