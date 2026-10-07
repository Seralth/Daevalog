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
//! summons' DoT ticks on the target (`dot_ticks` lines) apart from the hits,
//! up to 15 s after the last hit.

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
    fn own(&self, h: &Hit, window: (i64, i64), target: Option<i32>) -> bool {
        if h.ms < window.0 || h.ms > window.1 || target.is_some_and(|t| t != h.target) {
            return false;
        }
        let links = self.links_at(h.ms);
        let local = self.local_at(h.ms, h.ms);
        // Heals on you and your spirits are not a fight.
        let friendly = local.contains(&summon_resolver::resolve(h.target, links));
        local.contains(&summon_resolver::resolve(h.actor, links)) && !friendly
    }

    /// Print every fight in `window` (capture ms), only `target`'s if given.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn report(
        mut self,
        zone: chrono::FixedOffset,
        end_ms: i64,
        final_links: HashMap<i32, i32>,
        window: (i64, i64),
        target: Option<i32>,
        mobs: &HashMap<i32, i32>,
        skills: &SkillLookup,
        npcs: &NpcLookup,
        out: &mut dyn FnMut(String),
    ) {
        self.take_lines();
        self.links.push((end_ms, final_links));
        let instances = self.timeline.all_instances();
        let mut fights: Vec<(i32, Vec<&Hit>)> = Vec::new();
        let mut by_target: BTreeMap<i32, Vec<&Hit>> = BTreeMap::new();
        for h in self.hits.iter().filter(|h| self.own(h, window, target)) {
            by_target.entry(h.target).or_default().push(h);
        }
        let dots: Vec<&Hit> = self.dots.iter().filter(|d| self.own(d, window, target)).collect();
        for (t, hits) in by_target {
            let mut cur: Vec<&Hit> = Vec::new();
            for h in hits {
                if cur.last().is_some_and(|l| h.ms - l.ms > FIGHT_GAP_MS) {
                    fights.push((t, std::mem::take(&mut cur)));
                }
                cur.push(h);
            }
            fights.push((t, cur));
        }
        fights.sort_by_key(|(_, h)| h[0].ms);
        out(format!("\n== timeline: {} fights", fights.len()));
        for (t, hits) in fights {
            let (start, end) = (hits[0].ms, hits[hits.len() - 1].ms);
            let fight = self.fight(t, start, end, &hits, &dots, &instances, zone, mobs, skills, npcs);
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
        let dots = dots.iter().filter(|d| d.target == target && d.ms >= start && d.ms <= end + FIGHT_GAP_MS);
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

    /// Records of 2026-10-06 by the local player 6759, moved onto target
    /// 40171: two own stat records, a hit with two additional hits of 45,
    /// and `more`. The fight's timeline JSON.
    fn fight_of_one_hit(more: &[(&str, &str)]) -> serde_json::Value {
        let own = "294a36e734044100840300004c00000000007b01340800007c01000000000000000000000000";
        let hit = "270438ebb9022600e73430c1f4001202040002cf769b5f01000000e65bf00f022d2d0100";
        let lines = [("21:47:55.000", own), ("21:47:55.100", own), ("21:47:56.170", hit)];
        let text: String = lines
            .iter()
            .chain(more)
            .map(|(t, hex)| format!("2026-10-06T{t}000000-07:00|Client:40000:13328|{hex}\n"))
            .collect();
        let mut out = Vec::new();
        run(&text, Options { timeline: true, ..Default::default() }, &mut |l| out.push(l));
        let json = out.iter().find_map(|l| l.strip_prefix("timeline ")).expect("a timeline line");
        serde_json::from_str(json).unwrap()
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
}
