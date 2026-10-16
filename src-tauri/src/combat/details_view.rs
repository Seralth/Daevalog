//! What Details shows for one player, or every player, in the live fight:
//! the rules the 1.x page applied to the backend's Details answers before
//! drawing them (public/src/js/core.js `getDetails`; details.js
//! `buildCombinedDetails`, `renderStats`, `renderSkills`, `foldDotRows`,
//! `takenRows`, `buildPartyBarStats`, `renderDpsChart`, `renderTimeline`),
//! so a front end shows these numbers as they come.
//!
//! It reads only what 1.x reads: the Details context (`get_details_context`)
//! and the answers to `get_skill_details` and `get_displayed_skill_details`,
//! asked in the same order with the same players. `checks/details_report.rs`
//! prints these answers and the view for each fight of a replay.
//!
//! Every target is one fight, as on the meter: its time counts a boss and
//! its adds fought at once once and leaves out the gaps between pulls, its
//! healing is the context's (each tick once), and its hit times count from
//! the fight's first hit.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::combat::data_storage::UNATTRIBUTED_ID;
use crate::entity::deaths::DeathEntry;
use crate::entity::detail_bundle::{SkillLine, DetailBundle, TargetBreakdown, HealAt, TargetBreakdownReply};
use crate::entity::fight_record::FightRecord;
use crate::entity::taken::TakenSkillEntry;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// Points of the DPS chart after its start (renderDpsChart SAMPLES).
pub const CHART_SAMPLES: usize = 100;
/// Skill lanes of the timeline (renderTimeline).
pub const MAX_LANES: usize = 15;
/// A minimum damage the backend had no hit for.
const NO_MIN: i64 = 2_147_483_647;

/// Whom and which fight Details shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    /// A player by actor id; `None` for every player.
    pub player: Option<i32>,
    /// A target of the live fight; `None` for every target.
    pub target: Option<i32>,
}

/// A backend call 1.x Details makes: `get_skill_details` for one target, or
/// `get_displayed_skill_details` for the targets behind the meter's rows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Call {
    Target { target: i32, actors: Option<Vec<i32>> },
    Displayed { actors: Option<Vec<i32>> },
}

/// Names in the reader's language.
pub trait Names {
    /// The game's name of a skill (`SkillLookup::lookup_skill_name`); empty
    /// when the table has none.
    fn skill(&self, code: i32) -> String;
    /// The game's name of an NPC by its code; empty when the table has none.
    fn npc(&self, code: i32) -> String;
    /// A skill the table does not name ("Skill 1234", `skills.fallback`).
    fn unnamed_skill(&self, code: i32) -> String;
    /// A skill's damage over time row ("Fire - DOT", `skills.dot`).
    fn dot(&self, name: &str) -> String;
}

/// The words Details builds skill names with, in the window's language:
/// `skills.fallback` ("Skill {code}") and `skills.dot` ("{name} - DOT").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Words {
    pub unnamed_skill: String,
    pub dot: String,
}

impl Default for Words {
    fn default() -> Self {
        Self { unnamed_skill: "Skill {code}".into(), dot: "{name} - DOT".into() }
    }
}

/// `Names` from the meter's skill and NPC tables.
pub struct LookupNames<'a> {
    pub skills: &'a SkillLookup,
    pub npcs: &'a NpcLookup,
    pub words: &'a Words,
}

impl Names for LookupNames<'_> {
    fn skill(&self, code: i32) -> String {
        let name = self.skills.lookup_skill_name(code);
        if name.trim().is_empty() { String::new() } else { name }
    }
    fn npc(&self, code: i32) -> String {
        let name = self.npcs.npc_title(code);
        if name.trim().is_empty() { String::new() } else { name }
    }
    fn unnamed_skill(&self, code: i32) -> String {
        self.words.unnamed_skill.replace("{code}", &code.to_string())
    }
    fn dot(&self, name: &str) -> String {
        self.words.dot.replace("{name}", name)
    }
}

/// Who "you" are, as the meter's rows mark you.
#[derive(Debug, Clone, Copy, Default)]
pub struct You<'a> {
    pub id: Option<i64>,
    pub name: &'a str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailsView {
    /// The player picked, 0 for every player.
    pub player: i32,
    /// Every actor id the picked player stands for: all ids with their name.
    pub player_ids: Vec<i32>,
    /// The target picked, 0 for every target.
    pub target: i32,
    /// The players of the live fight: first those with damage on the chosen
    /// targets, most first (1.x's party bars), then the others.
    pub players: Vec<PlayerEntry>,
    /// How many of `players` 1.x lists as party bars (it draws 8 at most).
    pub bars: usize,
    /// Healing per player (1.x's party bars in HEAL), most first.
    pub healers: Vec<HealEntry>,
    /// Damage received per player, most first: TAKEN with that player
    /// picked, every id of one name together.
    pub tanks: Vec<TankEntry>,
    /// The targets the shown players hit, most damage first.
    pub targets: Vec<TargetEntry>,
    /// The target the fight is named after.
    pub title: String,
    pub damage: DamageTab,
    pub heal: HealTab,
    pub taken: TakenTab,
    /// Every player's damage taken by mechanic: who stood in it.
    pub mechanics: Vec<Mechanic>,
    pub chart: Chart,
    /// The healing per second, drawn as the DPS chart draws damage: each
    /// healer's line, the picked player's healing done.
    pub heal_chart: Chart,
    /// The damage received per second, each player's line, the picked
    /// player's damage received.
    pub taken_chart: Chart,
    /// The timeline's skill lanes.
    pub lanes: Vec<Lane>,
    /// The clock time the lanes' casts count from: the shown fight's first
    /// hit. For the buff tracks (`timeline_tracks`), not for the window.
    #[serde(skip)]
    pub clock_start: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerEntry {
    pub id: i32,
    /// The character name; the id as text while the game has given none.
    pub name: String,
    pub identifying: bool,
    pub unattributed: bool,
    /// The backend's class name, or empty.
    pub job: String,
    /// See `PersonalData::number`.
    pub number: u32,
    pub you: bool,
    /// Damage on the chosen targets; healing in `healers`.
    pub amount: i64,
    /// Per second; `None` with no fight time.
    pub per_second: Option<f64>,
    /// Percent of the players' sum.
    pub share: f64,
}

/// A player's healing (`amount`) with the healing skill entries behind it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealEntry {
    #[serde(flatten)]
    pub player: PlayerEntry,
    pub heals: i64,
}

/// A player's damage received (`amount`), per second of the fight and as a
/// share of everyone's, with the attacks, shield blocks and parries behind
/// it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TankEntry {
    #[serde(flatten)]
    pub player: PlayerEntry,
    pub attacks: i64,
    pub shield_block: i64,
    pub parry: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetEntry {
    pub id: i32,
    /// The NPC's name; empty when the tables have none.
    pub name: String,
    /// The shown players' damage on it.
    pub damage: i64,
    /// Percent of their damage on all the targets listed.
    pub share: f64,
}

/// The overview numbers of the DMG tab.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DamageStats {
    pub damage: i64,
    /// Per second of fight time; `None` with no fight time.
    pub per_second: Option<f64>,
    /// Percent of the targets' damage; `None` when 1.x shows "-".
    pub contribution: Option<f64>,
    pub battle_ms: i64,
    /// Direct hits: damage over time ticks are not hits.
    pub hits: i64,
    /// Hits with additional hits, and their rate.
    pub multi_hits: i64,
    pub multi_hit_rate: f64,
    pub multi_hit_damage: i64,
    /// Rates over the direct hits, to one decimal.
    pub crit_rate: f64,
    pub perfect_rate: f64,
    pub double_rate: f64,
    pub back_rate: f64,
    pub parry_rate: f64,
    pub regen: i64,
    pub max_hp: i64,
    /// Damage received and the attacks behind it.
    pub received: Option<Received>,
    /// The deaths of the shown players among you and your party; `None`
    /// when none of them has a count (details.js `deathTotal`).
    pub deaths: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Received {
    pub damage: i64,
    /// Attacks as the game counts them (`TakenStats::total`).
    pub attacks: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DamageTab {
    pub stats: DamageStats,
    /// The same for every player, beside a picked player's (1.x "x / total").
    pub all: Option<DamageStats>,
    pub skills: Vec<SkillRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealStats {
    pub heal: i64,
    /// `None` with no fight time.
    pub per_second: Option<f64>,
    pub battle_ms: i64,
    pub ticks: i64,
    pub hot_ticks: i64,
    /// Healing skill entries, per player and skill.
    pub skills: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealTab {
    pub stats: HealStats,
    pub skills: Vec<SkillRow>,
    /// Whom the shown players' healing went to, most first: the healing each
    /// player received from them, per second of the fight and as a share of
    /// it all.
    pub targets: Vec<PlayerEntry>,
}

/// Whole-percent rates over a row's hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    pub crit: i64,
    pub perfect: i64,
    pub double: i64,
    pub back: i64,
    pub front: i64,
    pub parry: i64,
    pub shield_block: i64,
    pub perfect_block: i64,
    pub iron_wall: i64,
    pub regeneration: i64,
    pub multi_hit: i64,
}

/// A skill row: one skill name, every shown player's casts of it, with its
/// damage over time folded in (HEAL: its heal over time).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRow {
    pub code: i32,
    pub name: String,
    /// A damage over time row with no direct row to go under.
    pub dot: bool,
    /// The backend's class name of the skill, or empty.
    pub job: String,
    /// With the folded damage over time.
    pub amount: i64,
    /// Percent of the tab's total.
    pub share: f64,
    /// Bar length, 0 to 1.
    pub fill: f64,
    /// The direct row's hits (HEAL: ticks).
    pub hits: i64,
    pub crit: i64,
    pub perfect: i64,
    pub double: i64,
    pub back: i64,
    pub front: i64,
    pub parry: i64,
    pub shield_block: i64,
    pub perfect_block: i64,
    pub iron_wall: i64,
    pub regeneration: i64,
    pub miss: i64,
    pub resist: i64,
    pub multi_hits: i64,
    pub rates: Rates,
    pub multi_hit_damage: i64,
    pub regen: i64,
    pub min: i64,
    /// Over the direct hits and the ticks.
    pub avg: i64,
    pub max: i64,
    /// The folded damage over time by its own numbers.
    pub over_time: Option<Box<SkillRow>>,
    /// With `over_time`: the direct hits alone, as 1.x shows a row opened
    /// on its damage over time.
    pub direct: Option<Box<SkillRow>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakenRow {
    pub code: i32,
    pub name: String,
    /// The monster by its NPC code; 0 when it was never seen spawning.
    pub source_code: i32,
    /// Its name; empty when the tables have none.
    pub source: String,
    pub damage: i64,
    pub share: f64,
    /// Attacks as the game counts them.
    pub attacks: i64,
    /// Hits with a value.
    pub hits: i64,
    pub shield_block: i64,
    pub parry: i64,
    pub perfect_block: i64,
    pub iron_wall: i64,
    pub regeneration: i64,
    /// Each hit result in whole percent of the hits; `None` where 1.x leaves
    /// the cell empty: none of them, or no hits.
    pub rates: TakenRates<Option<i64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakenRates<T> {
    pub shield_block: T,
    pub parry: T,
    pub perfect_block: T,
    pub iron_wall: T,
    pub regeneration: T,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub code: i32,
    pub name: String,
    pub damage: i64,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakenTab {
    /// `None` when there is no damage received list.
    pub received: Option<Received>,
    pub battle_ms: i64,
    /// Damage received per second; `None` with no fight time.
    pub per_second: Option<f64>,
    pub hits: i64,
    pub shield_block: i64,
    pub parry: i64,
    pub perfect_block: i64,
    pub iron_wall: i64,
    pub regeneration: i64,
    /// HP that Regeneration hits gave back.
    pub restored: i64,
    /// Each hit result as a rate of the hits, to one decimal.
    pub rates: TakenRates<f64>,
    /// By skill and monster, most damage first.
    pub rows: Vec<TakenRow>,
    /// The hit result columns 1.x shows: those any row has.
    pub results: Vec<&'static str>,
    /// By monster, most damage first.
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mechanic {
    pub code: i32,
    pub name: String,
    pub source_code: i32,
    pub source: String,
    pub damage: i64,
    pub share: f64,
    pub attacks: i64,
    pub hits: i64,
    /// Players it hit or damaged.
    pub players: usize,
    /// Those players, most damage first.
    pub hit: Vec<MechanicHit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MechanicHit {
    pub id: i32,
    pub attacks: i64,
    pub damage: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    /// The span the points cover; 0 when there is no chart.
    pub battle_ms: i64,
    /// Each point is the damage of this long before it, per second.
    pub window_ms: f64,
    /// The picked player's damage per second at `CHART_SAMPLES + 1` points;
    /// empty for every player.
    pub player: Vec<f64>,
    /// Every player's.
    pub party: Vec<f64>,
    /// Each player's, most damage first, as 1.x draws them.
    pub actors: Vec<ActorLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorLine {
    pub id: i32,
    pub job: String,
    pub points: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Lane {
    pub code: i32,
    pub name: String,
    pub dot: bool,
    pub job: String,
    /// Each cast, in ms after its target's first hit.
    pub at: Vec<i64>,
}

/// One damage or healing entry as getDetails kept it: named, and only with
/// damage, a miss or a resist.
#[derive(Debug, Clone, Default)]
struct Skill {
    actor: i32,
    code: i32,
    name: String,
    dot: bool,
    job: String,
    time: i64,
    dmg: i64,
    crit: i64,
    parry: i64,
    back: i64,
    frontal: i64,
    perfect: i64,
    double: i64,
    shield_block: i64,
    iron_wall: i64,
    regeneration: i64,
    perfect_block: i64,
    miss: i64,
    resist: i64,
    regen: i64,
    multi_hit_count: i64,
    multi_hit_damage: i64,
    min: i64,
    max: i64,
    at: Vec<i64>,
}

/// One answer, or several merged, as 1.x Details holds it.
#[derive(Debug, Clone, Default)]
struct Part {
    skills: Vec<Skill>,
    heals: Vec<Skill>,
    taken: Option<Vec<TakenSkillEntry>>,
    deaths: Option<Vec<DeathEntry>>,
    damage: i64,
    hits: i64,
    crit: i64,
    parry: i64,
    back: i64,
    perfect: i64,
    double: i64,
    multi_hit_count: i64,
    multi_hit_damage: i64,
    regen: i64,
    contribution: Option<f64>,
    battle_ms: i64,
    max_hp: i64,
    heal: i64,
    heal_ticks: i64,
    hot_ticks: i64,
    heal_skills: usize,
    heal_per_second: Option<f64>,
    /// The heal ticks of `heals`: (healer, tick), at clock times.
    heal_at: Vec<(i32, HealAt)>,
    /// The clock time the hit times count from: the answer's first hit, or
    /// the first hit of every target merged.
    clock_start: i64,
    /// The answer's first hit, which its hit times count from; 0 when none.
    start: i64,
    /// Every target merged: from the fight's first hit to its last, the
    /// charts' length. 0 for one answer, whose charts run its fight time.
    span_ms: i64,
}

impl Part {
    /// The charts' length (details.js `chartMs`).
    fn chart_ms(&self) -> i64 {
        if self.span_ms > 0 { self.span_ms } else { self.battle_ms }
    }

    /// The healing of these entries (details.js `readHeals`): the players
    /// in `only`, everyone when `None`.
    fn add_heals(&mut self, entries: &[SkillLine], only: Option<&HashSet<i32>>, names: &dyn Names) {
        for entry in entries.iter().filter(|e| only.is_none_or(|ids| ids.contains(&e.actor_id))) {
            if entry.dmg <= 0 {
                continue;
            }
            let skill = Skill::heal(entry, names);
            self.heal_at.extend(entry.heal_ticks.iter().map(|t| (entry.actor_id, *t)));
            self.heal += skill.dmg;
            self.heal_ticks += skill.time;
            if skill.dot {
                self.hot_ticks += skill.time;
            }
            self.heals.push(skill);
        }
        self.heal_skills = self.heals.len();
        self.heal_per_second = (self.battle_ms > 0).then(|| self.heal as f64 / self.battle_ms as f64 * 1000.0);
    }
}

/// The time any of these spans was being fought, gaps between them left out
/// (details.js `activeTime`, the meter's `active_time`).
fn active_time(spans: &[(i64, i64)]) -> i64 {
    let mut spans = spans.to_vec();
    spans.sort_unstable();
    let mut total = 0;
    let mut current: Option<(i64, i64)> = None;
    for (first, last) in spans {
        current = match current {
            Some((start, end)) if first <= end => Some((start, end.max(last))),
            Some((start, end)) => {
                total += end - start;
                Some((first, last))
            }
            None => Some((first, last)),
        };
    }
    total + current.map_or(0, |(start, end)| end - start)
}

/// Each context target's first and last hit (details.js `targetSpans`).
fn target_spans(targets: &[&TargetBreakdown]) -> Vec<(i64, i64)> {
    targets
        .iter()
        .filter(|t| t.target_id > 0 && t.last_damage_time > 0)
        .map(|t| (t.last_damage_time - t.battle_time, t.last_damage_time))
        .collect()
}

/// JS `Math.round` for the values here, which are never negative.
fn round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// A rate to one decimal (getDetails `pct`).
fn rate1(n: i64, of: i64) -> f64 {
    if of > 0 { round(n as f64 / of as f64 * 1000.0) / 10.0 } else { 0.0 }
}

/// A rate in whole percent (renderSkills `pct`).
fn rate0(n: i64, of: i64) -> i64 {
    if of > 0 { round(n as f64 / of as f64 * 100.0) as i64 } else { 0 }
}

fn percent(part: f64, whole: f64) -> f64 {
    if whole > 0.0 { part / whole * 100.0 } else { 0.0 }
}

impl Skill {
    fn damage(entry: &SkillLine, names: &dyn Names) -> Self {
        let mut name = names.skill(entry.code);
        if name.is_empty() {
            name = entry.name.trim().to_string();
        }
        if let Some(rest) = name.strip_prefix("Theostone:") {
            name = format!("Theo:{rest}");
        }
        if name.is_empty() {
            name = names.unnamed_skill(entry.code);
        }
        if entry.is_dot {
            name = names.dot(&name);
        }
        // The backend counts each mark in an i32; the rows add them up in i64.
        let [crit, parry, back, frontal, perfect, double, shield_block, iron_wall, regeneration, perfect_block, miss, resist] = [
            entry.crit,
            entry.parry,
            entry.back,
            entry.frontal,
            entry.perfect,
            entry.double,
            entry.shield_block,
            entry.iron_wall,
            entry.regeneration,
            entry.perfect_block,
            entry.miss,
            entry.resist,
        ]
        .map(i64::from);
        Self {
            actor: entry.actor_id,
            code: entry.code,
            name,
            dot: entry.is_dot,
            job: entry.job.clone(),
            time: entry.time.into(),
            dmg: entry.dmg,
            crit,
            parry,
            back,
            frontal,
            perfect,
            double,
            shield_block,
            iron_wall,
            regeneration,
            perfect_block,
            miss,
            resist,
            regen: entry.regen,
            multi_hit_count: entry.multi_hit_count.into(),
            multi_hit_damage: entry.multi_hit_damage,
            min: entry.min_dmg,
            max: entry.max_dmg,
            at: entry.hit_timestamps.clone(),
        }
    }

    fn heal(entry: &SkillLine, names: &dyn Names) -> Self {
        let mut name = names.skill(entry.code);
        if name.is_empty() {
            name = entry.name.trim().to_string();
        }
        if name.is_empty() {
            name = names.unnamed_skill(entry.code);
        }
        Self {
            actor: entry.actor_id,
            code: entry.code,
            name,
            dot: entry.is_dot,
            job: entry.job.clone(),
            time: entry.time.into(),
            dmg: entry.dmg,
            ..Self::default()
        }
    }
}

/// One backend answer as 1.x's getDetails reads it. `only` keeps those
/// players where the backend did not pick them itself; `of_damage` is the
/// damage the contribution is a share of (none: "-").
fn read(answer: &TargetBreakdownReply, only: Option<&HashSet<i32>>, of_damage: i64, names: &dyn Names) -> Part {
    let kept = |actor: i32| only.is_none_or(|ids| ids.contains(&actor));
    let mut part = Part::default();
    for entry in answer.skills.iter().filter(|e| kept(e.actor_id)) {
        // A skill that only missed or was resisted still gets its row.
        if entry.dmg <= 0 && entry.miss <= 0 && entry.resist <= 0 {
            continue;
        }
        let skill = Skill::damage(entry, names);
        part.damage += skill.dmg;
        part.regen += skill.regen;
        if !skill.dot {
            part.hits += skill.time;
            part.crit += skill.crit;
            part.parry += skill.parry;
            part.back += skill.back;
            part.perfect += skill.perfect;
            part.double += skill.double;
            part.multi_hit_count += skill.multi_hit_count;
            part.multi_hit_damage += skill.multi_hit_damage;
        }
        part.skills.push(skill);
    }
    part.contribution = (of_damage > 0).then(|| part.damage as f64 / of_damage as f64 * 100.0);
    part.battle_ms = answer.battle_time;
    part.start = answer.start_time;
    part.clock_start = answer.start_time;
    part.max_hp = answer.max_hp.into();
    part.add_heals(&answer.heal_skills, only, names);
    part.taken = Some(answer.taken_skills.iter().filter(|e| kept(e.actor_id)).cloned().collect());
    part.deaths = answer.deaths.as_ref().map(|list| list.iter().filter(|e| kept(e.actor_id)).copied().collect());
    part
}

/// The shared parts of every target's view: the context's spans of all the
/// fight's targets, and its healing and damage received, each counted once.
struct Fight<'a> {
    spans: Vec<(i64, i64)>,
    heals: &'a [SkillLine],
    taken: &'a [TakenSkillEntry],
    deaths: Option<&'a [DeathEntry]>,
    /// The context's fight time, as the meter counts it (0: none).
    battle_ms: i64,
}

/// Several targets' answers as one fight (buildCombinedDetails): its time is
/// the time any target was fought, its hit times count from its first hit,
/// and its healing and damage received are the context's, of the players in
/// `only` (everyone when `None`).
fn combine(parts: Vec<Part>, of_damage: i64, fight: &Fight, only: Option<&HashSet<i32>>, names: &dyn Names) -> Part {
    let mut spans = fight.spans.clone();
    spans.extend(parts.iter().filter(|p| p.start > 0).map(|p| (p.start, p.start + p.battle_ms)));
    let start = spans.iter().map(|s| s.0).min().unwrap_or(0);
    let mut out = Part::default();
    for part in parts {
        out.max_hp = out.max_hp.max(part.max_hp);
        // Each answer counts its hit times from its own target's first hit.
        let shift = if part.start > 0 { part.start - start } else { 0 };
        for mut skill in part.skills {
            if shift != 0 {
                for at in &mut skill.at {
                    *at += shift;
                }
            }
            out.damage += skill.dmg;
            out.regen += skill.regen;
            out.multi_hit_count += skill.multi_hit_count;
            out.multi_hit_damage += skill.multi_hit_damage;
            if !skill.dot {
                out.hits += skill.time;
                out.crit += skill.crit;
                out.parry += skill.parry;
                out.back += skill.back;
                out.perfect += skill.perfect;
                out.double += skill.double;
            }
            out.skills.push(skill);
        }
    }
    // The fight's time as the meter counts it, else the time any target was
    // fought (details.js `fightTimeMs`).
    out.battle_ms = if fight.battle_ms > 0 { fight.battle_ms } else { active_time(&spans) };
    out.span_ms = spans.iter().map(|s| s.1).max().map_or(0, |end| end - start);
    out.clock_start = start;
    out.contribution = Some(percent(out.damage as f64, of_damage as f64));
    out.add_heals(fight.heals, only, names);
    out.taken = Some(fight.taken.iter().filter(|e| only.is_none_or(|ids| ids.contains(&e.actor_id))).cloned().collect());
    out.deaths = fight.deaths.map(|list| list.iter().filter(|e| only.is_none_or(|ids| ids.contains(&e.actor_id))).copied().collect());
    out
}

/// Damage received and the attacks behind it (details.js `takenTotals`).
fn received(taken: Option<&[TakenSkillEntry]>) -> Option<Received> {
    taken.map(|entries| Received {
        damage: entries.iter().map(|e| e.stats.damage).sum(),
        attacks: entries.iter().map(|e| i64::from(e.stats.total())).sum(),
    })
}

fn damage_stats(part: &Part) -> DamageStats {
    DamageStats {
        damage: part.damage,
        per_second: (part.battle_ms > 0).then(|| part.damage as f64 / part.battle_ms as f64 * 1000.0),
        contribution: part.contribution,
        battle_ms: part.battle_ms,
        hits: part.hits,
        multi_hits: part.multi_hit_count,
        multi_hit_rate: rate1(part.multi_hit_count, part.hits),
        multi_hit_damage: part.multi_hit_damage,
        crit_rate: rate1(part.crit, part.hits),
        perfect_rate: rate1(part.perfect, part.hits),
        double_rate: rate1(part.double, part.hits),
        back_rate: rate1(part.back, part.hits),
        parry_rate: rate1(part.parry, part.hits),
        regen: part.regen,
        max_hp: part.max_hp,
        received: received(part.taken.as_deref()),
        deaths: part.deaths.as_ref().filter(|list| !list.is_empty()).map(|list| list.iter().map(|e| i64::from(e.deaths)).sum()),
    }
}

/// A row of entries of one name, folded and measured (renderSkills).
#[derive(Debug, Clone)]
struct Group {
    skill: Skill,
    over_time: Option<Skill>,
}

impl Group {
    fn combined(&self) -> i64 {
        self.skill.dmg + self.over_time.as_ref().map_or(0, |d| d.dmg)
    }
}

/// Adds each named count of `$from` into the same count of `$into`.
macro_rules! add_counts {
    ($into:expr, $from:expr; $($count:ident),+ $(,)?) => {
        $($into.$count += $from.$count;)+
    };
}

/// Entries of one name and kind summed, in the order first seen.
fn group(skills: &[Skill]) -> Vec<Skill> {
    let mut order: Vec<Skill> = Vec::new();
    let mut at: HashMap<(&str, bool), usize> = HashMap::new();
    for skill in skills {
        let Some(&i) = at.get(&(skill.name.as_str(), skill.dot)) else {
            at.insert((skill.name.as_str(), skill.dot), order.len());
            order.push(skill.clone());
            continue;
        };
        let row = &mut order[i];
        if row.actor != skill.actor {
            row.actor = 0;
        }
        if row.job.is_empty() {
            row.job.clone_from(&skill.job);
        }
        add_counts!(row, skill;
            time, dmg, regen, multi_hit_count, multi_hit_damage, miss, resist,
            crit, perfect, double, back, frontal, parry,
            shield_block, perfect_block, iron_wall, regeneration,
        );
        row.min = if row.min > 0 && skill.min > 0 { row.min.min(skill.min) } else if row.min > 0 { row.min } else { skill.min };
        row.max = row.max.max(skill.max);
    }
    order
}

/// Each damage over time row under the direct row of its skill code, never
/// matched by name (foldDotRows); one with no direct row stays, after them.
fn fold(rows: Vec<Skill>) -> Vec<Group> {
    let (dots, direct): (Vec<Skill>, Vec<Skill>) = rows.into_iter().partition(|r| r.dot);
    let mut dots: Vec<Option<Skill>> = dots.into_iter().map(Some).collect();
    let mut out: Vec<Group> = direct
        .into_iter()
        .map(|skill| {
            let over_time = dots.iter_mut().find(|d| d.as_ref().is_some_and(|d| d.code == skill.code)).and_then(Option::take);
            Group { skill, over_time }
        })
        .collect();
    out.extend(dots.into_iter().flatten().map(|skill| Group { skill, over_time: None }));
    out
}

fn min_hit(min: i64) -> i64 {
    if min >= NO_MIN { 0 } else { min }
}

fn rates(s: &Skill, hits: i64) -> Rates {
    Rates {
        crit: rate0(s.crit, hits),
        perfect: rate0(s.perfect, hits),
        double: rate0(s.double, hits),
        back: rate0(s.back, hits),
        front: rate0(s.frontal, hits),
        parry: rate0(s.parry, hits),
        shield_block: rate0(s.shield_block, hits),
        perfect_block: rate0(s.perfect_block, hits),
        iron_wall: rate0(s.iron_wall, hits),
        regeneration: rate0(s.regeneration, hits),
        multi_hit: rate0(s.multi_hit_count, hits),
    }
}

/// A row's numbers: a direct row with its damage over time shown together,
/// as 1.x shows a row that is not opened.
fn measure(s: &Skill, over_time: Option<&Skill>, base: f64) -> SkillRow {
    let amount = s.dmg + over_time.map_or(0, |d| d.dmg);
    let (min_a, min_b) = (min_hit(s.min), over_time.map_or(0, |d| min_hit(d.min)));
    let min = if over_time.is_some() && min_a > 0 && min_b > 0 { min_a.min(min_b) } else if min_a > 0 { min_a } else { min_b };
    let max = s.max.max(over_time.map_or(0, |d| d.max));
    let all_hits = s.time + over_time.map_or(0, |d| d.time);
    // The direct row's own counts go to the row as they are.
    let &Skill {
        time: hits,
        frontal: front,
        multi_hit_count: multi_hits,
        crit,
        perfect,
        double,
        back,
        parry,
        shield_block,
        perfect_block,
        iron_wall,
        regeneration,
        miss,
        resist,
        multi_hit_damage,
        regen,
        ..
    } = s;
    SkillRow {
        code: s.code,
        name: s.name.clone(),
        dot: s.dot,
        job: s.job.clone(),
        amount,
        share: percent(amount as f64, base),
        fill: if base > 0.0 { (amount as f64 / base).clamp(0.0, 1.0) } else { 0.0 },
        hits,
        crit,
        perfect,
        double,
        back,
        front,
        parry,
        shield_block,
        perfect_block,
        iron_wall,
        regeneration,
        miss,
        resist,
        multi_hits,
        rates: rates(s, hits),
        multi_hit_damage,
        regen,
        min,
        avg: if all_hits > 0 { round(amount as f64 / all_hits as f64) as i64 } else { 0 },
        max,
        over_time: over_time.map(|d| Box::new(measure(d, None, base))),
        direct: over_time.map(|_| Box::new(measure(s, None, base))),
    }
}

/// The skill table's rows, most damage first. `total` is what the shares
/// are of; with none, the rows' own sum (HEAL).
fn skill_rows(skills: &[Skill], total: Option<i64>) -> Vec<SkillRow> {
    let mut groups = fold(group(skills));
    // Sorted by damage with its damage over time, then by its own damage.
    groups.sort_by(|a, b| b.combined().cmp(&a.combined()).then(b.skill.dmg.cmp(&a.skill.dmg)));
    let sum: i64 = groups.iter().map(Group::combined).sum();
    let base = match total {
        Some(t) if t > 0 => t,
        _ => sum,
    } as f64;
    groups.iter().map(|g| measure(&g.skill, g.over_time.as_ref(), base)).collect()
}

/// The hit results a player turns a monster's hit into, as 1.x names its
/// damage received columns.
const TAKEN_RESULTS: [&str; 5] = ["shieldBlock", "parry", "perfectBlock", "ironWall", "regeneration"];

fn taken_name(entry: &TakenSkillEntry, names: &dyn Names) -> String {
    let name = names.skill(entry.code);
    if !name.is_empty() {
        return name;
    }
    if !entry.name.is_empty() {
        return entry.name.clone();
    }
    names.unnamed_skill(entry.code)
}

/// Damage received added up per skill name and monster, most damage first
/// (details.js `takenRows`), with who took it.
fn taken_groups<'a>(entries: &'a [TakenSkillEntry]) -> Vec<(&'a TakenSkillEntry, Vec<&'a TakenSkillEntry>)> {
    let mut order: Vec<(&TakenSkillEntry, Vec<&TakenSkillEntry>)> = Vec::new();
    let mut at: HashMap<(String, i32), usize> = HashMap::new();
    for e in entries {
        let key = (if e.name.is_empty() { e.code.to_string() } else { e.name.clone() }, e.source_code);
        match at.get(&key) {
            Some(&i) => order[i].1.push(e),
            None => {
                at.insert(key, order.len());
                order.push((e, vec![e]));
            }
        }
    }
    let damage = |list: &[&TakenSkillEntry]| list.iter().map(|e| e.stats.damage).sum::<i64>();
    order.sort_by(|a, b| damage(&b.1).cmp(&damage(&a.1)).then(a.0.code.cmp(&b.0.code)));
    order
}

fn source_name(code: i32, names: &dyn Names) -> String {
    if code > 0 { names.npc(code) } else { String::new() }
}

fn taken_tab(taken: Option<&[TakenSkillEntry]>, battle_ms: i64, names: &dyn Names) -> TakenTab {
    let entries = taken.unwrap_or_default();
    let sum = |f: fn(&TakenSkillEntry) -> i64| entries.iter().map(f).sum::<i64>();
    let total = sum(|e| e.stats.damage);
    let rows: Vec<TakenRow> = taken_groups(entries)
        .into_iter()
        .map(|(first, list)| {
            let sum = |f: fn(&TakenSkillEntry) -> i64| list.iter().map(|e| f(e)).sum::<i64>();
            let damage = sum(|e| e.stats.damage);
            TakenRow {
                code: first.code,
                name: taken_name(first, names),
                source_code: first.source_code,
                source: source_name(first.source_code, names),
                damage,
                share: percent(damage as f64, total as f64),
                attacks: sum(|e| e.stats.total().into()),
                hits: sum(|e| e.stats.hits.into()),
                shield_block: sum(|e| e.stats.shield_block.into()),
                parry: sum(|e| e.stats.parry.into()),
                perfect_block: sum(|e| e.stats.perfect_block.into()),
                iron_wall: sum(|e| e.stats.iron_wall.into()),
                regeneration: sum(|e| e.stats.regeneration.into()),
                rates: TakenRates::default(),
            }
        })
        .map(|mut row: TakenRow| {
            let of = |n: i64| (n > 0 && row.hits > 0).then(|| rate0(n, row.hits));
            row.rates = TakenRates {
                shield_block: of(row.shield_block),
                parry: of(row.parry),
                perfect_block: of(row.perfect_block),
                iron_wall: of(row.iron_wall),
                regeneration: of(row.regeneration),
            };
            row
        })
        .collect();
    let has = |f: fn(&TakenRow) -> i64| rows.iter().any(|r| f(r) > 0);
    let results: Vec<&'static str> = TAKEN_RESULTS
        .into_iter()
        .zip([
            has(|r| r.shield_block),
            has(|r| r.parry),
            has(|r| r.perfect_block),
            has(|r| r.iron_wall),
            has(|r| r.regeneration),
        ])
        .filter_map(|(name, shown)| shown.then_some(name))
        .collect();
    let mut by_source: Vec<Source> = Vec::new();
    for row in &rows {
        match by_source.iter_mut().find(|s| s.code == row.source_code) {
            Some(s) => s.damage += row.damage,
            None => by_source.push(Source { code: row.source_code, name: row.source.clone(), damage: row.damage, share: 0.0 }),
        }
    }
    by_source.sort_by(|a, b| b.damage.cmp(&a.damage).then(a.code.cmp(&b.code)));
    for s in &mut by_source {
        s.share = percent(s.damage as f64, total as f64);
    }
    let hits = sum(|e| e.stats.hits.into());
    let counts = TakenRates {
        shield_block: sum(|e| e.stats.shield_block.into()),
        parry: sum(|e| e.stats.parry.into()),
        perfect_block: sum(|e| e.stats.perfect_block.into()),
        iron_wall: sum(|e| e.stats.iron_wall.into()),
        regeneration: sum(|e| e.stats.regeneration.into()),
    };
    TakenTab {
        received: received(taken),
        battle_ms,
        per_second: (battle_ms > 0).then(|| total as f64 / battle_ms as f64 * 1000.0),
        hits,
        shield_block: counts.shield_block,
        parry: counts.parry,
        perfect_block: counts.perfect_block,
        iron_wall: counts.iron_wall,
        regeneration: counts.regeneration,
        restored: sum(|e| e.stats.restored),
        rates: TakenRates {
            shield_block: rate1(counts.shield_block, hits),
            parry: rate1(counts.parry, hits),
            perfect_block: rate1(counts.perfect_block, hits),
            iron_wall: rate1(counts.iron_wall, hits),
            regeneration: rate1(counts.regeneration, hits),
        },
        rows,
        results,
        sources: by_source,
    }
}

/// The players of the live fight: first those with damage on `target`, or
/// on every target in `live`, most first (1.x's party bars, the count
/// returned), then the others. Per second of the fight's time over those
/// targets, else of `shown_ms`.
fn players(ctx: &DetailBundle, target: Option<i32>, live: &[&TargetBreakdown], shown_ms: i64, you: &You) -> (Vec<PlayerEntry>, usize) {
    let (bar_targets, every_target): (Vec<&TargetBreakdown>, bool) = match target.and_then(|t| target_by_id(ctx, t)) {
        Some(t) => (vec![t], false),
        None => (live.to_vec(), true),
    };
    let mut by_damage: Vec<(i32, i64)> = Vec::new();
    // The fight's time over these targets, as the overview counts it.
    let mut bar_ms = active_time(&target_spans(&bar_targets));
    // On every target, the fight's time as the meter counts it.
    if every_target && ctx.battle_time > 0 {
        bar_ms = ctx.battle_time;
    }
    for t in &bar_targets {
        let mut actors: Vec<(i32, i64)> = t.actor_damage.iter().map(|(&id, &d)| (id, d)).filter(|&(id, _)| id > 0).collect();
        // The page read these from a JSON object, which lists number keys in order.
        actors.sort_by_key(|a| a.0);
        for (id, damage) in actors {
            match by_damage.iter_mut().find(|e| e.0 == id) {
                Some(e) => e.1 += damage,
                None => by_damage.push((id, damage)),
            }
        }
    }
    by_damage.sort_by(|a, b| b.1.cmp(&a.1));
    if bar_ms <= 0 {
        bar_ms = shown_ms;
    }
    let damage_sum: i64 = by_damage.iter().map(|e| e.1).sum();
    let damage_sum = if damage_sum > 0 { damage_sum } else { 1 };
    let mut players: Vec<PlayerEntry> = by_damage
        .iter()
        .map(|&(id, damage)| {
            let per_second = (bar_ms > 0).then(|| damage as f64 / bar_ms as f64 * 1000.0);
            player_entry(ctx, id, you, damage, per_second, damage as f64 / damage_sum as f64 * 100.0)
        })
        .collect();
    let bars = players.len();
    for actor in &ctx.actors {
        if actor.actor_id > 0 && !players.iter().any(|p| p.id == actor.actor_id) {
            players.push(player_entry(ctx, actor.actor_id, you, 0, None, 0.0));
        }
    }
    (players, bars)
}

/// Healing per player, from everyone's healing (1.x's HEAL party bars),
/// most first, per second of `battle_ms`.
fn healers(ctx: &DetailBundle, heals: &[Skill], battle_ms: i64, you: &You) -> Vec<HealEntry> {
    // Each player's id, healing and healing skill entries.
    let mut by_heal: Vec<(i32, i64, i64)> = Vec::new();
    for heal in heals.iter().filter(|h| h.actor > 0) {
        match by_heal.iter_mut().find(|e| e.0 == heal.actor) {
            Some(e) => {
                e.1 += heal.dmg;
                e.2 += 1;
            }
            None => by_heal.push((heal.actor, heal.dmg, 1)),
        }
    }
    by_heal.sort_by(|a, b| b.1.cmp(&a.1));
    let heal_sum: i64 = by_heal.iter().map(|e| e.1).sum();
    let heal_sum = if heal_sum > 0 { heal_sum } else { 1 };
    by_heal
        .iter()
        .map(|&(id, heal, heals)| {
            let per_second = (battle_ms > 0).then(|| heal as f64 / battle_ms as f64 * 1000.0);
            HealEntry { player: player_entry(ctx, id, you, heal, per_second, heal as f64 / heal_sum as f64 * 100.0), heals }
        })
        .collect()
}

/// The skills the meter's hover card lists.
pub const HOVER_SKILLS: usize = 5;

/// One skill of the meter's hover card.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoverSkill {
    pub name: String,
    /// The backend's class name, or empty.
    pub job: String,
    pub dot: bool,
    pub damage: i64,
}

/// The meter's hover card (core.js `fetchHoverTooltip`): the row's biggest
/// skill entries in `answer`, its actor's on the targets on screen, each
/// entry on its own as 1.x lists them (damage over time apart), most damage
/// first; and all the entries' damage.
pub fn hover(answer: &TargetBreakdownReply, names: &dyn Names) -> (Vec<HoverSkill>, i64) {
    let part = read(answer, None, 0, names);
    let mut skills: Vec<HoverSkill> = part
        .skills
        .iter()
        .map(|s| HoverSkill { name: s.name.clone(), job: s.job.clone(), dot: s.dot, damage: s.dmg })
        .collect();
    skills.sort_by(|a, b| b.damage.cmp(&a.damage));
    skills.truncate(HOVER_SKILLS);
    (skills, part.damage)
}

/// What the meter's Heal and Tank tabs take from Details on every player.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyView {
    pub players: Vec<PlayerEntry>,
    pub healers: Vec<HealEntry>,
    pub tanks: Vec<TankEntry>,
    /// The fight's time.
    pub battle_ms: i64,
}

/// `build`'s players, healers and damage received per player with no player
/// picked, from one call at most: on every target the healing and damage
/// received are the context's, so no target's answer is needed for them.
/// `displayed`: the meter's own targets (ENC), read as the meter's rows are,
/// over every one of them; `target` is then not used.
pub fn party_view(ctx: &DetailBundle, target: Option<i32>, displayed: Option<&[i32]>, you: &You, names: &dyn Names, ask: &mut dyn FnMut(&Call) -> TargetBreakdownReply) -> PartyView {
    let live: Vec<&TargetBreakdown> = ctx.targets.iter()
        .filter(|t| t.target_id > 0 && displayed.is_none_or(|ids| ids.contains(&t.target_id)))
        .collect();
    let target = if displayed.is_some() { None } else { target };
    let all = match target {
        Some(target) => {
            let of = target_by_id(ctx, target).map_or(0, |t| t.total_damage);
            read(&ask(&Call::Target { target, actors: None }), None, of, names)
        }
        None if live.is_empty() || displayed.is_some() => read(&ask(&Call::Displayed { actors: None }), None, 0, names),
        None => {
            let fight = Fight { spans: target_spans(&live), heals: &ctx.heal_skills, taken: &ctx.taken_skills, deaths: ctx.deaths.as_deref(), battle_ms: ctx.battle_time };
            combine(Vec::new(), 0, &fight, None, names)
        }
    };
    let (players, _) = players(ctx, target, &live, all.battle_ms, you);
    PartyView {
        players,
        healers: healers(ctx, &all.heals, all.battle_ms, you),
        tanks: tanks(ctx, all.taken.as_deref().unwrap_or_default(), all.battle_ms, you),
        battle_ms: all.battle_ms,
    }
}

/// Each player's damage received, from everyone's entries: what TAKEN shows
/// with that player picked, so every id of one name together
/// (`same_name_ids`), most first, per second of `battle_ms`.
fn tanks(ctx: &DetailBundle, taken: &[TakenSkillEntry], battle_ms: i64, you: &You) -> Vec<TankEntry> {
    // The ids of one player, then damage, attacks, shield blocks, parries.
    let mut by_player: Vec<(Vec<i32>, [i64; 4])> = Vec::new();
    for entry in taken.iter().filter(|e| e.actor_id > 0) {
        let at = match by_player.iter().position(|(ids, _)| ids.contains(&entry.actor_id)) {
            Some(at) => at,
            None => {
                by_player.push((same_name_ids(ctx, entry.actor_id), [0; 4]));
                by_player.len() - 1
            }
        };
        let sums = &mut by_player[at].1;
        sums[0] += entry.stats.damage;
        sums[1] += i64::from(entry.stats.total());
        sums[2] += i64::from(entry.stats.shield_block);
        sums[3] += i64::from(entry.stats.parry);
    }
    let everyone: i64 = by_player.iter().map(|(_, sums)| sums[0]).sum();
    let mut tanks: Vec<TankEntry> = by_player
        .into_iter()
        .map(|(ids, [damage, attacks, shield_block, parry])| {
            let id = ids.iter().copied().min().unwrap_or(0);
            let per_second = (battle_ms > 0).then(|| damage as f64 / battle_ms as f64 * 1000.0);
            TankEntry {
                player: player_entry(ctx, id, you, damage, per_second, percent(damage as f64, everyone as f64)),
                attacks,
                shield_block,
                parry,
            }
        })
        .collect();
    tanks.sort_by(|a, b| b.player.amount.cmp(&a.player.amount).then(a.player.id.cmp(&b.player.id)));
    tanks
}

/// Every player's damage taken per mechanic, and whom each one hit.
fn mechanics(taken: &[TakenSkillEntry], names: &dyn Names) -> Vec<Mechanic> {
    let total: i64 = taken.iter().map(|e| e.stats.damage).sum();
    taken_groups(taken)
        .into_iter()
        .map(|(first, list)| {
            let mut hit: Vec<MechanicHit> = Vec::new();
            for e in &list {
                let attacks = i64::from(e.stats.total());
                match hit.iter_mut().find(|h| h.id == e.actor_id) {
                    Some(h) => {
                        h.attacks += attacks;
                        h.damage += e.stats.damage;
                    }
                    None => hit.push(MechanicHit { id: e.actor_id, attacks, damage: e.stats.damage }),
                }
            }
            hit.retain(|h| h.attacks > 0 || h.damage > 0);
            hit.sort_by(|a, b| b.damage.cmp(&a.damage).then(b.attacks.cmp(&a.attacks)).then(a.id.cmp(&b.id)));
            let damage = list.iter().map(|e| e.stats.damage).sum::<i64>();
            Mechanic {
                code: first.code,
                name: taken_name(first, names),
                source_code: first.source_code,
                source: source_name(first.source_code, names),
                damage,
                share: percent(damage as f64, total as f64),
                attacks: list.iter().map(|e| i64::from(e.stats.total())).sum(),
                hits: list.iter().map(|e| i64::from(e.stats.hits)).sum(),
                players: hit.len(),
                hit,
            }
        })
        .collect()
}

/// Every actor's hits as (time, damage), each skill's damage spread evenly
/// over its hits; actors most damage first (renderDpsChart).
fn actor_hits(skills: &[Skill]) -> Vec<(i32, String, Vec<(i64, f64)>)> {
    let mut order: Vec<(i32, String, Vec<(i64, f64)>)> = Vec::new();
    for skill in skills.iter().filter(|s| s.actor > 0) {
        let i = match order.iter().position(|a| a.0 == skill.actor) {
            Some(i) => i,
            None => {
                order.push((skill.actor, String::new(), Vec::new()));
                order.len() - 1
            }
        };
        let per_hit = if skill.at.is_empty() { 0.0 } else { skill.dmg as f64 / skill.at.len() as f64 };
        order[i].2.extend(skill.at.iter().map(|&t| (t, per_hit)));
    }
    // Hits at one time in a fixed order, and equal totals by id: the entries
    // come in a hash map's order, and floats added in another order differ.
    for actor in &mut order {
        actor.2.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    }
    let total = |hits: &[(i64, f64)]| hits.iter().fold(0.0, |sum, h| sum + h.1);
    order.sort_by(|a, b| total(&b.2).partial_cmp(&total(&a.2)).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    order
}

/// Damage per second over the window before each point.
fn sample(hits: &[(i64, f64)], battle_ms: i64, window_ms: f64) -> Vec<f64> {
    (0..=CHART_SAMPLES)
        .map(|i| {
            let t = i as f64 / CHART_SAMPLES as f64 * battle_ms as f64;
            let lo = t - window_ms;
            let mut total = 0.0;
            for &(at, damage) in hits {
                let at = at as f64;
                if at > t {
                    break;
                }
                if at > lo {
                    total += damage;
                }
            }
            total / (window_ms / 1000.0)
        })
        .collect()
}

fn chart(all: &Part, shown: &Part, picked: Option<&HashSet<i32>>, jobs: &HashMap<i32, String>) -> Chart {
    let battle_ms = if all.chart_ms() > 0 { all.chart_ms() } else { shown.chart_ms() };
    let empty = Chart { battle_ms: 0, window_ms: 0.0, player: Vec::new(), party: Vec::new(), actors: Vec::new() };
    if battle_ms <= 0 || all.skills.is_empty() {
        return empty;
    }
    let hits: Vec<(i32, Vec<(i64, f64)>)> = actor_hits(&all.skills).into_iter().map(|(id, _, h)| (id, h)).collect();
    line_chart(battle_ms, &hits, picked, jobs)
}

/// A chart of each player's (time, amount) list, most first, over
/// `battle_ms`: each player's amount per second over the window before each
/// point (renderDpsChart), everyone's summed, and the picked players'.
fn line_chart(battle_ms: i64, lines: &[(i32, Vec<(i64, f64)>)], picked: Option<&HashSet<i32>>, jobs: &HashMap<i32, String>) -> Chart {
    if battle_ms <= 0 || lines.is_empty() {
        return Chart { battle_ms: 0, window_ms: 0.0, player: Vec::new(), party: Vec::new(), actors: Vec::new() };
    }
    // About a tenth of the fight, 5 to 15 s.
    let window_ms = (battle_ms as f64 * 0.1).clamp(5_000.0, 15_000.0);
    let actors: Vec<ActorLine> = lines
        .iter()
        .map(|(id, h)| ActorLine { id: *id, job: jobs.get(id).cloned().unwrap_or_default(), points: sample(h, battle_ms, window_ms) })
        .collect();
    let party = add_lines(actors.iter());
    let player = match picked {
        Some(ids) => add_lines(actors.iter().filter(|a| ids.contains(&a.id))),
        None => Vec::new(),
    };
    Chart { battle_ms, window_ms, player, party, actors }
}

/// Each player's amounts at their times from `start`, oldest first, the
/// players most first.
fn player_lines(amounts: impl Iterator<Item = (i32, i64, i64)>, start: i64) -> Vec<(i32, Vec<(i64, f64)>)> {
    let mut lines: Vec<(i32, Vec<(i64, f64)>)> = Vec::new();
    for (id, at, amount) in amounts {
        let point = (at - start, amount as f64);
        match lines.iter_mut().find(|l| l.0 == id) {
            Some(line) => line.1.push(point),
            None => lines.push((id, vec![point])),
        }
    }
    for line in &mut lines {
        line.1.sort_by_key(|p| p.0);
    }
    let total = |points: &[(i64, f64)]| points.iter().fold(0.0, |sum, p| sum + p.1);
    lines.sort_by(|a, b| total(&b.1).partial_cmp(&total(&a.1)).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    lines
}

/// The healing per second: each healer's ticks in `all`.
fn heal_chart(all: &Part, shown: &Part, picked: Option<&HashSet<i32>>, jobs: &HashMap<i32, String>) -> Chart {
    let battle_ms = if all.chart_ms() > 0 { all.chart_ms() } else { shown.chart_ms() };
    let lines = player_lines(all.heal_at.iter().map(|(healer, t)| (*healer, t.at, t.amount)), all.clock_start);
    line_chart(battle_ms, &lines, picked, jobs)
}

/// The damage received per second: each player's hits taken in `all`.
fn taken_chart(all: &Part, shown: &Part, picked: Option<&HashSet<i32>>, jobs: &HashMap<i32, String>) -> Chart {
    let battle_ms = if all.chart_ms() > 0 { all.chart_ms() } else { shown.chart_ms() };
    let hits = all.taken.iter().flatten().flat_map(|e| e.hit_times.iter().map(move |&(at, damage)| (e.actor_id, at, damage)));
    line_chart(battle_ms, &player_lines(hits, all.clock_start), picked, jobs)
}

/// Whom the shown players' healing went to (`HealTab::targets`).
fn heal_targets(ctx: &DetailBundle, shown: &Part, you: &You) -> Vec<PlayerEntry> {
    let mut by_target: Vec<(i32, i64)> = Vec::new();
    for (_, t) in &shown.heal_at {
        match by_target.iter_mut().find(|e| e.0 == t.target) {
            Some(e) => e.1 += t.amount,
            None => by_target.push((t.target, t.amount)),
        }
    }
    by_target.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let sum: i64 = by_target.iter().map(|e| e.1).sum();
    by_target
        .iter()
        .map(|&(id, heal)| {
            let per_second = (shown.battle_ms > 0).then(|| heal as f64 / shown.battle_ms as f64 * 1000.0);
            player_entry(ctx, id, you, heal, per_second, percent(heal as f64, sum as f64))
        })
        .collect()
}

fn add_lines<'a>(lines: impl Iterator<Item = &'a ActorLine>) -> Vec<f64> {
    let mut sum = vec![0.0; CHART_SAMPLES + 1];
    for line in lines {
        for (s, v) in sum.iter_mut().zip(&line.points) {
            *s += v;
        }
    }
    sum
}

/// The timeline's lanes: each skill name's casts, most damage first, then
/// most casts, then by name, at most 15 (renderTimeline). The page took the
/// damage of a name's first entry, and the entries come in a hash map's
/// order: a lane moved, or dropped out, from one read to the next.
fn lanes(skills: &[Skill], all_ms: i64, shown_ms: i64) -> Vec<Lane> {
    let battle_ms = if all_ms > 0 { all_ms } else { shown_ms };
    if battle_ms <= 0 {
        return Vec::new();
    }
    let mut order: Vec<(i64, Lane)> = Vec::new();
    for skill in skills.iter().filter(|s| !s.at.is_empty()) {
        match order.iter_mut().find(|(_, l)| l.name == skill.name && l.dot == skill.dot) {
            Some((dmg, lane)) => {
                *dmg += skill.dmg;
                lane.at.extend_from_slice(&skill.at);
            }
            None => order.push((
                skill.dmg,
                Lane { code: skill.code, name: skill.name.clone(), dot: skill.dot, job: skill.job.clone(), at: skill.at.clone() },
            )),
        }
    }
    order.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.at.len().cmp(&a.1.at.len()))
            .then(a.1.name.cmp(&b.1.name))
            .then(a.1.dot.cmp(&b.1.dot))
    });
    order
        .into_iter()
        .take(MAX_LANES)
        .map(|(_, mut lane)| {
            lane.at.sort_unstable();
            lane
        })
        .collect()
}

fn target_by_id(ctx: &DetailBundle, id: i32) -> Option<&TargetBreakdown> {
    ctx.targets.iter().find(|t| t.target_id == id)
}

fn target_name(t: &TargetBreakdown, names: &dyn Names) -> String {
    let name = if t.mob_code > 0 { names.npc(t.mob_code) } else { String::new() };
    if name.is_empty() { t.target_name.trim().to_string() } else { name }
}

/// The actor ids a pick stands for: every actor of the same name, as 1.x
/// matched its rows to the context by name; the id alone when none.
pub fn same_name_ids(ctx: &DetailBundle, id: i32) -> Vec<i32> {
    let named = ctx.actors.iter().find(|a| a.actor_id == id).map(|a| a.nickname.trim()).filter(|n| !n.is_empty() && *n != id.to_string());
    let Some(name) = named.filter(|_| id != UNATTRIBUTED_ID) else { return vec![id] };
    let name = name.to_lowercase();
    let ids: Vec<i32> = ctx.actors.iter().filter(|a| a.actor_id > 0 && a.nickname.trim().to_lowercase() == name).map(|a| a.actor_id).collect();
    if ids.is_empty() { vec![id] } else { ids }
}

/// The target Details opens on from the meter (core.js
/// `getDefaultDetailsOpenOptions`, details.js `open`): the meter's target,
/// every target where the mode has none, or the fight the context names.
pub fn opening_target(ctx: &DetailBundle, mode: &str, meter_target: i32, meter_target_named: bool, train_all: bool) -> Option<i32> {
    let concrete = meter_target > 0;
    let train = mode == "trainTargets";
    let all = (mode == "lastHitByMe" && !concrete && !meter_target_named)
        || (train && train_all)
        || (!concrete && mode == "allTargets");
    if all {
        return None;
    }
    if concrete {
        return Some(meter_target);
    }
    if ctx.current_target_id != 0 {
        return Some(ctx.current_target_id);
    }
    ctx.targets.first().map(|t| t.target_id)
}

/// A player of the context, named and numbered, with an amount.
pub fn player_entry(ctx: &DetailBundle, id: i32, you: &You, amount: i64, per_second: Option<f64>, share: f64) -> PlayerEntry {
    let actor = ctx.actors.iter().find(|a| a.actor_id == id);
    let name = actor.map_or_else(|| id.to_string(), |a| a.nickname.clone());
    let unattributed = id == UNATTRIBUTED_ID;
    PlayerEntry {
        id,
        identifying: !unattributed && (name.trim().is_empty() || name == id.to_string()),
        unattributed,
        job: actor.map(|a| a.job.clone()).unwrap_or_default(),
        number: ctx.numbers.get(&id).copied().unwrap_or(0),
        you: (!unattributed && !you.name.is_empty() && name == you.name) || you.id == Some(i64::from(id)),
        name,
        amount,
        per_second,
        share,
    }
}

/// Builds what Details shows from the Details context and the answers to the
/// calls 1.x makes, asked through `ask` in 1.x's order.
pub fn build(
    ctx: &DetailBundle,
    selection: &Selection,
    you: &You,
    names: &dyn Names,
    ask: &mut dyn FnMut(&Call) -> TargetBreakdownReply,
) -> DetailsView {
    let ids: Option<Vec<i32>> = selection.player.map(|id| same_name_ids(ctx, id));
    let picked: Option<HashSet<i32>> = ids.as_ref().map(|ids| ids.iter().copied().collect());
    let row_ids = selection.player.map(|id| vec![id]);
    let live: Vec<&TargetBreakdown> = ctx.targets.iter().filter(|t| t.target_id > 0).collect();
    let damage_on = |t: &TargetBreakdown, id: i32| t.actor_damage.get(&id).copied().unwrap_or(0);

    // The shown players' numbers, then everyone's.
    let (shown, all) = match selection.target {
        Some(target) => {
            let of = target_by_id(ctx, target).map_or(0, |t| t.total_damage);
            let shown = read(&ask(&Call::Target { target, actors: ids.clone() }), None, of, names);
            let all = match ids {
                Some(_) => read(&ask(&Call::Target { target, actors: None }), None, of, names),
                None => shown.clone(),
            };
            (shown, all)
        }
        None => {
            let selectable: Vec<&TargetBreakdown> = live
                .iter()
                .copied()
                .filter(|t| picked.as_ref().is_none_or(|p| p.iter().any(|&id| damage_on(t, id) > 0)))
                .collect();
            let fight = Fight { spans: target_spans(&live), heals: &ctx.heal_skills, taken: &ctx.taken_skills, deaths: ctx.deaths.as_deref(), battle_ms: ctx.battle_time };
            let shown = if selectable.is_empty() {
                read(&ask(&Call::Displayed { actors: row_ids.clone() }), picked.as_ref(), 0, names)
            } else {
                let parts = selectable
                    .iter()
                    .map(|t| read(&ask(&Call::Target { target: t.target_id, actors: ids.clone() }), None, t.total_damage, names))
                    .collect();
                combine(parts, selectable.iter().map(|t| t.total_damage).sum(), &fight, picked.as_ref(), names)
            };
            let all = match ids {
                None => shown.clone(),
                Some(_) if live.is_empty() => read(&ask(&Call::Displayed { actors: row_ids.clone() }), None, 0, names),
                Some(_) => {
                    let parts = live
                        .iter()
                        .map(|t| read(&ask(&Call::Target { target: t.target_id, actors: None }), None, t.total_damage, names))
                        .collect();
                    combine(parts, live.iter().map(|t| t.total_damage).sum(), &fight, None, names)
                }
            };
            (shown, all)
        }
    };

    let (players, bars) = players(ctx, selection.target, &live, shown.battle_ms, you);
    let healers = healers(ctx, &all.heals, shown.battle_ms, you);

    // The targets the shown players hit.
    let mut targets: Vec<TargetEntry> = live
        .iter()
        .map(|t| TargetEntry {
            id: t.target_id,
            name: target_name(t, names),
            damage: match &picked {
                Some(p) => p.iter().map(|&id| damage_on(t, id)).sum(),
                None => t.actor_damage.iter().filter(|(id, _)| **id > 0).map(|(_, d)| d).sum(),
            },
            share: 0.0,
        })
        .filter(|t| t.damage > 0)
        .collect();
    targets.sort_by(|a, b| b.damage.cmp(&a.damage).then(a.id.cmp(&b.id)));
    let target_sum: i64 = targets.iter().map(|t| t.damage).sum();
    for t in &mut targets {
        t.share = percent(t.damage as f64, target_sum as f64);
    }
    // The picked target, else the one the meter follows, else the one that
    // took the most damage, the lower id on a tie (details.js `titleTarget`).
    let title = selection
        .target
        .and_then(|id| target_by_id(ctx, id))
        .or_else(|| target_by_id(ctx, ctx.current_target_id).filter(|t| t.target_id > 0))
        .or_else(|| live.iter().copied().min_by(|a, b| b.total_damage.cmp(&a.total_damage).then(a.target_id.cmp(&b.target_id))))
        .map(|t| target_name(t, names))
        .unwrap_or_default();

    let jobs: HashMap<i32, String> = ctx.actors.iter().map(|a| (a.actor_id, a.job.clone())).collect();
    let everyone_taken = all.taken.as_deref().unwrap_or_default();
    let tanks = tanks(ctx, everyone_taken, all.battle_ms, you);
    DetailsView {
        player: selection.player.unwrap_or(0),
        player_ids: ids.clone().unwrap_or_default(),
        target: selection.target.unwrap_or(0),
        players,
        bars,
        healers,
        tanks,
        targets,
        title,
        damage: DamageTab {
            stats: damage_stats(&shown),
            all: ids.as_ref().map(|_| damage_stats(&all)),
            skills: skill_rows(&shown.skills, Some(shown.damage)),
        },
        heal: HealTab {
            stats: HealStats {
                heal: shown.heal,
                per_second: shown.heal_per_second,
                battle_ms: shown.battle_ms,
                ticks: shown.heal_ticks,
                hot_ticks: shown.hot_ticks,
                skills: shown.heal_skills,
            },
            skills: skill_rows(&shown.heals, None),
            targets: heal_targets(ctx, &shown, you),
        },
        taken: taken_tab(shown.taken.as_deref(), shown.battle_ms, names),
        mechanics: mechanics(everyone_taken, names),
        chart: chart(&all, &shown, picked.as_ref(), &jobs),
        heal_chart: heal_chart(&all, &shown, picked.as_ref(), &jobs),
        taken_chart: taken_chart(&all, &shown, picked.as_ref(), &jobs),
        lanes: lanes(&shown.skills, all.chart_ms(), shown.chart_ms()),
        clock_start: shown.clock_start,
    }
}

/// Each player's damage in a saved fight by actor id, from its saved skill
/// entries (details.js `openHistoryFight`).
pub fn saved_actor_damage(record: &FightRecord) -> HashMap<i32, i64> {
    let mut damage: HashMap<i32, i64> = HashMap::new();
    for skill in record.details.skills.iter().filter(|s| s.actor_id > 0) {
        *damage.entry(skill.actor_id).or_default() += skill.dmg;
    }
    damage
}

/// Whether a saved fight's actor is you: a saved fight's ids are from its
/// own session, so only your name counts (core.js `isUser` with `saved`).
pub fn saved_is_you(nickname: &str, you: &str) -> bool {
    !nickname.is_empty() && nickname.trim() == you.trim()
}

/// A saved fight's players numbered as 1.x numbers them (players.js
/// `savedFightNumbers`): by their first hit, then by id. You and the
/// unattributed row get none.
pub fn saved_numbers(record: &FightRecord, you: &str) -> HashMap<i32, u32> {
    let mut first: HashMap<i32, i64> = HashMap::new();
    let mut note = |id: i32, at: i64| {
        if id <= 0 || id == UNATTRIBUTED_ID {
            return;
        }
        let e = first.entry(id).or_insert(i64::MAX);
        *e = (*e).min(at);
    };
    let details = &record.details;
    for skill in details.skills.iter().chain(&details.heal_skills) {
        note(skill.actor_id, skill.hit_timestamps.iter().copied().min().unwrap_or(i64::MAX));
    }
    for actor in &record.actors {
        note(actor.actor_id, i64::MAX);
    }
    let is_you = |id: i32| record.actors.iter().find(|a| a.actor_id == id).is_some_and(|a| saved_is_you(&a.nickname, you));
    let mut order: Vec<(i32, i64)> = first.into_iter().filter(|&(id, _)| !is_you(id)).collect();
    order.sort_by_key(|&(id, at)| (at, id));
    order.into_iter().enumerate().map(|(i, (id, _))| (id, i as u32 + 1)).collect()
}

/// The Details context 1.x builds for a saved fight (details.js
/// `openHistoryFight`): its one target, each player's damage on it, and its
/// players numbered as `saved_numbers` numbers them.
pub fn saved_context(record: &FightRecord, you: &str) -> DetailBundle {
    DetailBundle {
        current_target_id: record.target_id,
        targets: vec![TargetBreakdown {
            target_id: record.target_id,
            target_name: record.boss_name.clone(),
            mob_code: record.mob_code,
            max_hp: 0,
            battle_time: record.duration_ms,
            last_damage_time: record.start_time_ms + record.duration_ms,
            total_damage: record.total_damage,
            actor_damage: saved_actor_damage(record),
        }],
        actors: record.actors.clone(),
        numbers: saved_numbers(record, you),
        taken_skills: Vec::new(),
        heal_skills: Vec::new(),
        deaths: None,
        // 1.x's context for a saved fight has no fight time of its own.
        battle_time: 0,
    }
}

/// What Details shows of a saved fight for every player: its saved answer
/// on its one target, as 1.x opens a fight from History.
pub fn saved_view(record: &FightRecord, you: &str, names: &dyn Names) -> DetailsView {
    let ctx = saved_context(record, you);
    let selection = Selection { player: None, target: Some(record.target_id) };
    build(&ctx, &selection, &You { id: None, name: you.trim() }, names, &mut |_| record.details.clone())
}

/// One player of a saved fight, as Details shows them with their bar
/// picked: the saved answer filtered to the player's ids (all of one name),
/// as 1.x filters a saved fight (core.js `getDetails`).
pub fn saved_player_view(record: &FightRecord, you: &str, player: i32, names: &dyn Names) -> DetailsView {
    let ctx = saved_context(record, you);
    let selection = Selection { player: Some(player), target: Some(record.target_id) };
    build(&ctx, &selection, &You { id: None, name: you.trim() }, names, &mut |call| saved_answer(record, call))
}

/// A saved fight's answer to one of 1.x's Details calls: every entry, or the
/// asked actors'.
fn saved_answer(record: &FightRecord, call: &Call) -> TargetBreakdownReply {
    let mut answer = record.details.clone();
    let (Call::Target { actors, .. } | Call::Displayed { actors }) = call;
    if let Some(ids) = actors {
        answer.skills.retain(|e| ids.contains(&e.actor_id));
        answer.heal_skills.retain(|e| ids.contains(&e.actor_id));
        answer.taken_skills.retain(|e| ids.contains(&e.actor_id));
        if let Some(deaths) = answer.deaths.as_mut() {
            deaths.retain(|e| ids.contains(&e.actor_id));
        }
    }
    answer
}

/// The overview of a saved fight for the actors in `ids`: its saved answer
/// filtered to them, as 1.x filters a saved fight when a player's bar is
/// picked (core.js `getDetails`).
pub fn saved_overview(record: &FightRecord, ids: &[i32], names: &dyn Names) -> (DamageStats, TakenTab) {
    let only: HashSet<i32> = ids.iter().copied().collect();
    let part = read(&record.details, Some(&only), record.total_damage, names);
    (damage_stats(&part), taken_tab(part.taken.as_deref(), part.battle_ms, names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::detail_bundle::ActorBreakdown;
    use crate::entity::taken::TakenStats;

    struct English;

    impl Names for English {
        fn skill(&self, code: i32) -> String {
            match code {
                100 => "Fire".into(),
                200 => "Ice".into(),
                300 => "Theostone: Spark".into(),
                900 => "Heal".into(),
                1800 => "Slam".into(),
                _ => String::new(),
            }
        }
        fn npc(&self, code: i32) -> String {
            if code == 77 { "Boss".into() } else { String::new() }
        }
        fn unnamed_skill(&self, code: i32) -> String {
            format!("Skill {code}")
        }
        fn dot(&self, name: &str) -> String {
            format!("{name} - DOT")
        }
    }

    fn target(id: i32, battle: i64, damage: &[(i32, i64)]) -> TargetBreakdown {
        TargetBreakdown {
            target_id: id,
            target_name: String::new(),
            mob_code: 0,
            max_hp: 0,
            battle_time: battle,
            last_damage_time: battle,
            total_damage: damage.iter().map(|d| d.1).sum(),
            actor_damage: damage.iter().copied().collect(),
        }
    }

    fn context(targets: Vec<TargetBreakdown>, actors: Vec<ActorBreakdown>) -> DetailBundle {
        DetailBundle { current_target_id: 0, targets, actors, numbers: HashMap::new(), taken_skills: Vec::new(), heal_skills: Vec::new(), deaths: None, battle_time: 0 }
    }

    fn taken(actor: i32, code: i32, source: i32, damage: i64, hits: i32) -> TakenSkillEntry {
        TakenSkillEntry {
            actor_id: actor,
            code,
            name: String::new(),
            source_code: source,
            stats: TakenStats { damage, hits, ..TakenStats::default() },
            hit_times: Vec::new(),
        }
    }

    /// `time` hits a second apart from 0, each `dmg / time`, unnamed.
    fn entry(actor: i32, code: i32, dot: bool, dmg: i64, time: i32) -> SkillLine {
        let each = if dmg > 0 { dmg / i64::from(time.max(1)) } else { 0 };
        SkillLine {
            actor_id: actor,
            code,
            name: String::new(),
            time,
            dmg,
            multi_hit_count: 0,
            multi_hit_damage: 0,
            multi_hit_hits: 0,
            min_dmg: each,
            max_dmg: each,
            crit: 0,
            shield_block: 0,
            parry: 0,
            back: 0,
            frontal: 0,
            perfect: 0,
            double: 0,
            iron_wall: 0,
            regeneration: 0,
            perfect_block: 0,
            miss: 0,
            resist: 0,
            regen: 0,
            job: String::new(),
            is_dot: dot,
            hit_timestamps: (0..i64::from(time)).map(|n| n * 1_000).collect(),
            specs: Vec::new(),
            heal_ticks: Vec::new(),
        }
    }

    /// `target`'s answer of these skills over `battle` ms.
    fn answer(target: i32, battle: i64, skills: Vec<SkillLine>) -> TargetBreakdownReply {
        TargetBreakdownReply {
            target_id: target,
            max_hp: 0,
            total_target_damage: skills.iter().map(|s| s.dmg).sum(),
            battle_time: battle,
            start_time: 0,
            skills,
            ping_history: Vec::new(),
            heal_skills: Vec::new(),
            taken_skills: Vec::new(),
            deaths: None,
        }
    }

    fn actor(id: i32, name: &str) -> ActorBreakdown {
        ActorBreakdown {
            actor_id: id,
            nickname: name.to_string(),
            job: String::new(),
            job_id: 0,
            party_heal: 0,
            regen: 0,
            damage_received: 0,
            hits_received: 0,
            dbid: 0,
            server_id: 0,
            level: 0,
            gear_score: 0,
            combat_power: 0,
        }
    }

    #[test]
    fn a_damage_over_time_row_goes_under_its_skill_and_counts_in_its_damage() {
        let skills: Vec<Skill> = [entry(1, 100, false, 600, 3), entry(1, 100, true, 400, 4), entry(1, 200, false, 900, 1)]
            .iter()
            .map(|e| Skill::damage(e, &English))
            .collect();
        let rows = skill_rows(&skills, Some(1900));
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Fire", "Ice"]);
        let fire = &rows[0];
        assert_eq!((fire.amount, fire.hits, fire.avg), (1000, 3, 143), "avg over hits and ticks: 1000 / 7");
        let dot = fire.over_time.as_ref().expect("the DoT under Fire");
        assert_eq!((dot.name.as_str(), dot.amount, dot.hits), ("Fire - DOT", 400, 4));
        assert!((fire.share - 1000.0 / 1900.0 * 100.0).abs() < 1e-9);
        let direct = fire.direct.as_ref().expect("Fire's direct hits, for the opened row");
        assert_eq!((direct.amount, direct.hits, direct.avg), (600, 3, 200));
        assert!(direct.over_time.is_none() && rows[1].direct.is_none(), "only a row with damage over time has one");
    }

    #[test]
    fn rows_of_one_name_add_up_over_players_and_rates_are_whole_percents() {
        let mut a = entry(1, 100, false, 500, 3);
        a.crit = 1;
        let mut b = entry(2, 100, false, 700, 4);
        b.crit = 2;
        b.min_dmg = 50;
        let skills: Vec<Skill> = [a, b].iter().map(|e| Skill::damage(e, &English)).collect();
        let rows = skill_rows(&skills, None);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].hits, rows[0].crit, rows[0].rates.crit, rows[0].min), (7, 3, 43, 50));
    }

    #[test]
    fn names_follow_the_page() {
        let theo = Skill::damage(&entry(1, 300, false, 10, 1), &English);
        assert_eq!(theo.name, "Theo: Spark");
        let unknown = Skill::damage(&entry(1, 555, true, 10, 1), &English);
        assert_eq!(unknown.name, "Skill 555 - DOT");
        // A skill that only missed keeps its row; one with nothing goes.
        let mut missed = entry(1, 200, false, 0, 0);
        missed.miss = 2;
        let part = read(&answer(9, 1000, vec![missed, entry(1, 100, false, 0, 0)]), None, 0, &English);
        assert_eq!(part.skills.len(), 1);
        assert_eq!(part.contribution, None);
    }

    #[test]
    fn every_target_adds_up_as_one_fight() {
        // A boss from 1,000 to 1,010 s with an add from 1,002 to 1,007 s,
        // then the next pull from 1,020 to 1,023 s.
        let mut boss = target(7, 10_000, &[(1, 600), (2, 400)]);
        boss.last_damage_time = 1_010_000;
        let mut add = target(8, 5_000, &[(1, 300)]);
        add.last_damage_time = 1_007_000;
        let mut next = target(9, 3_000, &[(2, 100)]);
        next.last_damage_time = 1_023_000;
        let mut ctx = context(vec![boss, add, next], vec![actor(1, "Ann"), actor(2, "Bo")]);
        ctx.heal_skills = vec![entry(1, 900, false, 500, 2), entry(2, 900, false, 300, 1)];
        let at = |start: i64, battle: i64, skills: Vec<SkillLine>| {
            let mut a = answer(0, battle, skills);
            a.start_time = start;
            // Each target's own healing: the view takes the context's.
            a.heal_skills = vec![entry(1, 900, false, 400, 1)];
            a
        };
        let mut calls = Vec::new();
        let view = build(&ctx, &Selection { player: Some(1), target: None }, &You::default(), &English, &mut |call| {
            calls.push(call.clone());
            match call {
                Call::Target { target: 7, actors: Some(_) } => at(1_000_000, 10_000, vec![entry(1, 100, false, 600, 6)]),
                Call::Target { target: 7, actors: None } => at(1_000_000, 10_000, vec![entry(1, 100, false, 600, 6), entry(2, 200, false, 400, 2)]),
                Call::Target { target: 8, .. } => at(1_002_000, 5_000, vec![entry(1, 100, false, 300, 3)]),
                Call::Target { target: 9, .. } => at(1_020_000, 3_000, vec![entry(2, 200, false, 100, 1)]),
                _ => unreachable!(),
            }
        });
        // The picked player's targets, then every target for the totals.
        assert_eq!(calls, [
            Call::Target { target: 7, actors: Some(vec![1]) },
            Call::Target { target: 8, actors: Some(vec![1]) },
            Call::Target { target: 7, actors: None },
            Call::Target { target: 8, actors: None },
            Call::Target { target: 9, actors: None },
        ]);
        let stats = &view.damage.stats;
        // 10 s of the boss, its add inside it, and 3 s of the next pull.
        assert_eq!((stats.damage, stats.hits, stats.battle_ms), (900, 9, 13_000));
        assert!((stats.contribution.unwrap() - 900.0 / 1300.0 * 100.0).abs() < 1e-9);
        assert_eq!(view.damage.all.as_ref().unwrap().damage, 1400);
        assert_eq!(view.players.iter().map(|p| (p.id, p.amount)).collect::<Vec<_>>(), [(1, 900), (2, 500)]);
        assert_eq!(view.players[0].per_second, Some(900.0 / 13_000.0 * 1000.0), "over the fight's time");
        assert_eq!(view.targets.iter().map(|t| (t.id, t.damage)).collect::<Vec<_>>(), [(7, 600), (8, 300)]);
        // The fight's healing, the picked player's in the overview.
        assert_eq!((view.heal.stats.heal, view.heal.stats.ticks, view.heal.stats.battle_ms), (500, 2, 13_000));
        assert_eq!(view.healers.iter().map(|h| (h.player.id, h.player.amount, h.heals)).collect::<Vec<_>>(), [(1, 500, 1), (2, 300, 1)]);
        // Hit times from the fight's first hit; the charts run to its last.
        assert_eq!(view.chart.battle_ms, 23_000);
        assert_eq!(view.lanes.len(), 1);
        assert_eq!(view.lanes[0].at, [0, 1_000, 2_000, 2_000, 3_000, 3_000, 4_000, 4_000, 5_000]);
        let everyone = build(&ctx, &Selection::default(), &You::default(), &English, &mut |call| match call {
            Call::Target { target: 9, .. } => at(1_020_000, 3_000, vec![entry(2, 200, false, 100, 1)]),
            Call::Target { target: 7, .. } => at(1_000_000, 10_000, vec![entry(2, 200, false, 400, 2)]),
            _ => at(1_002_000, 5_000, vec![]),
        });
        let ice = everyone.lanes.iter().find(|l| l.name == "Ice").unwrap();
        assert_eq!(ice.at, [0, 1_000, 20_000], "the next pull's hit at 20 s");
    }

    #[test]
    fn lanes_are_in_one_order_whatever_order_the_entries_come_in() {
        // The entries come in a hash map's order. Fire: 100 from one player
        // and 900 from another. Heal, Slam and two unnamed skills: 300 each,
        // Heal with the most casts.
        let mut entries = vec![
            entry(1, 100, false, 100, 1),
            entry(1, 200, false, 500, 1),
            entry(2, 100, false, 900, 1),
            entry(1, 1800, false, 300, 1),
            entry(1, 900, false, 300, 3),
            entry(2, 8, false, 300, 1),
            entry(2, 7, false, 300, 1),
        ];
        for turn in 0..entries.len() {
            entries.rotate_left(1);
            let skills: Vec<Skill> = entries.iter().map(|e| Skill::damage(e, &English)).collect();
            let names: Vec<String> = lanes(&skills, 10_000, 0).into_iter().map(|l| l.name).collect();
            assert_eq!(names, ["Fire", "Ice", "Heal", "Skill 7", "Skill 8", "Slam"], "turn {turn}");
        }

        // Sixteen equal lanes: the one left out is the last by name.
        let mut entries: Vec<SkillLine> = (1..=16).map(|code| entry(1, code, false, 100, 1)).collect();
        let forward: Vec<Skill> = entries.iter().map(|e| Skill::damage(e, &English)).collect();
        entries.reverse();
        let backward: Vec<Skill> = entries.iter().map(|e| Skill::damage(e, &English)).collect();
        let (forward, backward) = (lanes(&forward, 10_000, 0), lanes(&backward, 10_000, 0));
        assert_eq!(forward, backward);
        assert_eq!(forward.len(), MAX_LANES);
        assert!(forward.iter().all(|l| l.name != "Skill 9"));
    }

    #[test]
    fn chart_points_do_not_depend_on_the_order_of_the_entries() {
        // Hits at one time, 10^16, 1 and 1: a float sum in another order
        // differs in its last digits. Players 1 and 2 with equal damage.
        let hit = |actor: i32, code: i32, dmg: i64| Skill { actor, code, dmg, at: vec![1_000], ..Skill::default() };
        let one = [hit(1, 100, 10_000_000_000_000_000), hit(1, 7, 1), hit(1, 8, 1), hit(2, 200, 10_000_000_000_000_002)];
        let other = [hit(2, 200, 10_000_000_000_000_002), hit(1, 7, 1), hit(1, 8, 1), hit(1, 100, 10_000_000_000_000_000)];
        let (one, other) = (actor_hits(&one), actor_hits(&other));
        assert_eq!(one, other);
        assert_eq!(one.iter().map(|a| a.0).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(sample(&one[0].2, 10_000, 5_000.0), sample(&other[0].2, 10_000, 5_000.0));
    }

    #[test]
    fn with_no_healing_there_are_no_healers() {
        let ctx = context(vec![target(7, 10_000, &[(1, 600)])], vec![actor(1, "Ann")]);
        let view = build(&ctx, &Selection::default(), &You::default(), &English, &mut |_| answer(7, 10_000, vec![entry(1, 100, false, 600, 6)]));
        assert!(view.healers.is_empty());
        assert_eq!(view.heal.stats.heal, 0);
        assert_eq!(view.players.len(), 1, "the damage bars stay under DMG");
    }

    #[test]
    fn a_saved_fights_player_shows_their_entries_only() {
        let record: FightRecord = serde_json::from_value(serde_json::json!({
            "id": "auto_700_1", "bossName": "Boss", "targetId": 700, "startTimeMs": 1_000_000, "durationMs": 10_000,
            "totalDamage": 9_000, "jobs": ["정령성"], "isTrain": false,
            "details": {"targetId": 700, "maxHp": 0, "totalTargetDamage": 9_000, "battleTime": 10_000, "startTime": 0,
                "skills": [
                    {"actorId": 1, "code": 16010000, "name": "", "time": 10, "dmg": 6_000, "crit": 5, "job": "정령성",
                     "multiHitCount": 0, "multiHitDamage": 0, "parry": 0, "back": 0, "perfect": 0, "double": 0, "regen": 0,
                     "hitTimestamps": [0, 1000]},
                    {"actorId": 2, "code": 16010000, "name": "", "time": 6, "dmg": 3_000, "crit": 0, "job": "정령성",
                     "multiHitCount": 0, "multiHitDamage": 0, "parry": 0, "back": 0, "perfect": 0, "double": 0, "regen": 0,
                     "hitTimestamps": [500]}
                ],
                "pingHistory": [], "healSkills": [],
                "takenSkills": [{"actorId": 2, "code": 1800001, "name": "", "sourceCode": 0, "damage": 400, "hits": 2, "parry": 1}],
                "deaths": [{"actorId": 1, "deaths": 0}, {"actorId": 2, "deaths": 1}]},
            "actors": [
                {"actorId": 1, "nickname": "Me", "job": "정령성"},
                {"actorId": 2, "nickname": "Other", "job": "정령성"}
            ]
        }))
        .unwrap();
        let view = saved_player_view(&record, "Me", 2, &English);
        assert_eq!(view.damage.stats.damage, 3_000);
        assert_eq!(view.damage.skills.iter().map(|r| r.amount).sum::<i64>(), 3_000);
        assert_eq!(view.taken.rows.iter().map(|r| r.damage).collect::<Vec<_>>(), vec![400]);
        // Everyone's view keeps both.
        assert_eq!(saved_view(&record, "Me", &English).damage.stats.damage, 9_000);
    }

    #[test]
    fn a_pick_takes_every_actor_of_that_name() {
        let ctx = context(vec![], vec![actor(1, "Ann"), actor(5, "ann"), actor(2, "Bo"), actor(9, "9")]);
        assert_eq!(same_name_ids(&ctx, 1), [1, 5]);
        assert_eq!(same_name_ids(&ctx, 9), [9], "no name yet: the id alone");
        assert_eq!(same_name_ids(&ctx, 4), [4]);
    }

    #[test]
    fn damage_received_adds_up_per_skill_and_monster() {
        let mut parried = taken(2, 1800, 77, 300, 1);
        parried.stats.parry = 1;
        let entries = vec![taken(1, 1800, 77, 900, 2), parried, taken(1, 1200, 0, 50, 0)];
        let tab = taken_tab(Some(&entries), 10_000, &English);
        assert_eq!(tab.rows.iter().map(|r| (r.name.as_str(), r.source.as_str(), r.damage, r.attacks)).collect::<Vec<_>>(),
            [("Slam", "Boss", 1200, 3), ("Skill 1200", "", 50, 0)]);
        assert_eq!(tab.results, ["parry"]);
        assert_eq!((tab.rows[0].rates.parry, tab.rows[0].rates.shield_block), (Some(33), None), "1 of 3 hits, and none");
        assert_eq!(tab.rates.parry, 33.3);
        assert_eq!(tab.received, Some(Received { damage: 1250, attacks: 3 }));
        assert_eq!(tab.per_second, Some(125.0));
        let m = mechanics(&entries, &English);
        assert_eq!((m[0].players, m[0].hit[0].id, m[0].hit[1].id), (2, 1, 2));
        assert_eq!(m[1].players, 1);
    }

    #[test]
    fn damage_received_per_player_takes_every_id_of_one_name() {
        let ctx = context(vec![target(7, 10_000, &[(1, 600), (5, 100), (2, 400)])], vec![actor(1, "Ann"), actor(5, "ann"), actor(2, "Bo")]);
        let mut blocked = taken(5, 1800, 77, 200, 2);
        blocked.stats.shield_block = 1;
        let mut parried = taken(2, 1800, 77, 300, 1);
        parried.stats.parry = 1;
        parried.stats.miss = 1;
        let entries = vec![taken(1, 1800, 77, 900, 3), blocked, parried, taken(9, 1200, 0, 0, 0)];
        let tanks = tanks(&ctx, &entries, 10_000, &You { id: Some(2), name: "" });
        let rows: Vec<_> = tanks.iter().map(|t| (t.player.id, t.player.amount, t.attacks, t.shield_block, t.parry)).collect();
        assert_eq!(rows, [(1, 1100, 5, 1, 0), (2, 300, 2, 0, 1), (9, 0, 0, 0, 0)]);
        assert_eq!(tanks[0].player.per_second, Some(110.0));
        assert!((tanks[0].player.share - 1100.0 / 1400.0 * 100.0).abs() < 1e-9);
        assert!(tanks[1].player.you && !tanks[0].player.you);
        assert!(tanks[2].player.identifying, "not in the context: the id");
    }

    #[test]
    fn the_hover_card_lists_the_five_biggest_entries_each_on_its_own() {
        let theo = entry(1, 300, false, 50, 1);
        let skills = vec![
            entry(1, 100, false, 600, 3),
            entry(1, 100, true, 700, 4),
            entry(1, 200, false, 900, 1),
            theo,
            entry(1, 900, false, 10, 1),
            entry(1, 1800, false, 20, 1),
            entry(1, 4242, false, 0, 0),
        ];
        let (top, total) = hover(&answer(7, 10_000, skills), &English);
        let rows: Vec<(&str, i64)> = top.iter().map(|s| (s.name.as_str(), s.damage)).collect();
        assert_eq!(rows, [("Ice", 900), ("Fire - DOT", 700), ("Fire", 600), ("Theo: Spark", 50), ("Slam", 20)]);
        assert_eq!(total, 2280);
    }

    #[test]
    fn details_open_on_the_meters_target_or_every_target() {
        let mut ctx = context(vec![target(7, 1, &[]), target(8, 1, &[])], vec![]);
        assert_eq!(opening_target(&ctx, "bossTargets", 8, true, true), Some(8));
        assert_eq!(opening_target(&ctx, "allTargets", 0, false, true), None);
        assert_eq!(opening_target(&ctx, "lastHitByMe", 0, false, true), None);
        assert_eq!(opening_target(&ctx, "trainTargets", 8, true, true), None);
        assert_eq!(opening_target(&ctx, "trainTargets", 8, true, false), Some(8));
        ctx.current_target_id = 8;
        assert_eq!(opening_target(&ctx, "encounter", 0, false, true), Some(8));
    }

    #[test]
    fn healing_goes_to_its_targets_and_draws_each_healers_line() {
        let ctx = context(vec![target(7, 10_000, &[(1, 600), (2, 300)])], vec![actor(1, "Ann"), actor(2, "Bo")]);
        // Bo heals Ann twice and once himself; Ann heals herself once.
        let mut bo = entry(2, 17_010_000, false, 900, 3);
        bo.heal_ticks = vec![
            HealAt { at: 1_000, target: 1, amount: 400 },
            HealAt { at: 2_000, target: 2, amount: 100 },
            HealAt { at: 3_000, target: 1, amount: 400 },
        ];
        let mut ann = entry(1, 17_010_000, false, 50, 1);
        ann.heal_ticks = vec![HealAt { at: 4_000, target: 1, amount: 50 }];
        let mut fight = answer(7, 10_000, vec![entry(1, 100, false, 600, 6), entry(2, 100, false, 300, 3)]);
        fight.heal_skills = vec![bo, ann];
        let mut hit = taken(1, 1800, 77, 300, 2);
        hit.hit_times = vec![(2_000, 100), (6_000, 200)];
        fight.taken_skills = vec![hit];
        // The backend keeps only the asked actors' entries, as `details_for` does.
        let view = build(&ctx, &Selection { player: Some(2), target: Some(7) }, &You::default(), &English, &mut |call| {
            let mut answer = fight.clone();
            if let Call::Target { actors: Some(ids), .. } = call {
                answer.skills.retain(|e| ids.contains(&e.actor_id));
                answer.heal_skills.retain(|e| ids.contains(&e.actor_id));
                answer.taken_skills.retain(|e| ids.contains(&e.actor_id));
            }
            answer
        });

        let targets: Vec<(i32, i64)> = view.heal.targets.iter().map(|t| (t.id, t.amount)).collect();
        assert_eq!(targets, vec![(1, 800), (2, 100)], "Bo's healing, most first");
        assert_eq!(view.heal.targets[0].name, "Ann");
        assert!((view.heal.targets[0].share - 800.0 / 9.0).abs() < 1e-9);

        // Both healers draw a line; the picked one is Bo's.
        assert_eq!(view.heal_chart.actors.iter().map(|a| a.id).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(view.heal_chart.player, view.heal_chart.actors[0].points);
        let window = view.heal_chart.window_ms / 1000.0;
        assert_eq!(view.heal_chart.party[50], (800.0 + 100.0 + 50.0) / window, "every tick by 5 s");
        // Damage received: Ann's two hits; Bo took none.
        assert_eq!(view.taken_chart.actors.iter().map(|a| a.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(view.taken_chart.party[100], 200.0 / window, "the hit at 6 s, in the window before 10 s");
        assert!(view.taken_chart.player.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn the_chart_is_damage_over_the_window_before_each_point() {
        let hits = vec![(0, 100.0), (1_000, 100.0), (9_000, 100.0)];
        let points = sample(&hits, 10_000, 5_000.0);
        assert_eq!(points.len(), CHART_SAMPLES + 1);
        // The window before 0 s holds the hit at 0, before 4 s both early
        // hits, before 5 s only the one at 1 s, before 10 s the one at 9 s.
        assert_eq!((points[0], points[40], points[50], points[100]), (20.0, 40.0, 20.0, 20.0));
    }
}
