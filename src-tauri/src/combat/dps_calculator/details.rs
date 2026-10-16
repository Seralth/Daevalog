//! Skill details: the Details panel data for a target, the rows on screen, or a saved fight.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::combat::data_storage::{DeathsBy, HealsBy, SegmentIdentity, SkillTotals, TakenBy, TargetTotals, UNATTRIBUTED_ID};
use crate::combat::details_view::{self, Call, DetailsView, HoverSkill, LookupNames, PartyView, Selection, Words, You};
use crate::combat::timeline_tracks::{self, Fight, SavedRecords, TrackNames, Tracks};
use crate::entity::deaths::DeathEntry;
use crate::entity::detail_bundle::*;
use crate::entity::job_class::JobClass;
use crate::entity::skill_group::row_skill;
use crate::entity::taken::{TakenSkillEntry, TakenStats};
use crate::entity::owner_chain;

use super::fights::actor_stats;
use super::meter_rows::{active_time, own_span};
use super::rows::{merge_name_aliases, display_name_for};
use super::{DpsCalculator, TargetScope};

/// How much of a fight an answer reads.
#[derive(Clone, Copy, PartialEq)]
enum Depth {
    /// The meter's hover card: no hit times, healing, damage taken, deaths
    /// or ping.
    Hover,
    /// The meter's Heal and Tank tabs: the healing without its ticks, which
    /// only Details' charts and heal targets draw.
    Party,
    /// Everything Details shows.
    Full,
}

const DOT_MARK: &str = " - DOT";
const HOT_MARK: &str = " - HoT";

/// Who was who for one answer: summon links, names and your id. Live
/// answers take the storage's; a cleared segment keeps its own.
pub(super) struct Names {
    pub(super) links: HashMap<i32, i32>,
    pub(super) nicks: HashMap<i32, String>,
    pub(super) you: Option<i32>,
}

impl Names {
    pub(super) fn of_segment(identity: &SegmentIdentity) -> Self {
        Names {
            links: identity.summons.clone(),
            nicks: identity.nicknames.clone(),
            you: identity.local_player_id.map(|id| id as i32),
        }
    }

    fn owner(&self, id: i32) -> i32 {
        owner_chain::resolve(id, &self.links)
    }

    /// The player behind `id` (its last owner); `None` when that is 0 or
    /// below, which is no one.
    fn player_of(&self, id: i32) -> Option<i32> {
        Some(self.owner(id)).filter(|&owner| owner > 0)
    }

    pub(super) fn name(&self, id: i32) -> String {
        display_name_for(id, &self.nicks, &self.links)
    }
}

/// One fight's ids as Details lists them: one id per name.
struct Canon<'a> {
    names: &'a Names,
    by_name: HashMap<String, i32>,
}

impl<'a> Canon<'a> {
    fn new(fight: &TargetTotals, names: &'a Names) -> Self {
        let damage: HashMap<i32, i64> = fight.actors.iter().map(|(id, actor)| (*id, actor.total_damage)).collect();
        let by_name = merge_name_aliases(&damage, &names.links, &names.nicks, names.you);
        Canon { names, by_name }
    }

    fn of(&self, id: i32) -> i32 {
        self.by_name.get(&self.names.name(id)).copied().unwrap_or(id)
    }
}

/// What an answer is made of beside the fight. A hover answer has no
/// `extras`, and so no healing, damage taken, deaths or ping.
pub(super) struct FightParts<'a> {
    pub(super) fight: &'a TargetTotals,
    pub(super) max_hp: i32,
    pub(super) extras: Option<Extras<'a>>,
}

/// The lists a full answer has beside the damage.
pub(super) struct Extras<'a> {
    pub(super) heals: &'a HealsBy,
    pub(super) taken: &'a TakenBy,
    pub(super) deaths: &'a DeathsBy,
}

/// One damage entry from its skills' summed hits (`SkillTotals::absorb`
/// sums them); hit times count from `start`, still in order.
fn damage_entry(blank: SkillLine, sum: SkillTotals, start: i64) -> SkillLine {
    SkillLine {
        time: sum.hit_count,
        dmg: sum.total_damage,
        multi_hit_count: sum.multi_hit_count,
        multi_hit_damage: sum.multi_hit_damage,
        multi_hit_hits: sum.multi_hit_hits,
        // Still at the start: no skill of the entry hit for damage (rule 6).
        min_dmg: if sum.min_damage == i64::MAX { 0 } else { sum.min_damage },
        max_dmg: sum.max_damage.max(0),
        crit: sum.crit_count,
        shield_block: sum.shield_block_count,
        parry: sum.parry_count,
        back: sum.back_count,
        frontal: sum.frontal_count,
        perfect: sum.perfect_count,
        double: sum.double_count,
        iron_wall: sum.iron_wall_count,
        regeneration: sum.regeneration_count,
        perfect_block: sum.perfect_block_count,
        miss: sum.miss_count,
        resist: sum.resist_count,
        regen: sum.heal_amount,
        hit_timestamps: sum.hit_timestamps.iter().map(|&at| at - start).collect(),
        specs: sum.spec_flags.to_vec(),
        ..blank
    }
}

/// An answer with nothing in it.
fn empty_answer(target_id: i32) -> TargetBreakdownReply {
    TargetBreakdownReply {
        target_id,
        max_hp: 0,
        total_target_damage: 0,
        battle_time: 0,
        start_time: 0,
        skills: Vec::new(),
        ping_history: Vec::new(),
        heal_skills: Vec::new(),
        taken_skills: Vec::new(),
        deaths: None,
    }
}

/// The ids an actor filter lets through: the ids asked for, and every
/// player in the fight listed under the same id as one of them.
fn let_through(fight: &TargetTotals, canon: &Canon, asked: &[i32]) -> HashSet<i32> {
    let listed: HashSet<i32> = asked.iter().map(|&id| canon.of(id)).collect();
    let mut ids: HashSet<i32> = asked.iter().copied().collect();
    for player in fight.actors.keys().filter_map(|&actor| canon.names.player_of(actor)) {
        if listed.contains(&canon.of(player)) {
            ids.insert(player);
        }
    }
    ids
}

fn class_name(class: Option<JobClass>) -> String {
    class.map(|c| c.class_name().to_string()).unwrap_or_default()
}

/// A player as Details and a saved fight list them, with the damage taken
/// of `taken` under its id; the party roster's fields are left at 0.
pub(super) fn actor_summary(
    id: i32,
    nickname: String,
    class: Option<JobClass>,
    (party_heal, regen): (i64, i64),
    taken: &[TakenSkillEntry],
) -> ActorBreakdown {
    let mut took = TakenStats::default();
    for entry in taken.iter().filter(|e| e.actor_id == id) {
        took.absorb(&entry.stats);
    }
    ActorBreakdown {
        actor_id: id,
        nickname,
        job: class_name(class),
        job_id: class.map_or(0, |c| c.job_code_head()),
        party_heal,
        regen,
        damage_received: took.damage,
        hits_received: took.total(),
        dbid: 0,
        server_id: 0,
        level: 0,
        gear_score: 0,
        combat_power: 0,
    }
}

/// Each listed target's damage per listed id, and each listed id's class
/// from its hits over every listed target. Ids that are neither named nor
/// of a class are left out.
fn listed_damage(listed: &[TargetTotals], names: &Names) -> (Vec<HashMap<i32, i64>>, HashMap<i32, JobClass>) {
    let mut damage_on: Vec<HashMap<i32, i64>> = Vec::with_capacity(listed.len());
    let mut hits_of: HashMap<i32, Vec<(i32, i32)>> = HashMap::new();
    for fight in listed {
        let canon = Canon::new(fight, names);
        let mut damage: HashMap<i32, i64> = HashMap::new();
        for (&actor, data) in &fight.actors {
            let Some(player) = names.player_of(actor) else { continue };
            let id = canon.of(player);
            let sum = damage.entry(id).or_default();
            *sum = sum.saturating_add(data.total_damage);
            hits_of.entry(id).or_default().extend(data.skills.values().map(|s| (s.skill_code, s.hit_count)));
        }
        damage_on.push(damage);
    }
    let classes: HashMap<i32, JobClass> = hits_of
        .into_iter()
        .filter(|&(id, _)| id != UNATTRIBUTED_ID)
        .filter_map(|(id, hits)| JobClass::by_hits(hits).map(|class| (id, class)))
        .collect();
    let someone = |id: i32| id == UNATTRIBUTED_ID || classes.contains_key(&id) || names.nicks.contains_key(&id);
    for damage in &mut damage_on {
        damage.retain(|&id, _| someone(id));
    }
    (damage_on, classes)
}

/// Each name's listed id, the lowest when two listed ids share a name.
pub(super) fn ids_by_name(ids: &BTreeSet<i32>, name_of: impl Fn(i32) -> String) -> HashMap<String, i32> {
    let mut by_name = HashMap::new();
    for &id in ids {
        by_name.entry(name_of(id)).or_insert(id);
    }
    by_name
}

/// Details answers: one target, the targets on the meter, and the context
/// the page opens with.
impl DpsCalculator {
    pub(super) fn live_names(&self) -> Names {
        Names {
            links: self.data_storage.pet_owners(),
            nicks: self.data_storage.all_names(),
            you: self.data_storage.local_player_id().map(|id| id as i32),
        }
    }

    fn in_encounter(&self) -> bool {
        self.target_selection_mode == TargetScope::Encounter
    }

    pub fn target_breakdown(&self, target_id: i32, actor_ids: Option<&[i32]>) -> TargetBreakdownReply {
        self.one_target(target_id, actor_ids, Depth::Full)
    }

    /// `target_breakdown` without hit times, healing, damage taken,
    /// deaths or ping, for the meter's hover card.
    pub fn get_hover_details(&self, target_id: i32, actor_ids: Option<&[i32]>) -> TargetBreakdownReply {
        self.one_target(target_id, actor_ids, Depth::Hover)
    }

    /// The answer over every target the meter shows.
    pub fn get_displayed_details(&self, actor_ids: Option<&[i32]>) -> TargetBreakdownReply {
        self.on_screen(actor_ids, Depth::Full)
    }

    pub fn get_displayed_hover_details(&self, actor_ids: Option<&[i32]>) -> TargetBreakdownReply {
        self.on_screen(actor_ids, Depth::Hover)
    }

    /// The healing during `fights`: without its ticks for the Heal and Tank
    /// tabs, from the sums storage keeps between reads.
    fn heals_in(&self, fights: &[&TargetTotals], depth: Depth) -> HealsBy {
        if depth == Depth::Party {
            self.data_storage.fights_heal_sums(fights)
        } else {
            self.data_storage.fights_heals(fights)
        }
    }

    fn one_target(&self, target_id: i32, actor_ids: Option<&[i32]>, depth: Depth) -> TargetBreakdownReply {
        let storage = &self.data_storage;
        let light = depth == Depth::Hover;
        let mut fights = storage.get_target_snapshots(&[target_id], self.in_encounter(), light);
        let Some(fight) = fights.remove(&target_id) else {
            return empty_answer(target_id);
        };
        let max_hp = storage.enemy_max_hp(target_id).unwrap_or(0);
        let names = self.live_names();
        if light {
            return self.answer_from(&FightParts { fight: &fight, max_hp, extras: None }, &names, actor_ids);
        }
        let (heals, taken, deaths) = (self.heals_in(&[&fight], depth), storage.fight_taken(&fight), storage.fight_deaths(&fight));
        let extras = Some(Extras { heals: &heals, taken: &taken, deaths: &deaths });
        self.answer_from(&FightParts { fight: &fight, max_hp, extras }, &names, actor_ids)
    }

    fn on_screen(&self, actor_ids: Option<&[i32]>, depth: Depth) -> TargetBreakdownReply {
        if let [single] = self.displayed_targets[..] {
            return self.one_target(single, actor_ids, depth);
        }
        let storage = &self.data_storage;
        let encounter = self.in_encounter();
        let light = depth == Depth::Hover;
        let Some(fight) = self.displayed_fight(encounter, light) else {
            return empty_answer(0);
        };
        let names = self.live_names();
        let mut answer = if light {
            self.answer_from(&FightParts { fight: &fight, max_hp: 0, extras: None }, &names, actor_ids)
        } else {
            let heals = self.heals_in(&[&fight], depth);
            let taken = storage.taken_on(&self.displayed_targets, encounter, None);
            let deaths = storage.deaths_on(&self.displayed_targets, encounter, None);
            let extras = Some(Extras { heals: &heals, taken: &taken, deaths: &deaths });
            self.answer_from(&FightParts { fight: &fight, max_hp: 0, extras }, &names, actor_ids)
        };
        answer.battle_time = self.displayed_battle_time;
        answer
    }

    /// The targets on the meter as one fight; with `encounter`, their
    /// earlier fights in the encounter too.
    fn displayed_fight(&self, encounter: bool, light: bool) -> Option<TargetTotals> {
        let fights = self.data_storage.get_target_snapshots(&self.displayed_targets, encounter, light);
        TargetTotals::merged(self.displayed_targets.iter().filter_map(|t| fights.get(t)))
    }

    /// One fight's answer, every entry under the id Details lists its
    /// player by; `actor_ids` keeps those players only.
    pub(super) fn answer_from(&self, parts: &FightParts, names: &Names, actor_ids: Option<&[i32]>) -> TargetBreakdownReply {
        let fight = parts.fight;
        let canon = Canon::new(fight, names);
        let through = actor_ids.map(|asked| let_through(fight, &canon, asked));
        let kept = |id: i32| through.as_ref().is_none_or(|ids| ids.contains(&id));
        let (first, last) = (fight.first_damage_time, fight.last_damage_time);

        let mut damage: BTreeMap<(i32, i32, bool), SkillTotals> = BTreeMap::new();
        for (&actor, data) in &fight.actors {
            let Some(player) = names.player_of(actor).filter(|&p| kept(p)) else { continue };
            let id = canon.of(player);
            for skill in data.skills.values() {
                let code = row_skill(skill.skill_code, &self.skill_lookup);
                let sum = damage.entry((id, code, skill.is_dot)).or_insert_with(|| SkillTotals::new(code, skill.is_dot));
                sum.absorb(skill.clone());
            }
        }
        let skills = damage
            .into_iter()
            .map(|((id, code, over_time), sum)| damage_entry(self.blank_entry(id, code, over_time, DOT_MARK), sum, first))
            .collect();

        let (mut heal_skills, mut taken_skills, mut deaths, mut ping_history) = (Vec::new(), Vec::new(), None, Vec::new());
        if let Some(extras) = &parts.extras {
            let place = |id: i32| canon.of(id);
            heal_skills = self.heal_entries(extras.heals, names, place);
            heal_skills.retain(|e| kept(e.actor_id));
            taken_skills = self.taken_entries(extras.taken, place);
            taken_skills.retain(|e| kept(e.actor_id));
            let mut fell = death_entries(extras.deaths, place);
            fell.retain(|e| kept(e.actor_id));
            deaths = Some(fell);
            ping_history = self
                .pings
                .latency_between(first, last)
                .into_iter()
                .map(|(at, ping_ms)| LatencySample { ts_ms: at - first, ping_ms })
                .collect();
        }
        TargetBreakdownReply {
            target_id: fight.target_id,
            max_hp: parts.max_hp,
            total_target_damage: fight.total_damage,
            battle_time: (last - first).max(0),
            start_time: first,
            skills,
            ping_history,
            heal_skills,
            taken_skills,
            deaths,
        }
    }

    /// A zeroed entry of `actor`'s skill `code`, named, with the class the
    /// code belongs to; `mark` follows the name of a tick entry.
    fn blank_entry(&self, actor: i32, code: i32, over_time: bool, mark: &str) -> SkillLine {
        let mut name = self.skill_lookup.lookup_skill_name(code);
        if over_time && !name.is_empty() {
            name.push_str(mark);
        }
        let class = if actor == UNATTRIBUTED_ID { None } else { JobClass::job_of_skill(code) };
        SkillLine {
            actor_id: actor,
            code,
            name,
            time: 0,
            dmg: 0,
            multi_hit_count: 0,
            multi_hit_damage: 0,
            multi_hit_hits: 0,
            min_dmg: 0,
            max_dmg: 0,
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
            job: class_name(class),
            is_dot: over_time,
            hit_timestamps: Vec::new(),
            specs: Vec::new(),
            heal_ticks: Vec::new(),
        }
    }

    /// Heal entries by (listed id, skill, heal over time). A healer goes
    /// under `place` of its player and is left out without one; whom a tick
    /// healed goes under `place` of its owner.
    fn heal_entries(&self, heals: &HealsBy, names: &Names, place: impl Fn(i32) -> i32) -> Vec<SkillLine> {
        let mut rows: BTreeMap<(i32, i32, bool), SkillLine> = BTreeMap::new();
        for (&healer, by_skill) in heals {
            let Some(id) = names.player_of(healer).map(&place) else { continue };
            for (&(code, over_time), data) in by_skill {
                let row = rows.entry((id, code, over_time)).or_insert_with(|| self.blank_entry(id, code, over_time, HOT_MARK));
                row.dmg = row.dmg.saturating_add(data.total_heal);
                row.time = row.time.saturating_add(data.tick_count);
                row.heal_ticks.extend(data.ticks.iter().map(|tick| HealAt { target: place(names.owner(tick.target)), ..*tick }));
            }
        }
        rows.into_values()
            .map(|mut row| {
                row.heal_ticks.sort_unstable_by_key(|t| (t.at, t.target, t.amount));
                row
            })
            .collect()
    }

    /// Damage taken entries by (listed id, skill); `player_id` gives a
    /// player's listed id.
    fn taken_entries(&self, taken: &TakenBy, player_id: impl Fn(i32) -> i32) -> Vec<TakenSkillEntry> {
        let mut rows: BTreeMap<(i32, i32), TakenSkillEntry> = BTreeMap::new();
        for (&player, by_skill) in taken {
            let id = player_id(player);
            for (&code, data) in by_skill {
                let row = rows.entry((id, code)).or_insert_with(|| TakenSkillEntry {
                    actor_id: id,
                    code,
                    name: self.skill_lookup.lookup_skill_name(code),
                    source_code: data.source_code,
                    stats: TakenStats::default(),
                    hit_times: Vec::new(),
                });
                row.stats.absorb(&data.stats);
                row.source_code = row.source_code.max(data.source_code);
                row.hit_times.extend_from_slice(&data.hit_times);
            }
        }
        rows.into_values()
            .map(|mut row| {
                row.hit_times.sort_unstable();
                row
            })
            .collect()
    }

    /// What the Details page opens with: the live targets (in TRAIN and BOSS
    /// those on the meter), most damage first, with their players and the
    /// healing, damage taken and deaths over all of them.
    pub fn get_details_context(&self) -> DetailBundle {
        self.context_at(Depth::Full)
    }

    /// `get_details_context` for the meter's Heal and Tank tabs
    /// (`party_view_from`): its heal entries carry no ticks.
    pub fn party_context(&self) -> DetailBundle {
        self.context_at(Depth::Party)
    }

    fn context_at(&self, depth: Depth) -> DetailBundle {
        let storage = &self.data_storage;
        let names = self.live_names();
        let listed = self.listed_targets();
        let (damage_on, mut classes) = listed_damage(&listed, &names);
        let ids: BTreeSet<i32> = damage_on.iter().flat_map(|d| d.keys().copied()).collect();

        // Each target lists its players under its own canonical ids (rule
        // 18), so one player may be listed under two ids. Damage taken,
        // deaths and healing span every listed target: they go by name to
        // one listed id, the lowest (rule 23).
        let by_name = ids_by_name(&ids, |id| names.name(id));
        let place = |id: i32| by_name.get(&names.name(id)).copied().unwrap_or(id);

        let listed_ids: Vec<i32> = listed.iter().map(|t| t.target_id).collect();
        let taken_skills = self.taken_entries(&storage.taken_on(&listed_ids, false, None), place);
        let deaths = death_entries(&storage.deaths_on(&listed_ids, false, None), place);
        let fights: Vec<&TargetTotals> = listed.iter().collect();
        let heal_skills = self.heal_entries(&self.heals_in(&fights, depth), &names, place);

        let own = actor_stats(listed.iter());
        let actors: Vec<ActorBreakdown> = self
            .with_encounter_players(&ids, &mut classes, &names)
            .iter()
            .map(|&id| {
                let stats = own.get(&id).copied().unwrap_or_default();
                actor_summary(id, names.name(id), classes.get(&id).copied(), stats, &taken_skills)
            })
            .collect();
        let numbers = self.player_numbers(actors.iter().map(|a| (a.actor_id, a.nickname.as_str())));
        DetailBundle {
            current_target_id: storage.focused_target(),
            targets: self.target_summaries(&listed, damage_on),
            actors,
            numbers,
            taken_skills,
            heal_skills,
            deaths: Some(deaths),
            battle_time: self.listed_battle_time(&listed, &names),
        }
    }

    /// `ids`, and in ENC also the players of the encounter's earlier pulls
    /// and fights, their classes added to `classes`: ENC's Heal and Tank
    /// tabs read the whole encounter, and a player not listed showed there
    /// as an id with no class.
    fn with_encounter_players(&self, ids: &BTreeSet<i32>, classes: &mut HashMap<i32, JobClass>, names: &Names) -> BTreeSet<i32> {
        let mut everyone = ids.clone();
        let whole = if self.in_encounter() { self.displayed_fight(true, true) } else { None };
        if let Some(whole) = whole {
            let (damage_on, theirs) = listed_damage(std::slice::from_ref(&whole), names);
            everyone.extend(damage_on.iter().flat_map(|d| d.keys().copied()));
            classes.extend(theirs.into_iter().filter(|(id, _)| !ids.contains(id)));
        }
        everyone
    }

    /// The live targets Details lists (in TRAIN and BOSS those on the
    /// meter), most damage first, then lowest id.
    fn listed_targets(&self) -> Vec<TargetTotals> {
        let mut listed: Vec<TargetTotals> = self.data_storage.copy_targets_brief().into_values().collect();
        if matches!(self.target_selection_mode, TargetScope::TrainTargets | TargetScope::BossTargets) {
            listed.retain(|t| self.displayed_targets.contains(&t.target_id));
        }
        listed.sort_by_key(|t| (Reverse(t.total_damage), t.target_id));
        listed
    }

    /// TRAIN counts your own time on the dummies, as the meter does; the
    /// other modes the time anything on the list was fought.
    fn listed_battle_time(&self, listed: &[TargetTotals], names: &Names) -> i64 {
        if self.target_selection_mode == TargetScope::TrainTargets {
            self.own_side_ids(&names.links).map_or(0, |mine| {
                active_time(listed.iter().filter_map(|t| own_span(t, &mine, &names.links)), i64::MIN)
            })
        } else {
            active_time(listed.iter().map(|t| (t.first_damage_time, t.last_damage_time)), i64::MIN)
        }
    }

    fn target_summaries(&self, listed: &[TargetTotals], damage_on: Vec<HashMap<i32, i64>>) -> Vec<TargetBreakdown> {
        let storage = &self.data_storage;
        listed
            .iter()
            .zip(damage_on)
            .map(|(fight, actor_damage)| TargetBreakdown {
                target_id: fight.target_id,
                target_name: self.target_label(fight.target_id),
                mob_code: storage.enemy_code(fight.target_id).unwrap_or(0),
                max_hp: storage.enemy_max_hp(fight.target_id).unwrap_or(0),
                battle_time: (fight.last_damage_time - fight.first_damage_time).max(0),
                last_damage_time: fight.last_damage_time,
                total_damage: fight.total_damage,
                actor_damage,
            })
            .collect()
    }
}

/// What Details shows, for a front end that draws it as it comes
/// (`combat/details_view.rs`), from the calls the 1.x page makes.
impl DpsCalculator {
    pub fn details_view(&self, selection: &Selection, words: &Words) -> DetailsView {
        let context = self.get_details_context();
        self.details_view_from(&context, selection, words, &mut |call| self.details_call(call))
    }

    /// `details_view` from this context, asking `ask` what 1.x asks the
    /// backend.
    pub fn details_view_from(
        &self,
        context: &DetailBundle,
        selection: &Selection,
        words: &Words,
        ask: &mut dyn FnMut(&Call) -> TargetBreakdownReply,
    ) -> DetailsView {
        let names = LookupNames { skills: &self.skill_lookup, npcs: &self.npc_lookup, words };
        let name = self.data_storage.local_character_name().unwrap_or_default();
        let you = You { id: self.data_storage.local_player_id(), name: &name };
        details_view::build(context, selection, &you, &names, ask)
    }

    /// What the meter's Heal and Tank tabs take from Details on every player
    /// and `target` (`None`: every target), from this context. ENC reads the
    /// encounter's targets as its DPS rows do, with the damage taken and the
    /// deaths of its earlier fights (reading only the target Details opens
    /// on, the Tank tab showed 0 for everyone). Its answers read
    /// the healing without ticks, as `party_context` does.
    pub fn party_view_from(&self, context: &DetailBundle, target: Option<i32>) -> PartyView {
        let words = Words::default();
        let names = LookupNames { skills: &self.skill_lookup, npcs: &self.npc_lookup, words: &words };
        let name = self.data_storage.local_character_name().unwrap_or_default();
        let you = You { id: self.data_storage.local_player_id(), name: &name };
        let encounter = self.target_selection_mode == TargetScope::Encounter;
        let displayed = encounter.then_some(self.displayed_targets.as_slice());
        details_view::party_view(context, target, displayed, &you, &names, &mut |call| self.call_at(call, Depth::Party))
    }

    /// The meter's hover card for the row of `actor`, as 1.x asks for it
    /// (`get_displayed_hover_details`): its biggest skills and all their
    /// damage.
    pub fn hover_view(&self, actor: i32, words: &Words) -> (Vec<HoverSkill>, i64) {
        self.hover_of(&self.get_displayed_hover_details(Some(&[actor])), words)
    }

    /// The hover card from an answer already read. Skills of equal damage
    /// keep the answer's order, which a new read may change, as in 1.x.
    pub fn hover_of(&self, answer: &TargetBreakdownReply, words: &Words) -> (Vec<HoverSkill>, i64) {
        let names = LookupNames { skills: &self.skill_lookup, npcs: &self.npc_lookup, words };
        details_view::hover(answer, &names)
    }

    /// The timeline's buff, debuff and stat tracks of the player `view`
    /// shows, over its fight; none with every player shown. With every
    /// target, the debuffs are on the target the player hit most.
    pub fn timeline_tracks(&self, view: &DetailsView, names: &TrackNames) -> Tracks {
        let player: HashSet<i32> = view.player_ids.iter().copied().collect();
        let you = self.data_storage.local_player_id().and_then(|id| i32::try_from(id).ok()).is_some_and(|id| player.contains(&id));
        let target = Some(view.target).filter(|&t| t != 0).or_else(|| view.targets.first().map(|t| t.id));
        let fight = Fight { start: view.clock_start, length: view.chart.battle_ms, player: &player, you, target };
        if fight.length <= 0 || player.is_empty() {
            return Tracks::default();
        }
        let end = fight.start + fight.length;
        let instances = self.data_storage.abnormals_between(fight.start, end);
        let stats = if you { self.data_storage.stats_between(fight.start, end) } else { Vec::new() };
        let links = self.data_storage.pet_owners();
        let owner = |id: i32| owner_chain::resolve(id, &links);
        let skill = |code: u32| i32::try_from(code).map(|c| self.skill_lookup.lookup_skill_name(c)).unwrap_or_default();
        timeline_tracks::tracks(&fight, &instances, &stats, owner, skill, names)
    }

    /// What a saved fight keeps for its timeline (`SavedRecords`): the
    /// records of the fight's time, from its first hit to its last.
    pub(super) fn saved_timeline(
        &self,
        fight: &TargetTotals,
        players: &HashSet<i32>,
        summons: &HashMap<i32, i32>,
        you: i32,
    ) -> SavedRecords {
        let (from, to) = (fight.first_damage_time, fight.last_damage_time);
        let instances = self.data_storage.abnormals_between(from, to);
        let stats = if players.contains(&you) { self.data_storage.stats_between(from, to) } else { Vec::new() };
        let owner = |id: i32| owner_chain::resolve(id, summons);
        SavedRecords::keep(&instances, &stats, players, fight.target_id, owner, you, from)
    }

    /// One of the calls 1.x Details makes.
    pub fn details_call(&self, call: &Call) -> TargetBreakdownReply {
        self.call_at(call, Depth::Full)
    }

    fn call_at(&self, call: &Call, depth: Depth) -> TargetBreakdownReply {
        match call {
            Call::Target { target, actors } => self.one_target(*target, actors.as_deref(), depth),
            Call::Displayed { actors } => self.on_screen(actors.as_deref(), depth),
        }
    }

    /// The target Details opens on from the meter's row, in this mode.
    pub fn details_opening_target(&self, context: &DetailBundle, meter_target: i32, meter_target_named: bool, train_all: bool) -> Option<i32> {
        details_view::opening_target(context, self.target_selection_mode.id(), meter_target, meter_target_named, train_all)
    }
}

/// Deaths per player on the id `uid_of` gives each, the deaths of two ids of
/// one player added up, by id.
fn death_entries(deaths: &DeathsBy, uid_of: impl Fn(i32) -> i32) -> Vec<DeathEntry> {
    let mut by_uid: HashMap<i32, u32> = HashMap::new();
    for (&player, &n) in deaths {
        *by_uid.entry(uid_of(player)).or_default() += n;
    }
    let mut out: Vec<DeathEntry> = by_uid.into_iter().map(|(actor_id, deaths)| DeathEntry { actor_id, deaths }).collect();
    out.sort_by_key(|e| e.actor_id);
    out
}

#[cfg(test)]
mod tests {
    use super::super::test_kit::{land, meter, storage_for, GLADIATOR_SKILL};
    use super::*;

    #[test]
    fn the_heal_and_tank_tabs_read_the_healing_without_its_ticks() {
        let s = storage_for(Some(1_000));
        s.name_entity(1_000, "Me");
        s.name_entity(1_001, "Cleric");
        for at in (1_000..10_000).step_by(1_000) {
            land(&s, 1_000, 900, at, GLADIATOR_SKILL, 500);
            land(&s, 1_001, 901, at + 500, GLADIATOR_SKILL, 500);
            s.add_heal_event(1_001, 1_000, 17_010_000, 100, at % 2_000 == 0, at);
            s.add_heal_event(1_000, 1_001, 17_010_000, 50, false, at + 500);
        }
        let mut calc = meter(&s);
        calc.set_target_selection_mode("allTargets");
        calc.get_dps();
        let (full, party) = (calc.get_details_context(), calc.party_context());
        assert!(full.heal_skills.iter().all(|e| e.heal_ticks.len() == e.time as usize));
        assert!(party.heal_skills.iter().all(|e| e.heal_ticks.is_empty()));
        let json = |c: &DetailBundle| serde_json::to_value(c).unwrap();
        assert_eq!(json(&party), json(&full), "the ticks are never written");
        for target in [None, Some(900), Some(901)] {
            let view = calc.details_view_from(&full, &Selection { player: None, target }, &Words::default(), &mut |c| calc.details_call(c));
            let tabs = calc.party_view_from(&party, target);
            assert!(!tabs.healers.is_empty());
            assert_eq!((tabs.players, tabs.healers, tabs.tanks), (view.players, view.healers, view.tanks));
        }
        crate::clock::set_override(None);
    }

    /// The Heal and Tank tabs' read late in a 400 against 400 fight: 400
    /// players on a boss and 400 strangers healing, 300,000 heal ticks, a
    /// few new between reads. Run with `cargo test --release --lib --
    /// --ignored party_read_time --nocapture`.
    #[test]
    #[ignore]
    fn party_read_time() {
        let s = storage_for(Some(1_000));
        (1_000..1_800).for_each(|p| s.name_entity(p, &format!("P{p}")));
        let heal = |at: i64, n: i32| {
            for i in 0..n {
                s.add_heal_event(1_000 + i % 800, 1_000 + i % 400, 17_010_000 + i % 5, 100, i % 2 == 0, at);
            }
        };
        for at in (1_000..601_000).step_by(1_000) {
            (1_000..1_400).for_each(|p| land(&s, p, 900, at, GLADIATOR_SKILL, 500));
            heal(at, 500);
        }
        let mut calc = meter(&s);
        calc.set_target_selection_mode("allTargets");
        let reads = 20;
        let started = std::time::Instant::now();
        for at in (601_000..).step_by(500).take(reads) {
            (1_000..1_010).for_each(|p| land(&s, p, 900, at, GLADIATOR_SKILL, 500));
            heal(at, 50);
            let context = calc.party_context();
            let target = calc.details_opening_target(&context, 900, true, false);
            assert_eq!(calc.party_view_from(&context, target).healers.len(), 400);
        }
        println!("{:.1} ms per read", started.elapsed().as_secs_f64() * 1000.0 / reads as f64);
        crate::clock::set_override(None);
    }

    #[test]
    fn a_damage_entry_carries_every_field_of_its_skill() {
        let hits = SkillTotals {
            skill_code: 11_010_000,
            is_dot: true,
            hit_count: 1,
            total_damage: 2,
            min_damage: 3,
            max_damage: 4,
            crit_count: 5,
            back_count: 6,
            frontal_count: 7,
            shield_block_count: 8,
            parry_count: 9,
            perfect_count: 10,
            double_count: 11,
            iron_wall_count: 12,
            regeneration_count: 13,
            perfect_block_count: 14,
            miss_count: 15,
            resist_count: 16,
            multi_hit_count: 17,
            multi_hit_damage: 18,
            multi_hit_hits: 19,
            heal_amount: 20,
            hit_timestamps: vec![1_021, 1_022],
            spec_flags: [true, false, true, false, true],
        };
        let mut sum = SkillTotals::new(11_010_000, true);
        sum.absorb(hits);
        let calc = meter(&storage_for(None));
        let e = damage_entry(calc.blank_entry(7, 11_010_000, true, DOT_MARK), sum, 1_000);
        // How the hits landed, 5 to 16 in the order above.
        let landed = [e.crit, e.back, e.frontal, e.shield_block, e.parry, e.perfect];
        let blocked = [e.double, e.iron_wall, e.regeneration, e.perfect_block, e.miss, e.resist];
        assert_eq!([landed, blocked].concat(), (5..=16).collect::<Vec<i32>>());
        assert_eq!((e.time, e.dmg, e.min_dmg, e.max_dmg, e.regen), (1, 2, 3, 4, 20));
        assert_eq!((e.multi_hit_count, e.multi_hit_damage, e.multi_hit_hits), (17, 18, 19));
        assert_eq!((e.actor_id, e.code, e.is_dot), (7, 11_010_000, true));
        assert_eq!((e.hit_timestamps, e.specs), (vec![21, 22], vec![true, false, true, false, true]));
    }
}
