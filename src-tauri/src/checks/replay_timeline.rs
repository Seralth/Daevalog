//! `A2_REPLAY_TIMELINE`: each fight of the local player with its hits, the
//! buffs and debuffs on you, your summons and the target, and your stats.
//!
//! A fight is the local player's and their summons' hits on one target, cut
//! where they stop for more than 15 s. Times are capture ms, as in the
//! `hit_flags` lines. Per fight one `fight` line, one `buff` line per timed
//! buff or debuff of yours (its name, times from the fight's start, uptime
//! in the fight), then the whole fight as one `timeline {json}` line, other
//! players' debuffs on the target and passives included, with the game's
//! English names of the abnormals and stats. The JSON keeps your and your
//! summons' DoT ticks on the target (`dot_ticks` lines: the ticks the game's
//! records count, so none of a spirit that has left) apart from the hits:
//! from the window's start when one is given, else from 15 s before the first
//! hit, never the ticks of the fight before on that target, up to 15 s after
//! the last hit.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

use crate::capture::abnormal::{self, Instance, StatEvent, Timeline};
use crate::entity::owner_chain;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// The gap that ends a fight, as the encounter mode's default.
const FIGHT_GAP_MS: i64 = 15_000;
/// With no window given, how long before a fight's first hit its DoT ticks
/// count: the same gap, so ticks belong to a fight on either side alike.
const TICK_LEAD_MS: i64 = FIGHT_GAP_MS;

/// The `hit_flags` and `dot_ticks` trace lines, kept instead of printed,
/// each with its target and level.
#[derive(Clone, Default)]
pub(crate) struct HitTap(Arc<Mutex<Vec<(&'static str, tracing::Level, String)>>>);

impl HitTap {
    /// Collect the lines while the guard lives (this thread only).
    pub(crate) fn install(&self) -> tracing::subscriber::DefaultGuard {
        self.install_for(&["hit_flags", "dot_ticks"])
    }
    /// The same for the lines of these targets.
    pub(crate) fn install_for(&self, targets: &'static [&'static str]) -> tracing::subscriber::DefaultGuard {
        let filter = tracing_subscriber::filter::filter_fn(move |m| targets.contains(&m.target()));
        crate::trace_tap::set_default(tracing_subscriber::registry().with(self.clone().with_filter(filter)))
    }
    pub(crate) fn take(&self) -> Vec<(&'static str, tracing::Level, String)> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// The text of an event's `message` field; other fields are passed over.
#[derive(Default)]
struct MessageText(String);

impl Visit for MessageText {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        // A format message's Debug form is its plain text.
        if f.name() == "message" {
            self.0 = format!("{v:?}");
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for HitTap {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let meta = event.metadata();
        let mut message = MessageText::default();
        event.record(&mut message);
        let line = (meta.target(), *meta.level(), message.0);
        self.0.lock().unwrap().push(line);
    }
}

/// One `hit_flags` or `dot_ticks` line: `<ms> key=value ...`.
struct Hit {
    ms: i64,
    actor: i32,
    target: i32,
    fields: Map<String, Value>,
}

fn read_hit(line: &str) -> Option<Hit> {
    let mut words = line.split_whitespace();
    let ms = words.next()?.parse().ok()?;
    let mut fields = Map::new();
    for w in words {
        let (k, v) = w.split_once('=')?;
        let v = v.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(v));
        fields.insert(k.to_string(), v);
    }
    let id = |k: &str| fields.get(k).and_then(Value::as_i64).map(|v| v as i32);
    Some(Hit { ms, actor: id("actor")?, target: id("target")?, fields })
}

/// What the replay gathers for the timeline while it runs.
#[derive(Default)]
pub(crate) struct Gather {
    pub tap: HitTap,
    pub timeline: Timeline,
    /// The game's English names of abnormals and of stats.
    abnormal_names: HashMap<u32, String>,
    stat_names: HashMap<u16, String>,
    hits: Vec<Hit>,
    dots: Vec<Hit>,
    /// (from ms, entity) each time the local player's id changed.
    local: Vec<(i64, i32)>,
    /// (ms, summon links) at each map load and at the end: a link holds
    /// from the snapshot before it up to this one.
    links: Vec<(i64, HashMap<i32, i32>)>,
    map_loads: usize,
    /// (ms, NPC code) per target, at its first hit and each time the code
    /// changed: the game hands a dead monster's id to a later spawn, so the
    /// code at the end of the capture can be another NPC's.
    codes: HashMap<i32, Vec<(i64, i32)>>,
}

impl Gather {
    /// With the game data in `data_dir`: each abnormal's stack limit, and
    /// the names of abnormals and stats.
    pub(crate) fn new(data_dir: &std::path::Path) -> Gather {
        let read = |file: &str| -> Value {
            std::fs::read_to_string(data_dir.join(file)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
        };
        let names = |table: &Value| -> Vec<(String, String)> {
            let entries = table.as_object().into_iter().flatten();
            entries.filter_map(|(id, name)| Some((id.clone(), name.as_str()?.to_string()))).collect()
        };
        let mut g = Gather::default();
        g.abnormal_names = names(&read("i18n/abnormals/en.json")).into_iter().filter_map(|(id, n)| Some((id.parse().ok()?, n))).collect();
        // A stat's English name, else the game's own name for it (EStat).
        let stats = names(&read("stats.json")["stats"]).into_iter().chain(names(&read("i18n/stats/en.json")));
        g.stat_names = stats.filter_map(|(id, n)| Some((id.parse().ok()?, n))).collect();
        g.timeline.set_stack_limits(abnormal::stack_limits(data_dir));
        g
    }

    fn take_lines(&mut self) {
        for (kind, _, line) in self.tap.take() {
            let Some(h) = read_hit(&line) else { continue };
            if kind == "dot_ticks" { self.dots.push(h) } else { self.hits.push(h) }
        }
    }

    /// After each capture line: the hits it made and the NPC code of each
    /// one's target then (`code`), who you are, and the summon links before
    /// a map load makes the entities anew.
    pub(crate) fn after_line(
        &mut self,
        ms: i64,
        local: Option<i32>,
        code: impl Fn(i32) -> Option<i32>,
        links: impl FnOnce() -> HashMap<i32, i32>,
    ) {
        let seen = self.hits.len();
        self.take_lines();
        for h in &self.hits[seen..] {
            let Some(c) = code(h.target) else { continue };
            let list = self.codes.entry(h.target).or_default();
            if list.last().is_none_or(|l| l.1 != c) {
                list.push((h.ms, c));
            }
        }
        if let Some(id) = local
            && self.local.last().is_none_or(|l| l.1 != id)
        {
            self.local.push((ms, id));
        }
        let loads = self.timeline.map_loads();
        if loads != self.map_loads {
            self.map_loads = loads;
            self.links.push((ms, links()));
        }
    }

    fn local_at(&self, from: i64, to: i64) -> HashSet<i32> {
        let mut out = HashSet::new();
        for (i, &(since, id)) in self.local.iter().enumerate() {
            let until = self.local.get(i + 1).map_or(i64::MAX, |n| n.0);
            if since <= to && until >= from {
                out.insert(id);
            }
        }
        out
    }

    fn links_at(&self, ms: i64) -> &HashMap<i32, i32> {
        &self.links.iter().find(|(at, _)| *at >= ms).or(self.links.last()).expect("links at the end").1
    }

    /// A line of yours or your summons' on an enemy, in the window.
    fn own(&self, h: &Hit, window: (Option<i64>, i64), target: Option<i32>) -> bool {
        if window.0.is_some_and(|from| h.ms < from) || h.ms > window.1 || target.is_some_and(|t| t != h.target) {
            return false;
        }
        let links = self.links_at(h.ms);
        let local = self.local_at(h.ms, h.ms);
        // Heals on you and your spirits are not a fight.
        let friendly = local.contains(&owner_chain::resolve(h.target, links));
        local.contains(&owner_chain::resolve(h.actor, links)) && !friendly
    }

    /// Print every fight in `window` (capture ms; from the capture's start
    /// when its start is None), only `target`'s if given.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn report(
        mut self,
        zone: chrono::FixedOffset,
        end_ms: i64,
        final_links: HashMap<i32, i32>,
        window: (Option<i64>, i64),
        target: Option<i32>,
        mobs: &HashMap<i32, i32>,
        skills: &SkillLookup,
        npcs: &NpcLookup,
        out: &mut dyn FnMut(String),
    ) {
        self.take_lines();
        self.links.push((end_ms, final_links));
        let instances = self.timeline.all_instances();
        let mut by_target: BTreeMap<i32, Vec<&Hit>> = BTreeMap::new();
        for h in self.hits.iter().filter(|h| self.own(h, window, target)) {
            by_target.entry(h.target).or_default().push(h);
        }
        let dots: Vec<&Hit> = self.dots.iter().filter(|d| self.own(d, window, target)).collect();
        // (target, hits, ms of its first tick)
        let mut fights: Vec<(i32, Vec<&Hit>, i64)> = Vec::new();
        for (t, hits) in by_target {
            // Ticks before the first hit (a DoT cast before the window, or
            // by a cast that does no damage) count from the window's start,
            // else up to TICK_LEAD_MS before; never the fight before's.
            let mut ticks_end = i64::MIN;
            for run in hits.chunk_by(|a, b| b.ms - a.ms <= FIGHT_GAP_MS) {
                let (start, end) = (run[0].ms, run[run.len() - 1].ms);
                let from = window.0.unwrap_or(start - TICK_LEAD_MS).max(ticks_end.saturating_add(1));
                ticks_end = end + FIGHT_GAP_MS;
                fights.push((t, run.to_vec(), from));
            }
        }
        fights.sort_by_key(|(_, h, _)| h[0].ms);
        out(format!("\n== timeline: {} fights", fights.len()));
        for (t, hits, ticks_from) in fights {
            let (start, end) = (hits[0].ms, hits[hits.len() - 1].ms);
            let fight = self.fight(t, start, end, ticks_from, &hits, &dots, &instances, zone, mobs, skills, npcs);
            for line in fight.0 {
                out(line);
            }
            out(format!("timeline {}", fight.1));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fight(
        &self,
        target: i32,
        start: i64,
        end: i64,
        ticks_from: i64,
        hits: &[&Hit],
        dots: &[&Hit],
        instances: &[Instance],
        zone: chrono::FixedOffset,
        mobs: &HashMap<i32, i32>,
        skills: &SkillLookup,
        npcs: &NpcLookup,
    ) -> (Vec<String>, Value) {
        let local = self.local_at(start, end);
        let mut local_ids: Vec<i32> = local.iter().copied().collect();
        local_ids.sort();
        let links = self.links_at(end);
        let owner = |id: i32| owner_chain::resolve(id, links);
        let who = |entity: i32| {
            if entity == target {
                Some("target")
            } else if local.contains(&entity) {
                Some("self")
            } else if local.contains(&owner(entity)) {
                Some("summon")
            } else {
                None
            }
        };
        let near: Vec<Instance> = instances
            .iter()
            .filter(|i| who(i.entity).is_some() && i.start_ms <= end && i.end_ms.is_none_or(|e| e >= start))
            .cloned()
            .collect();
        let tracks = abnormal::tracks(&near, owner);
        // The code the target had by the fight's end, else the last one seen.
        let then = self.codes.get(&target).and_then(|list| list.iter().rev().find(|(at, _)| *at <= end)).map(|l| l.1);
        let target_name = then.or_else(|| mobs.get(&target).copied()).map(|c| npcs.npc_title(c)).unwrap_or_default();
        let length = (end - start).max(1);
        let tod = |ms: i64| {
            chrono::DateTime::from_timestamp_millis(ms)
                .map(|t| t.with_timezone(&zone).format("%H:%M:%S%.3f").to_string())
                .unwrap_or_default()
        };
        let mut lines = vec![format!(
            "\nfight target {target} {target_name} {} .. {} ({:.1} s): {} hits, {} buffs, local {local_ids:?}",
            tod(start),
            tod(end),
            length as f64 / 1000.0,
            hits.len(),
            tracks.len(),
        )];
        let mut buffs = Vec::new();
        for t in &tracks {
            let source = if local.contains(&t.owner) { "self" } else if t.owner == target { "target" } else { "other" };
            let name = t.skills.first().map(|&s| skills.skill_title(s as i32)).unwrap_or_default();
            let own_name = self.abnormal_names.get(&t.abnormal).map_or("", String::as_str);
            // Time on in the fight, and the stack count weighted by it.
            let (mut on, mut weighted) = (0i64, 0i64);
            for (k, &(at, n)) in t.stacks.iter().enumerate() {
                let next = t.stacks.get(k + 1).map_or(t.end_ms.unwrap_or(end), |s| s.0);
                let span = next.min(end) - at.max(start);
                if n > 0 && span > 0 {
                    on += span;
                    weighted += span * n as i64;
                }
            }
            if !t.endless && source == "self" {
                lines.push(format!(
                    "buff {} {} {} {own_name:?} via {name:?} from {source} lv {} {:+.1} .. {} s, {}, uptime {:.0}%, stacks {:.1} avg {} max",
                    who(t.entity).unwrap_or("-"),
                    t.entity,
                    t.abnormal,
                    t.level,
                    (t.start_ms - start) as f64 / 1000.0,
                    t.end_ms.map_or("-".to_string(), |e| format!("{:+.1}", (e - start) as f64 / 1000.0)),
                    t.end.label(),
                    100.0 * on as f64 / length as f64,
                    if on > 0 { weighted as f64 / on as f64 } else { 0.0 },
                    t.stacks.iter().map(|s| s.1).max().unwrap_or(0),
                ));
            }
            buffs.push(json!({
                "on": who(t.entity),
                "entity": t.entity,
                "abnormal": t.abnormal,
                "name": own_name,
                "source": source,
                "source_entity": t.owner,
                "skills": t.skills,
                "skill_name": name,
                "level": t.level,
                "endless": t.endless,
                "start_ms": t.start_ms,
                "end_ms": t.end_ms,
                "end": t.end.label(),
                "uptime_ms": on,
                "stacks": t.stacks,
            }));
        }
        let (at_start, changes) = stats_in(&self.timeline.stats, &local, start, end);
        let stat_ids = at_start.keys().copied().chain(changes.iter().filter_map(|c| c["stat"].as_u64().map(|s| s as u16)));
        let stat_names: BTreeMap<u16, &str> =
            stat_ids.filter_map(|id| Some((id, self.stat_names.get(&id)?.as_str()))).collect();
        let row = |h: &&Hit| {
            let mut f = h.fields.clone();
            f.insert("ms".into(), h.ms.into());
            f.insert("owner".into(), owner(h.actor).into());
            Value::Object(f)
        };
        // Ticks go on after the last hit. The next fight on this target
        // starts more than FIGHT_GAP_MS later.
        let dots = dots.iter().filter(|d| d.target == target && d.ms >= ticks_from && d.ms <= end + FIGHT_GAP_MS);
        let json = json!({
            "target": target,
            "target_name": target_name,
            "start_ms": start,
            "end_ms": end,
            "local_ids": local_ids,
            "hits": hits.iter().map(row).collect::<Vec<_>>(),
            "dots": dots.map(row).collect::<Vec<_>>(),
            "buffs": buffs,
            "stats": { "at_start": at_start, "changes": changes, "names": stat_names },
        });
        (lines, json)
    }
}

/// Your stat sheet when the fight starts, and each change during it.
fn stats_in(events: &[StatEvent], local: &HashSet<i32>, start: i64, end: i64) -> (BTreeMap<u16, i32>, Vec<Value>) {
    let mut sheet = BTreeMap::new();
    let mut changes = Vec::new();
    for e in events {
        if e.ms > end {
            break;
        }
        if e.entity.is_some_and(|id| !local.contains(&id)) {
            continue;
        }
        if e.ms <= start {
            if e.whole_sheet {
                sheet.clear();
            }
            sheet.extend(e.values.iter().copied());
        } else {
            for &(stat, value) in &e.values {
                changes.push(json!({ "ms": e.ms, "stat": stat, "value": value }));
            }
        }
    }
    (sheet, changes)
}

#[cfg(test)]
mod tests {
    use super::super::replay_report::{run, Options};
    use crate::built::{self, Abnormal, Hit, F};

    // Records built from the field layout with made-up values: you are
    // `YOU`, your Fire Spirit `SPIRIT`, fighting `TARGET`.
    const YOU: u32 = 2_800;
    const SPIRIT: u32 = 28_001;
    const TARGET: u32 = 40_051;
    /// Noon UTC on the day the lines are dated.
    const NOON: i64 = 1_767_355_200_000;

    fn stats_off() -> Vec<u8> {
        built::stats(YOU, &[(65, 500), (76, 0), (379, 1000), (380, 0)])
    }

    /// A Spiritmaster's hit of yours on `TARGET`, layout 6, from the front.
    fn hit(skill: u32, uid: u8, values: &[u32]) -> Hit {
        Hit {
            target: TARGET,
            switch: 0x06,
            actor: YOU,
            skill,
            uid,
            hit_type: 2,
            plotter: Some((0, 0, 1)),
            effect: skill * 100 + 11,
            count: 1,
            values: values.to_vec(),
            ..Hit::default()
        }
    }

    /// Records: the Fire Spirit's spawn, two own stat records,
    /// Spirit's Benediction on and its stats, two hits on the target, the
    /// buff and its stats off.
    #[test]
    fn a_fight_lists_the_buffs_and_stats_around_its_hits() {
        // Added 40 ms after its start on the server's clock.
        let benediction = Abnormal {
            flags: 0x13,
            instance: 300,
            abnormal: 161_900_001,
            length: 10_000,
            end: NOON + 11_960,
            caster: YOU,
            level: 2,
            skill: Some(16_190_000),
            byte: Some(0),
            position: [-1500.5, 2500.25, 120.0],
        };
        let lines = [
            ("12:00:00.000", built::spirit_spawn(SPIRIT, YOU)),
            ("12:00:01.100", stats_off()),
            ("12:00:01.200", stats_off()),
            ("12:00:02.000", benediction.added(YOU)),
            ("12:00:02.000", built::stats(YOU, &[(65, 1500), (76, 1000), (379, 3000), (380, 2000)])),
            ("12:00:04.000", hit(16_370_001, 0x41, &[11_000, 1_200, 1, 0]).packet()),
            ("12:00:04.100", hit(16_300_030, 0x42, &[11_000, 9_500, 1, 0]).packet()),
            ("12:00:12.000", built::removed(YOU, &[(0x00, 300, 1, None)])),
            ("12:00:12.000", stats_off()),
        ];
        let text: String = lines
            .iter()
            .map(|(t, bytes)| format!("2026-01-02T{t}000000+00:00|Client:40000:13328|{}\n", built::to_hex(bytes)))
            .collect();
        let mut out = Vec::new();
        run(&text, Options { timeline: true, ..Default::default() }, &mut |l| out.push(l));
        let fight = format!("fight target {TARGET} ");
        assert!(out.iter().any(|l| l.trim_start().starts_with(&fight) && l.contains("2 hits")), "{out:#?}");
        let json = out.iter().find_map(|l| l.strip_prefix("timeline ")).expect("a timeline line");
        let fight: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(fight["hits"].as_array().map(Vec::len), Some(2));
        let buff = fight["buffs"].as_array().unwrap().iter().find(|b| b["abnormal"] == 161_900_001).expect("the buff");
        let (on, off) = (NOON + 2_000, NOON + 12_000);
        assert_eq!(
            (buff["on"].as_str(), buff["source"].as_str(), buff["start_ms"].as_i64(), buff["end_ms"].as_i64()),
            (Some("self"), Some("self"), Some(on), Some(off))
        );
        assert_eq!(buff["stacks"], serde_json::json!([[on, 1], [off, 0]]));
        assert_eq!(buff["name"], "Spirit's Benediction");
        let buff = format!("buff self {YOU} 161900001 \"Spirit's Benediction\" via ");
        assert!(out.iter().any(|l| l.starts_with(&buff)), "{out:#?}");
        assert_eq!(fight["stats"]["at_start"]["379"], 3000);
        assert_eq!(fight["stats"]["names"]["379"], "PvE Damage Boost");
        // The spirit's passive from its spawn, at its summon's level.
        let passive = fight["buffs"].as_array().unwrap().iter().find(|b| b["abnormal"] == 161_002_304).expect("the passive");
        assert_eq!(
            (passive["on"].as_str(), passive["entity"].as_i64(), passive["level"].as_i64(), passive["endless"].as_bool()),
            (Some("summon"), Some(SPIRIT as i64), Some(12), Some(true))
        );
        assert_eq!(passive["name"], "Fire Spirit");
    }

    /// `TARGET` spawns as Phantasmal Lakshmi and is fought, then the game
    /// gives its id to Transcendent Bakarma: each fight keeps the name its
    /// target had then.
    #[test]
    fn a_fight_is_named_by_the_npc_its_target_was_then() {
        let spawn = |code: u32| {
            built::Spawn { id: TARGET, kind: 0x04, code, hp: 119_700, filler: 50, ..built::Spawn::default() }.packet()
        };
        let lines = [
            ("12:00:00.000", stats_off()),
            ("12:00:06.100", stats_off()),
            ("12:00:07.000", spawn(2_310_401)),
            ("12:00:08.000", hit(16_370_001, 0x41, &[11_000, 1_200, 1, 0]).packet()),
            ("12:01:00.000", spawn(2_310_471)),
            ("12:01:01.000", hit(16_370_001, 0x42, &[11_000, 1_200, 1, 0]).packet()),
        ];
        let text: String = lines
            .iter()
            .map(|(t, bytes)| format!("2026-01-02T{t}000000+00:00|Client:40000:13328|{}\n", built::to_hex(bytes)))
            .collect();
        let mut out = Vec::new();
        run(&text, Options { timeline: true, ..Default::default() }, &mut |l| out.push(l));
        let fight = format!("fight target {TARGET} ");
        let names: Vec<&str> = out
            .iter()
            .filter_map(|l| l.trim_start().strip_prefix(&fight))
            .map(|l| l.split(" 12:").next().unwrap_or_default())
            .collect();
        assert_eq!(names, ["Phantasmal Lakshmi", "Transcendent Bakarma"], "{out:#?}");
    }

    /// The timeline JSON of each fight in `lines`, replayed
    /// from `from` (time of day) when given.
    fn timelines(lines: &[(&str, Vec<u8>)], from: Option<&str>) -> Vec<serde_json::Value> {
        let text: String = lines
            .iter()
            .map(|(t, bytes)| format!("2026-01-02T{t}000000+00:00|Client:40000:13328|{}\n", built::to_hex(bytes)))
            .collect();
        let mut out = Vec::new();
        let options = Options { timeline: true, from: from.map(str::to_string), ..Default::default() };
        run(&text, options, &mut |l| out.push(l));
        out.iter().filter_map(|l| l.strip_prefix("timeline ")).map(|j| serde_json::from_str(j).unwrap()).collect()
    }

    /// Each fight's ticks: (ms, actor, damage).
    fn ticks(fight: &serde_json::Value) -> Vec<(i64, i64, i64)> {
        let dots = fight["dots"].as_array().expect("dots");
        dots.iter().map(|d| (d["ms"].as_i64().unwrap(), d["actor"].as_i64().unwrap(), d["damage"].as_i64().unwrap())).collect()
    }

    /// Records by you on `TARGET`: two own stat records, a hit
    /// with two additional hits of 50, and `more`. The fight's timeline JSON.
    fn fight_of_one_hit(more: &[(&str, Vec<u8>)]) -> serde_json::Value {
        let hit = Hit {
            switch: 0x26,
            uid: 0x05,
            plotter: Some((0x04, 0, 2)),
            effect: 1_604_024_015,
            ..hit(16_040_240, 0x05, &[11_000, 2_000, 2, 50, 50, 1, 0])
        };
        // The own records over 5 s apart, or the replay takes the second for a resend.
        let lines = [("12:00:00.000", stats_off()), ("12:00:06.100", stats_off()), ("12:00:07.200", hit.packet())];
        let lines: Vec<(&str, Vec<u8>)> = lines.into_iter().chain(more.iter().cloned()).collect();
        timelines(&lines, None).into_iter().next().expect("a timeline line")
    }

    #[test]
    fn a_hit_has_its_additional_hits_damage_and_scalar() {
        let fight = fight_of_one_hit(&[]);
        let hit = &fight["hits"][0];
        assert_eq!(
            (hit["damage"].as_i64(), hit["multi"].as_i64(), hit["multi_dmg"].as_i64(), hit["scalar"].as_i64()),
            (Some(1_900), Some(2), Some(100), Some(11_000))
        );
    }

    /// A Jointstrike: Curse tick (16140000) of yours, 10 s and 16 s after
    /// the hit.
    #[test]
    fn a_fight_keeps_its_dot_ticks_apart_from_its_hits() {
        let tick = built::tick(TARGET, 0x0a, YOU, 1, 1_614_000_051, 750, &[F::U32(16_140_140)]);
        let fight = fight_of_one_hit(&[("12:00:17.200", tick.clone()), ("12:00:23.200", tick)]);
        assert_eq!(fight["hits"].as_array().map(Vec::len), Some(1));
        // The second tick is more than 15 s after the last hit.
        let dots = fight["dots"].as_array().expect("dots");
        assert_eq!(dots.len(), 1, "{dots:?}");
        assert_eq!(
            (dots[0]["actor"].as_i64(), dots[0]["owner"].as_i64(), dots[0]["skill"].as_i64(), dots[0]["damage"].as_i64()),
            (Some(YOU as i64), Some(YOU as i64), Some(16_140_000), Some(750))
        );
        assert_eq!(dots[0]["ms"].as_i64(), Some(NOON + 17_200));
    }

    /// Records on a boss, made-up ids: the
    /// link record to your Wind Spirit, an own record (twice), a hit, then
    /// two Malicious Whirlwind ticks, the spirit's `42 36` flag 7 and two more
    /// ticks. The game's record counted the two before it left, and the list
    /// keeps those only.
    #[test]
    fn a_spirit_s_ticks_after_it_left_are_not_in_the_list() {
        const BOSS: u32 = 40_031;
        const WIND: u32 = 26_001;
        const OWNER: u32 = 2_700;
        let own = built::stats(OWNER, &[]);
        let tick = built::tick(BOSS, 0x0a, WIND, 2_300, 1_600_110_913, 400, &[F::U32(16_001_112)]);
        let link = Hit {
            target: WIND,
            switch: 0x04,
            actor: OWNER,
            skill: 16_770_001,
            uid: 0x51,
            hit_type: 2,
            effect: 1_677_000_111,
            count: 1,
            values: vec![11_000, 200, 1, 0],
            ..Hit::default()
        };
        let hit = Hit {
            target: BOSS,
            switch: 0x06,
            actor: OWNER,
            skill: 16_330_020,
            uid: 0x52,
            hit_type: 2,
            plotter: Some((0, 0, 1)),
            effect: 1_633_002_011,
            count: 2,
            values: vec![11_000, 2_000, 2, 0],
            ..Hit::default()
        };
        let lines = [
            ("12:00:00.000", link.packet()),
            ("12:00:00.100", own.clone()),
            ("12:00:00.200", own),
            ("12:00:01.200", hit.packet()),
            ("12:00:03.700", tick.clone()),
            ("12:00:04.700", tick.clone()),
            ("12:00:05.700", built::packet([0x42, 0x36], &[F::V(WIND), F::V(0), F::V(7)])),
            ("12:00:05.700", tick.clone()),
            ("12:00:06.700", tick),
        ];
        let fights = timelines(&lines, None);
        assert_eq!(fights.len(), 1);
        assert_eq!(fights[0]["target"], BOSS);
        assert_eq!(ticks(&fights[0]), [(NOON + 3_700, WIND as i64, 400), (NOON + 4_700, WIND as i64, 400)]);
    }

    /// Records with made-up ids: a Melee Training
    /// Scarecrow's spawn, an own record (twice), a hit, three Jointstrike:
    /// Corrode ticks of 150, and the next hit 15 s after the first. A game
    /// record that starts after the first hit (the analyzer restarted)
    /// counts the ticks.
    #[test]
    fn a_fight_takes_its_ticks_from_the_window_start_but_not_the_fight_before_s() {
        const DUMMY: u32 = 26_601;
        const ME: u32 = 4_600;
        let own = built::stats(ME, &[(44, 0)]);
        let tick = built::tick(DUMMY, 0x0a, ME, 8_900, 1_615_000_021, 150, &[F::U32(16_150_010)]);
        let first = Hit {
            target: DUMMY,
            switch: 0x06,
            actor: ME,
            skill: 16_800_001,
            uid: 0x61,
            hit_type: 2,
            plotter: Some((0, 0, 1)),
            effect: 1_680_000_111,
            count: 1,
            values: vec![10_500, 300, 1, 0],
            ..Hit::default()
        };
        let next = Hit {
            switch: 0x36,
            skill: 16_010_000,
            uid: 0x62,
            plotter: Some((0x80, 0, 1)),
            effect: 1_601_000_011,
            values: vec![10_500, 900, 1, 20, 1, 0],
            ..first.clone()
        };
        let lines = [
            ("11:58:20.000", built::scarecrow_spawn(DUMMY)),
            ("11:59:57.000", own.clone()),
            ("11:59:58.000", own),
            ("11:59:58.900", first.packet()),
            ("12:00:00.000", tick.clone()),
            ("12:00:02.000", tick.clone()),
            ("12:00:03.000", tick),
            ("12:00:14.000", next.packet()),
        ];
        let at = |s: i64| NOON + s;
        let me = ME as i64;
        let corrode = [(at(0), me, 150), (at(2_000), me, 150), (at(3_000), me, 150)];
        // From the record's start: one fight, the ticks before its first hit.
        let fights = timelines(&lines, Some("11:59:59.500"));
        assert_eq!(fights.len(), 1);
        assert_eq!((fights[0]["target"].as_i64(), fights[0]["start_ms"].as_i64()), (Some(DUMMY as i64), Some(at(14_000))));
        assert_eq!(ticks(&fights[0]), corrode);
        // The whole capture: the ticks are in the fight before's 15 s, so
        // the next fight does not take them.
        let fights = timelines(&lines, None);
        assert_eq!(fights.len(), 2);
        assert_eq!(ticks(&fights[0]), corrode);
        assert_eq!(ticks(&fights[1]), []);
        // With no fight before, they are within 15 s before the first hit.
        let alone: Vec<(&str, Vec<u8>)> = lines.iter().filter(|l| l.0 != "11:59:58.900").cloned().collect();
        let fights = timelines(&alone, None);
        assert_eq!(fights.len(), 1);
        assert_eq!(ticks(&fights[0]), corrode);
        // With no hit after them they count all the same, as in the game's
        // records.
        let fights = timelines(&lines[..lines.len() - 1], None);
        assert_eq!(fights.len(), 1);
        assert_eq!(ticks(&fights[0]), corrode);
    }

    #[test]
    fn the_tap_keeps_the_message_of_its_own_targets_only() {
        let tap = super::HitTap::default();
        {
            let _on = tap.install_for(&["hit_flags"]);
            tracing::trace!(target: "hit_flags", "1000 a=1");
            tracing::trace!(target: "other", "2000 b=2");
            tracing::trace!(target: "hit_flags", amount = 5);
        }
        let lines = tap.take();
        assert_eq!(
            lines,
            vec![("hit_flags", tracing::Level::TRACE, "1000 a=1".to_string()), ("hit_flags", tracing::Level::TRACE, String::new())]
        );
        assert!(tap.take().is_empty());
    }
}
