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
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

use crate::capture::abnormal::{self, Instance, StatEvent, Timeline};
use crate::entity::summon_resolver;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// The gap that ends a fight, as the encounter mode's default.
const FIGHT_GAP_MS: i64 = 15_000;
/// With no window given, how long before a fight's first hit its DoT ticks
/// count: the same gap, so ticks belong to a fight on either side alike.
const TICK_LEAD_MS: i64 = FIGHT_GAP_MS;

/// The `hit_flags` and `dot_ticks` trace lines, kept instead of printed,
/// each with its target.
#[derive(Clone, Default)]
pub(crate) struct HitTap(Arc<Mutex<Vec<(&'static str, String)>>>);

impl HitTap {
    /// Collect the lines while the guard lives (this thread only).
    pub(crate) fn install(&self) -> tracing::subscriber::DefaultGuard {
        let filter = tracing_subscriber::filter::filter_fn(|m| matches!(m.target(), "hit_flags" | "dot_ticks"));
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone().with_filter(filter)))
    }
    fn take(&self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl<S: tracing::Subscriber> Layer<S> for HitTap {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut m = Message(String::new());
        event.record(&mut m);
        self.0.lock().unwrap().push((event.metadata().target(), m.0));
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
        let table = read("abnormals.json");
        let limits = table["abnormals"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, a)| Some((id.parse().ok()?, a["stacks"].as_u64()? as u32)))
            .collect();
        g.timeline.set_stack_limits(limits);
        g
    }

    fn take_lines(&mut self) {
        for (kind, line) in self.tap.take() {
            let Some(h) = read_hit(&line) else { continue };
            if kind == "dot_ticks" { self.dots.push(h) } else { self.hits.push(h) }
        }
    }

    /// After each capture line: the hits it made, who you are, and the
    /// summon links before a map load makes the entities anew.
    pub(crate) fn after_line(&mut self, ms: i64, local: Option<i32>, links: impl FnOnce() -> HashMap<i32, i32>) {
        self.take_lines();
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
        let friendly = local.contains(&summon_resolver::resolve(h.target, links));
        local.contains(&summon_resolver::resolve(h.actor, links)) && !friendly
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
        let owner = |id: i32| summon_resolver::resolve(id, links);
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
        let target_name = mobs.get(&target).map(|&c| npcs.get_npc_name(c)).unwrap_or_default();
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
            let name = t.skills.first().map(|&s| skills.get_skill_name(s as i32)).unwrap_or_default();
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

    /// Records of 2026-10-06 (the local player is entity 6759): the Fire
    /// Spirit's spawn (its legion name replaced by x's), two own stat
    /// records, Spirit's Benediction on and its stats, two hits on 40171,
    /// the buff and its stats off.
    #[test]
    fn a_fight_lists_the_buffs_and_stats_around_its_hits() {
        let stats_off = "294a36e734044100840300004c00000000007b01340800007c01000000000000000000000000";
        let spirit = "c9014136d1db015f1000b18e2c00400200394b47000f20c700c05b465c5a3943ce8301985598554f0a00004f0a000000\
            0000000000000000000000f837020064000000f04902000100000000000000a08601000000000000e204000101011101\
            40b39809ffffffffffffffff8075d52abb030000e7340d02705c4c47ab1020c79cf25a46070206671a00006c00000000\
            00b1040b787878787878787878787801000200000000000000000000000000000002cd008c000000d000500100002d00\
            0000dd1d030000";
        let lines = [
            ("21:58:28.121", spirit),
            ("21:58:29.300", stats_off),
            ("21:58:29.400", stats_off),
            ("21:58:30.225", "342a38e73401138202e165a6091027000000000000bc5cba14a1010000e73403300af7000047e44c47b46a22c700e85a46"),
            ("21:58:30.225", "294a36e7340441006c0700004c00e80300007b01041000007c01d00700000000000000000000"),
            ("21:58:32.217", "240438ebb9020600e73451c9f9006902000001afa3926101000000e65b8a090100"),
            ("21:58:32.277", "240438ebb9020600e734feb7f800660200000143df276101000000e65bec490100"),
            ("21:58:40.267", "0d2c38e7340100820201"),
            ("21:58:40.267", stats_off),
        ];
        let text: String =
            lines.iter().map(|(t, hex)| format!("2026-10-06T{t}000000-07:00|Client:40000:13328|{hex}\n")).collect();
        let mut out = Vec::new();
        run(&text, Options { timeline: true, ..Default::default() }, &mut |l| out.push(l));
        assert!(out.iter().any(|l| l.trim_start().starts_with("fight target 40171 ") && l.contains("2 hits")), "{out:#?}");
        let json = out.iter().find_map(|l| l.strip_prefix("timeline ")).expect("a timeline line");
        let fight: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(fight["hits"].as_array().map(Vec::len), Some(2));
        let buff = fight["buffs"].as_array().unwrap().iter().find(|b| b["abnormal"] == 161_900_001).expect("the buff");
        let (on, off) = (1_791_349_110_225i64, 1_791_349_120_267i64);
        assert_eq!(
            (buff["on"].as_str(), buff["source"].as_str(), buff["start_ms"].as_i64(), buff["end_ms"].as_i64()),
            (Some("self"), Some("self"), Some(on), Some(off))
        );
        assert_eq!(buff["stacks"], serde_json::json!([[on, 1], [off, 0]]));
        assert_eq!(buff["name"], "Spirit's Benediction");
        assert!(out.iter().any(|l| l.starts_with("buff self 6759 161900001 \"Spirit's Benediction\" via ")), "{out:#?}");
        assert_eq!(fight["stats"]["at_start"]["379"], 4100);
        assert_eq!(fight["stats"]["names"]["379"], "PvE Damage Boost");
        // The spirit's passive from its spawn, at its summon's level.
        let passive = fight["buffs"].as_array().unwrap().iter().find(|b| b["abnormal"] == 161_002_304).expect("the passive");
        assert_eq!(
            (passive["on"].as_str(), passive["entity"].as_i64(), passive["level"].as_i64(), passive["endless"].as_bool()),
            (Some("summon"), Some(28113), Some(13), Some(true))
        );
        assert_eq!(passive["name"], "Fire Spirit");
    }

    /// The timeline JSON of each fight in records of 2026-10-06, replayed
    /// from `from` (time of day) when given.
    fn timelines(lines: &[(&str, &str)], from: Option<&str>) -> Vec<serde_json::Value> {
        let text: String =
            lines.iter().map(|(t, hex)| format!("2026-10-06T{t}000000-07:00|Client:40000:13328|{hex}\n")).collect();
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

    /// Records of 2026-10-06 by the local player 6759, moved onto target
    /// 40171: two own stat records, a hit with two additional hits of 45,
    /// and `more`. The fight's timeline JSON.
    fn fight_of_one_hit(more: &[(&str, &str)]) -> serde_json::Value {
        let own = "294a36e734044100840300004c00000000007b01340800007c01000000000000000000000000";
        let hit = "270438ebb9022600e73430c1f4001202040002cf769b5f01000000e65bf00f022d2d0100";
        let lines = [("21:47:55.000", own), ("21:47:55.100", own), ("21:47:56.170", hit)];
        let lines: Vec<(&str, &str)> = lines.iter().chain(more).copied().collect();
        timelines(&lines, None).into_iter().next().expect("a timeline line")
    }

    #[test]
    fn a_hit_has_its_additional_hits_damage_and_scalar() {
        let fight = fight_of_one_hit(&[]);
        let hit = &fight["hits"][0];
        assert_eq!(
            (hit["damage"].as_i64(), hit["multi"].as_i64(), hit["multi_dmg"].as_i64(), hit["scalar"].as_i64()),
            (Some(1942), Some(2), Some(90), Some(11750))
        );
    }

    /// A Jointstrike: Curse tick (16140000) of 6759's, 10 s and 16 s after
    /// the hit.
    #[test]
    fn a_fight_keeps_its_dot_ticks_apart_from_its_hits() {
        let tick = "170538ebb9020ae73401b3af3360f2056c47f600";
        let fight = fight_of_one_hit(&[("21:48:06.170", tick), ("21:48:12.170", tick)]);
        assert_eq!(fight["hits"].as_array().map(Vec::len), Some(1));
        // The second tick is more than 15 s after the last hit.
        let dots = fight["dots"].as_array().expect("dots");
        assert_eq!(dots.len(), 1, "{dots:?}");
        assert_eq!(
            (dots[0]["actor"].as_i64(), dots[0]["owner"].as_i64(), dots[0]["skill"].as_i64(), dots[0]["damage"].as_i64()),
            (Some(6759), Some(6759), Some(16_140_000), Some(754))
        );
        assert_eq!(dots[0]["ms"].as_i64(), Some(1_791_348_486_170));
    }

    /// Records of 2026-10-06 21:16 (Kernon of the West 48776; the local
    /// player is 15740): the link record to Wind Spirit 25676, an own record
    /// (twice), a hit, then two Malicious Whirlwind ticks, the spirit's
    /// `42 36` flag 7 and two more ticks. The game's record counted the two
    /// before it left, and the list keeps those only.
    #[test]
    fn a_spirit_s_ticks_after_it_left_are_not_in_the_list() {
        let own = "114a36fc7a000000000000000000";
        let tick = "19053888fd020accc801881241c15f5ff7025828f400";
        let lines = [
            ("21:16:20.180", "210438ccc8010400fc7ad1e3ff00eb02affdf46301000000e65be0010100"),
            ("21:16:20.284", own),
            ("21:16:20.384", own),
            ("21:16:21.382", "24043888fd020600fc7a242df900f1020000011ba2556102000000e65bda0f0200"),
            ("21:16:23.871", tick),
            ("21:16:24.881", tick),
            ("21:16:25.876", "0b4236ccc8010007"),
            ("21:16:25.876", tick),
            ("21:16:26.879", tick),
        ];
        let fights = timelines(&lines, None);
        assert_eq!(fights.len(), 1);
        assert_eq!(fights[0]["target"], 48776);
        assert_eq!(ticks(&fights[0]), [(1_791_346_583_871, 25676, 375), (1_791_346_584_881, 25676, 375)]);
    }

    /// Records of 2026-10-06 15:47 (the local player is 4525): Melee
    /// Training Scarecrow 26622's spawn, an own record (twice), a hit, three
    /// Jointstrike: Corrode ticks of 144, and the next hit 15 s after the
    /// first. The game's record of 15:47:17 starts after the first hit (the
    /// player restarted the analyzer) and counts the ticks.
    #[test]
    fn a_fight_takes_its_ticks_from_the_window_start_but_not_the_fight_before_s() {
        let spawn = "94014136fecf01042000239f240040026063f1c7fb7dd5c70016c04600d08942003101d3980694a70764000000640000\
            000000000000000000000000000000000000000000640000000100000000000000000000000000000000000000010601110181\
            969800ffffffffffffffff8075d52abb030000fecf0101006063f1c7fb7dd5c70016c04601000a000000a495f91800";
        let own = "174a36ad23012c00000000000000000000000000";
        let tick = "180538fecf010aad23eb45d5f142609001fa6df600";
        let lines = [
            ("15:45:37.214", spawn),
            ("15:47:14.211", own),
            ("15:47:15.211", own),
            ("15:47:15.815", "240438fecf010600ad230159000172020000016fc4226401000000c052c2020100"),
            ("15:47:16.918", tick),
            ("15:47:18.914", tick),
            ("15:47:19.914", tick),
            ("15:47:30.864", "260438fecf013600ad23104bf40074028000014b526d5f01000000c052a60701140100"),
        ];
        let at = |s: i64| 1_791_326_836_918 + s;
        let corrode = [(at(0), 4525, 144), (at(1996), 4525, 144), (at(2996), 4525, 144)];
        // From the record's start: one fight, the ticks before its first hit.
        let fights = timelines(&lines, Some("15:47:16.500"));
        assert_eq!(fights.len(), 1);
        assert_eq!((fights[0]["target"].as_i64(), fights[0]["start_ms"].as_i64()), (Some(26622), Some(at(13_946))));
        assert_eq!(ticks(&fights[0]), corrode);
        // The whole capture: the ticks are in the fight before's 15 s, so
        // the next fight does not take them.
        let fights = timelines(&lines, None);
        assert_eq!(fights.len(), 2);
        assert_eq!(ticks(&fights[0]), corrode);
        assert_eq!(ticks(&fights[1]), []);
        // With no fight before, they are within 15 s before the first hit.
        let alone: Vec<(&str, &str)> = lines.iter().filter(|l| l.0 != "15:47:15.815").copied().collect();
        let fights = timelines(&alone, None);
        assert_eq!(fights.len(), 1);
        assert_eq!(ticks(&fights[0]), corrode);
        // With no hit after them they count all the same, as in the game's
        // records.
        let fights = timelines(&lines[..lines.len() - 1], None);
        assert_eq!(fights.len(), 1);
        assert_eq!(ticks(&fights[0]), corrode);
    }
}
