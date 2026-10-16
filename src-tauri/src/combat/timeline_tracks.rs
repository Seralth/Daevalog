//! The buff, debuff and stat tracks of Details' timeline: what storage keeps
//! of the server's abnormal and stat records (`capture::abnormal`) for one
//! player in one fight, cut to the fight and named.
//!
//! - Buffs and debuffs on the player: every timed abnormal on them, whoever
//!   put it on, each under the tab the game shows it under.
//! - Debuffs on the target: every timed abnormal the player or their summons
//!   put on the target.
//! - Stats: yours only (the server sends no one else's), each one that
//!   changed in the fight, as steps from its value at the start.
//!
//! Passives and other abnormals without a timer are left out: they are on
//! for the whole fight. Abnormals of one name share a lane. Times are ms
//! after the fight's start, as the skill lanes' casts.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::capture::abnormal::{End, Instance, StatEvent};

use super::details_view::{DetailsView, MAX_LANES};
use crate::entity::fight_record::FightRecord;

/// The names of abnormals and stats in one language, English where it has
/// none, and for a stat the game's own name where neither has one; the
/// stats the game shows as a percent (`stats.json`), and the abnormals it
/// shows under Debuffs (`abnormals.json`).
#[derive(Debug, Default)]
pub struct TrackNames {
    abnormals: HashMap<u32, String>,
    stats: HashMap<u16, String>,
    percent: HashSet<u16>,
    debuffs: HashSet<u32>,
}

impl TrackNames {
    pub fn load(data_dir: &Path, language: &str) -> TrackNames {
        let read = |file: &str| -> serde_json::Value {
            std::fs::read_to_string(data_dir.join(file)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
        };
        fn table<K: std::str::FromStr + std::hash::Hash + Eq>(into: &mut HashMap<K, String>, value: &serde_json::Value) {
            for (id, name) in value.as_object().into_iter().flatten() {
                if let (Ok(id), Some(name)) = (id.parse(), name.as_str().filter(|n| !n.trim().is_empty())) {
                    into.insert(id, name.to_string());
                }
            }
        }
        let mut names = TrackNames::default();
        let sheet = read("stats.json");
        table(&mut names.stats, &sheet["stats"]);
        names.percent = sheet["percent"].as_array().into_iter().flatten().filter_map(|n| u16::try_from(n.as_u64()?).ok()).collect();
        let marks = read("abnormals.json");
        names.debuffs = marks["abnormals"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, mark)| mark["debuff"] == true)
            .filter_map(|(id, _)| id.parse().ok())
            .collect();
        for language in ["en", language] {
            table(&mut names.abnormals, &read(&format!("i18n/abnormals/{language}.json")));
            table(&mut names.stats, &read(&format!("i18n/stats/{language}.json")));
        }
        names
    }
}

/// A buff or debuff: each time it was up, as [from, to].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpanTrack {
    pub name: String,
    /// "self" for one on the player, "target" for one on the target.
    pub on: &'static str,
    /// The game shows it under Debuffs.
    pub debuff: bool,
    pub spans: Vec<[i64; 2]>,
}

/// A stat: its value at the fight's start, then each change, as [at, value].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatTrack {
    pub name: String,
    /// The game shows it as a percent; its values are hundredths.
    pub percent: bool,
    pub steps: Vec<[i64; 2]>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tracks {
    pub buffs: Vec<SpanTrack>,
    pub stats: Vec<StatTrack>,
}

/// Whose tracks, in which fight.
pub struct Fight<'a> {
    /// The clock time of the fight's start, and its length.
    pub start: i64,
    pub length: i64,
    /// The player's actor ids (every id of one name).
    pub player: &'a HashSet<i32>,
    /// Whether the player is you: only then are there stats.
    pub you: bool,
    pub target: Option<i32>,
}

/// What a saved fight keeps of the server's records for its timeline, so
/// Details can show a fight from History as it showed it live: the timed
/// abnormals on its players and the ones they or their summons put on its
/// target, each as [entity, caster, abnormal, skill or 0, start, end or -1];
/// the summons behind those casters as [summon, owner]; your actor id; and
/// your stats, as the sheet at the fight's start, then each change. Empty in
/// fights saved by older builds. An upload never sends it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedRecords {
    #[serde(default)]
    pub abnormals: Vec<[i64; 6]>,
    #[serde(default)]
    pub summons: Vec<[i32; 2]>,
    #[serde(default)]
    pub you: i32,
    #[serde(default)]
    pub stats: Vec<SavedStats>,
}

/// One stat record of a saved fight: when, and each stat's new value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedStats {
    pub ms: i64,
    pub values: Vec<[i32; 2]>,
}

impl SavedRecords {
    pub fn is_empty(&self) -> bool {
        self.abnormals.is_empty() && self.stats.is_empty()
    }

    /// The records of one fight, `from` to `to`, out of what storage kept:
    /// `players` are its players' actor ids, `owner` maps a caster to whoever
    /// stands behind it, `you` is your actor id (-1 when not known).
    pub fn keep(
        instances: &[Instance],
        stats: &[StatEvent],
        players: &HashSet<i32>,
        target: i32,
        owner: impl Fn(i32) -> i32,
        you: i32,
        from: i64,
    ) -> SavedRecords {
        let mut summons: Vec<[i32; 2]> = Vec::new();
        let mut abnormals = Vec::new();
        for i in instances.iter().filter(|i| !i.endless) {
            let behind = owner(i.caster);
            let kept = players.contains(&i.entity) || (i.entity == target && players.contains(&behind));
            if !kept {
                continue;
            }
            if behind != i.caster && !summons.contains(&[i.caster, behind]) {
                summons.push([i.caster, behind]);
            }
            abnormals.push([
                i.entity as i64,
                i.caster as i64,
                i.abnormal as i64,
                i.skill.unwrap_or(0) as i64,
                i.start_ms,
                i.end_ms.unwrap_or(-1),
            ]);
        }
        let mut saved_stats = Vec::new();
        if players.contains(&you) {
            // Everything before the fight as one sheet, then each change.
            let mut sheet: BTreeMap<u16, i32> = BTreeMap::new();
            for e in stats.iter().filter(|e| e.entity.is_none_or(|id| id == you)) {
                if e.ms <= from {
                    if e.whole_sheet {
                        sheet.clear();
                    }
                    sheet.extend(e.values.iter().copied());
                    continue;
                }
                if saved_stats.is_empty() {
                    saved_stats.push(SavedStats { ms: from, values: sheet.iter().map(|(&s, &v)| [s as i32, v]).collect() });
                }
                saved_stats.push(SavedStats { ms: e.ms, values: e.values.iter().map(|&(s, v)| [s as i32, v]).collect() });
            }
        }
        SavedRecords { abnormals, summons, you, stats: saved_stats }
    }

    /// Back into what `tracks` reads.
    pub fn instances(&self) -> Vec<Instance> {
        self.abnormals
            .iter()
            .map(|&[entity, caster, abnormal, skill, start, end]| Instance {
                entity: entity as i32,
                instance: 0,
                abnormal: abnormal as u32,
                caster: caster as i32,
                skill: (skill > 0).then_some(skill as u32),
                level: 0,
                endless: false,
                start_ms: start,
                end_ms: (end >= 0).then_some(end),
                end: End::Unseen,
                server_end_ms: None,
            })
            .collect()
    }

    /// Back into what `tracks` reads: the first is the sheet at the start.
    pub fn stat_changes(&self) -> Vec<StatEvent> {
        self.stats
            .iter()
            .enumerate()
            .map(|(n, s)| StatEvent {
                ms: s.ms,
                entity: None,
                whole_sheet: n == 0,
                values: s.values.iter().filter_map(|&[stat, v]| Some((u16::try_from(stat).ok()?, v))).collect(),
            })
            .collect()
    }

    /// Whose summon `id` is, as saved; `id` itself when it is no summon.
    pub fn owner(&self, id: i32) -> i32 {
        self.summons.iter().find(|s| s[0] == id).map_or(id, |s| s[1])
    }
}

/// The tracks of `fight` from what storage kept around it. `owner` maps a
/// caster to whoever stands behind it (a summon to its summoner); `skill`
/// names a skill, for an abnormal the tables do not name. An abnormal with
/// neither name is left out.
pub fn tracks(
    fight: &Fight,
    instances: &[Instance],
    stats: &[StatEvent],
    owner: impl Fn(i32) -> i32,
    skill: impl Fn(u32) -> String,
    names: &TrackNames,
) -> Tracks {
    if fight.length <= 0 || fight.player.is_empty() {
        return Tracks::default();
    }
    let end = fight.start + fight.length;
    let on = |i: &Instance| {
        if fight.player.contains(&i.entity) {
            Some("self")
        } else if Some(i.entity) == fight.target && fight.player.contains(&owner(i.caster)) {
            Some("target")
        } else {
            None
        }
    };
    // (on, debuff, name) -> each instance's time, clipped to the fight.
    let mut lanes: BTreeMap<(&'static str, bool, String), Vec<(i64, i64)>> = BTreeMap::new();
    for i in instances.iter().filter(|i| !i.endless) {
        let Some(on) = on(i) else { continue };
        // One added and taken off in the same read was never on.
        let (from, to) = (i.start_ms.max(fight.start), i.end_ms.unwrap_or(end).min(end));
        if i.end_ms == Some(i.start_ms) || to <= from {
            continue;
        }
        // One with no name of its own and no named skill is a system marker
        // the game hides (a stagger alert, a stun break's start): left out.
        let Some(name) = names.abnormals.get(&i.abnormal).cloned().or_else(|| i.skill.map(&skill).filter(|n| !n.is_empty()))
        else {
            continue;
        };
        let debuff = names.debuffs.contains(&i.abnormal);
        lanes.entry((on, debuff, name)).or_default().push((from - fight.start, to - fight.start));
    }
    let mut buffs: Vec<(i64, SpanTrack)> = lanes
        .into_iter()
        .map(|((on, debuff, name), times)| {
            let spans = joined(times);
            let up = spans.iter().map(|s| s[1] - s[0]).sum();
            (up, SpanTrack { name, on, debuff, spans })
        })
        .collect();
    // The player's buffs, then their debuffs, then the target's; each
    // section longest up first, at most MAX_LANES each.
    let section = |t: &SpanTrack| (t.on != "self", t.debuff);
    buffs.sort_by(|a, b| section(&a.1).cmp(&section(&b.1)).then(b.0.cmp(&a.0)).then(a.1.name.cmp(&b.1.name)));
    let mut kept: HashMap<(bool, bool), usize> = HashMap::new();
    let buffs = buffs
        .into_iter()
        .filter(|(_, t)| {
            let n = kept.entry(section(t)).or_default();
            *n += 1;
            *n <= MAX_LANES
        })
        .map(|(_, t)| t)
        .collect();
    let stats = if fight.you { stat_tracks(stats, fight, names) } else { Vec::new() };
    Tracks { buffs, stats }
}

/// The tracks of a saved fight as Details shows it (`view`, from
/// `details_view::saved_view` or `saved_player_view`), from the records it
/// was saved with: the same as the live window showed.
pub fn saved_tracks(record: &FightRecord, view: &DetailsView, skill: impl Fn(u32) -> String, names: &TrackNames) -> Tracks {
    let saved = &record.timeline;
    let player: HashSet<i32> = view.player_ids.iter().copied().collect();
    let fight = Fight {
        start: view.clock_start,
        length: view.chart.battle_ms,
        player: &player,
        you: player.contains(&saved.you),
        target: Some(record.target_id),
    };
    tracks(&fight, &saved.instances(), &saved.stat_changes(), |id| saved.owner(id), skill, names)
}

/// Times that overlap or touch, as one.
fn joined(mut times: Vec<(i64, i64)>) -> Vec<[i64; 2]> {
    times.sort_unstable();
    let mut out: Vec<[i64; 2]> = Vec::new();
    for (from, to) in times {
        match out.last_mut() {
            Some(last) if from <= last[1] => last[1] = last[1].max(to),
            _ => out.push([from, to]),
        }
    }
    out
}

/// Each of your stats that changed in the fight: the sheet at its start,
/// then the changes.
fn stat_tracks(events: &[StatEvent], fight: &Fight, names: &TrackNames) -> Vec<StatTrack> {
    let end = fight.start + fight.length;
    let mut sheet: BTreeMap<u16, i32> = BTreeMap::new();
    let mut steps: BTreeMap<u16, Vec<[i64; 2]>> = BTreeMap::new();
    for e in events.iter().filter(|e| e.entity.is_none_or(|id| fight.player.contains(&id))) {
        if e.ms > end {
            break;
        }
        if e.ms <= fight.start {
            if e.whole_sheet {
                sheet.clear();
            }
            sheet.extend(e.values.iter().copied());
            continue;
        }
        for &(stat, value) in &e.values {
            let list = steps.entry(stat).or_insert_with(|| sheet.get(&stat).map(|&v| vec![[0, v as i64]]).unwrap_or_default());
            if list.last().is_none_or(|s| s[1] != value as i64) {
                list.push([e.ms - fight.start, value as i64]);
            }
        }
    }
    steps
        .into_iter()
        .filter(|(_, s)| s.len() > 1)
        .map(|(stat, steps)| StatTrack {
            name: names.stats.get(&stat).cloned().unwrap_or_else(|| stat.to_string()),
            percent: names.percent.contains(&stat),
            steps,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::abnormal::End;

    fn instance(entity: i32, caster: i32, abnormal: u32, start: i64, end: Option<i64>) -> Instance {
        Instance {
            entity,
            instance: 1,
            abnormal,
            caster,
            skill: None,
            level: 1,
            endless: false,
            start_ms: start,
            end_ms: end,
            end: if end.is_some() { End::Expired } else { End::Open },
            server_end_ms: None,
        }
    }

    fn names() -> TrackNames {
        TrackNames {
            abnormals: HashMap::from([
                (1, "Blessing".to_string()),
                (2, "Curse".to_string()),
                (3, "Blessing".to_string()),
                (4, "Stun".to_string()),
            ]),
            stats: HashMap::from([(128, "Critical Hit".to_string())]),
            percent: HashSet::new(),
            debuffs: HashSet::from([2, 4]),
        }
    }

    const YOU: i32 = 10;
    const SPIRIT: i32 = 11;
    const BOSS: i32 = 99;

    fn fight(player: &HashSet<i32>) -> Fight<'_> {
        Fight { start: 100_000, length: 60_000, player, you: true, target: Some(BOSS) }
    }

    fn owner(id: i32) -> i32 {
        if id == SPIRIT { YOU } else { id }
    }

    #[test]
    fn buffs_on_you_and_your_debuffs_on_the_target_are_cut_to_the_fight() {
        let you = HashSet::from([YOU]);
        let list = [
            // On before the fight, off 5 s in.
            instance(YOU, 50, 1, 90_000, Some(105_000)),
            // The same name from another id, overlapping: one lane, one span.
            instance(YOU, 51, 3, 104_000, Some(110_000)),
            // Your spirit's curse on the target, still on at the end.
            instance(BOSS, SPIRIT, 2, 130_000, None),
            // Another player's curse on the target: not yours.
            instance(BOSS, 77, 2, 110_000, Some(120_000)),
            // Added and taken off in one read.
            instance(YOU, 50, 1, 140_000, Some(140_000)),
        ];
        let t = tracks(&fight(&you), &list, &[], owner, |_| String::new(), &names());
        assert_eq!(
            t.buffs,
            [
                SpanTrack { name: "Blessing".into(), on: "self", debuff: false, spans: vec![[0, 10_000]] },
                SpanTrack { name: "Curse".into(), on: "target", debuff: true, spans: vec![[30_000, 60_000]] },
            ]
        );
    }

    #[test]
    fn a_debuff_on_the_player_has_a_section_of_its_own() {
        let you = HashSet::from([YOU]);
        let list = [
            // The boss's stun on you: shorter than the curse, still first.
            instance(YOU, BOSS, 4, 120_000, Some(122_000)),
            instance(BOSS, YOU, 2, 101_000, Some(150_000)),
            instance(YOU, YOU, 1, 110_000, Some(111_000)),
        ];
        let t = tracks(&fight(&you), &list, &[], owner, |_| String::new(), &names());
        let order: Vec<(&str, &str, bool)> = t.buffs.iter().map(|b| (b.name.as_str(), b.on, b.debuff)).collect();
        assert_eq!(order, [("Blessing", "self", false), ("Stun", "self", true), ("Curse", "target", true)]);
    }

    #[test]
    fn passives_and_unnamed_abnormals() {
        let you = HashSet::from([YOU]);
        let mut passive = instance(YOU, YOU, 1, 0, None);
        passive.endless = true;
        let mut unnamed = instance(YOU, YOU, 500, 101_000, Some(102_000));
        unnamed.skill = Some(7);
        let bare = instance(YOU, YOU, 501, 103_000, Some(104_000));
        let skill = |s: u32| if s == 7 { "Seven".to_string() } else { String::new() };
        let t = tracks(&fight(&you), &[passive, unnamed, bare], &[], owner, skill, &names());
        let names: Vec<&str> = t.buffs.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Seven"], "one with neither name is left out");
    }

    #[test]
    fn stats_that_changed_step_from_their_value_at_the_start() {
        let you = HashSet::from([YOU]);
        let sheet = |ms, values: Vec<(u16, i32)>| StatEvent { ms, entity: None, whole_sheet: true, values };
        let change = |ms, values: Vec<(u16, i32)>| StatEvent { ms, entity: Some(YOU), whole_sheet: false, values };
        let events = [
            sheet(50_000, vec![(128, 600), (52, 900)]),
            change(90_000, vec![(128, 612)]),
            change(110_000, vec![(128, 740), (52, 900)]),
            change(125_000, vec![(128, 612)]),
            change(170_000, vec![(128, 800)]),
        ];
        let t = tracks(&fight(&you), &[], &events, owner, |_| String::new(), &names());
        assert_eq!(t.stats, [StatTrack { name: "Critical Hit".into(), percent: false, steps: vec![[0, 612], [10_000, 740], [25_000, 612]] }]);
        let other = Fight { you: false, ..fight(&you) };
        assert!(tracks(&other, &[], &events, owner, |_| String::new(), &names()).stats.is_empty(), "no one else's stats");
    }

    #[test]
    fn a_saved_fight_keeps_what_its_tracks_need_and_gives_the_same_tracks() {
        let you = HashSet::from([YOU]);
        let mut passive = instance(YOU, YOU, 1, 0, None);
        passive.endless = true;
        let list = [
            instance(YOU, 50, 1, 90_000, Some(105_000)),
            instance(BOSS, SPIRIT, 2, 130_000, None),
            // Another player's curse on the target, and a stranger's buff.
            instance(BOSS, 77, 2, 110_000, Some(120_000)),
            instance(77, 77, 1, 110_000, Some(120_000)),
            passive,
        ];
        let sheet = |ms, values: Vec<(u16, i32)>| StatEvent { ms, entity: None, whole_sheet: true, values };
        let change = |ms, values: Vec<(u16, i32)>| StatEvent { ms, entity: Some(YOU), whole_sheet: false, values };
        let events = [
            sheet(50_000, vec![(128, 600), (52, 900)]),
            change(90_000, vec![(128, 612)]),
            change(110_000, vec![(128, 740)]),
            change(125_000, vec![(128, 612)]),
        ];
        let saved = SavedRecords::keep(&list, &events, &you, BOSS, owner, YOU, 100_000);
        assert_eq!(saved.abnormals.len(), 2, "yours and your spirit's, no passive");
        assert_eq!(saved.summons, [[SPIRIT, YOU]]);
        assert_eq!(saved.stats.len(), 3, "the sheet at the start, then two changes");
        let saved: SavedRecords = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        let live = tracks(&fight(&you), &list, &events, owner, |_| String::new(), &names());
        let again = tracks(&fight(&you), &saved.instances(), &saved.stat_changes(), |id| saved.owner(id), |_| String::new(), &names());
        assert_eq!(again, live);
        assert!(!live.stats.is_empty() && live.buffs.len() == 2);
        let stranger = SavedRecords::keep(&list, &events, &HashSet::from([77]), BOSS, owner, YOU, 100_000);
        assert!(stranger.stats.is_empty(), "only your stats, in a fight you are in");
    }

    #[test]
    fn the_game_data_names_stats_and_marks_the_percents() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
        let names = TrackNames::load(&data, "en");
        assert_eq!(names.stats.get(&44).map(String::as_str), Some("Critical Damage Boost"));
        assert!(names.percent.contains(&44), "Critical Damage Boost is a percent");
        assert!(!names.percent.contains(&128), "Critical Hit is a rating");
        assert_eq!(names.abnormals.get(&161900001).map(String::as_str), Some("Spirit's Benediction"));
        assert!(names.debuffs.contains(&100), "Stun is on the Debuffs tab");
        assert!(!names.debuffs.contains(&161900001), "Spirit's Benediction is a buff");
    }
}
