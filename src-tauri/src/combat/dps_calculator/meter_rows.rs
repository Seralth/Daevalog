//! The meter rows: damage, DPS and the per-row stats for the targets on screen.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::clock::now_ms;
use crate::combat::data_storage::{ActorTotals, RosterEntry, SecondStats, TargetTotals, UNATTRIBUTED_ID};
use crate::entity::deaths::DeathRow;
use crate::entity::dps_data::DpsData;
use crate::entity::job_class::JobClass;
use crate::entity::personal_data::PersonalData;
use crate::entity::owner_chain;
use crate::entity::taken::{TakenRow, TakenStats};

use super::rows::{merge_name_aliases, display_name_for, UNKNOWN_CLASS};
use super::target::Owners;
use super::{DpsCalculator, TargetScope};

impl DpsCalculator {
    /// Damage taken over the fights behind the rows (the same targets, the
    /// same window), one row per player, the rows' ids where they have one.
    /// Players with no damage row are named by the game or are you; an id
    /// nothing names is left out, as on the damage rows.
    pub(super) fn taken_rows(&mut self, dps: &DpsData) -> Vec<TakenRow> {
        let encounter_mode = self.target_selection_mode == TargetScope::Encounter;
        let since = self.rows_since();
        let key: super::TakenKey = (
            self.data_storage.damage_generation(),
            self.data_storage.taken_generation(),
            self.target_selection_mode,
            self.displayed_targets.clone(),
            since,
        );
        if let Some((cached, rows)) = &self.taken_cache {
            if *cached == key {
                return rows.clone();
            }
        }
        let taken = self.data_storage.taken_per_player(&mut self.taken_sums, &self.displayed_targets, encounter_mode, since);
        let nicknames = self.data_storage.all_names();
        let local = self.data_storage.local_player_id().map(|v| v as i32);
        let party = self.data_storage.roster_entries();
        let by_name: HashMap<&str, i32> = dps.map.iter().map(|(&id, d)| (d.nickname.trim(), id)).collect();
        let mut rows: HashMap<i32, TakenRow> = HashMap::new();
        for (player, stats) in &taken {
            let name = nicknames.get(player);
            if name.is_none() && Some(*player) != local && !dps.map.contains_key(player) {
                continue;
            }
            let nickname = name.cloned().unwrap_or_else(|| player.to_string());
            let uid = by_name.get(nickname.trim()).copied().unwrap_or(*player);
            let row = rows.entry(uid).or_insert_with(|| {
                let shown = dps.map.get(&uid);
                let job = self.row_job(shown, &nickname, &party);
                TakenRow { actor_id: uid, nickname, job, number: shown.map_or(0, |d| d.number), stats: TakenStats::default() }
            });
            row.stats.absorb(stats);
        }
        let mut out: Vec<TakenRow> = rows.into_values().collect();
        out.sort_by_key(|r| (std::cmp::Reverse(r.stats.damage), r.actor_id));
        self.taken_cache = Some((key, out.clone()));
        out
    }

    /// Where the fights behind the rows start: the window's start, or in
    /// ENC the encounter's.
    fn rows_since(&self) -> Option<i64> {
        let encounter_mode = self.target_selection_mode == TargetScope::Encounter;
        self.window_since().or_else(|| {
            encounter_mode
                .then(|| self.data_storage.current_encounter())
                .flatten()
                .filter(|e| !e.blind)
                .map(|e| e.start)
        })
    }

    /// Deaths over the fights behind the rows, one row for you and each
    /// party member, on the rows' ids where they have one. `None` for a
    /// member whose HP the game did not send during them.
    pub(super) fn death_rows(&self, dps: &DpsData) -> Vec<DeathRow> {
        if self.displayed_targets.is_empty() {
            return Vec::new();
        }
        let encounter_mode = self.target_selection_mode == TargetScope::Encounter;
        let deaths = self.data_storage.deaths_on(&self.displayed_targets, encounter_mode, self.rows_since());
        let nicknames = self.data_storage.all_names();
        let party = self.data_storage.roster_entries();
        let by_name: HashMap<&str, i32> = dps.map.iter().map(|(&id, d)| (d.nickname.trim(), id)).collect();
        let mut ids: Vec<i32> = deaths.keys().copied().chain(self.data_storage.party_ids()).collect();
        ids.sort_unstable();
        ids.dedup();
        // One row per name: a player gets a new id on every zone load.
        let mut rows: HashMap<String, DeathRow> = HashMap::new();
        for player in ids {
            let nickname = nicknames.get(&player).cloned().unwrap_or_else(|| player.to_string());
            let row = rows.entry(nickname.trim().to_string()).or_insert_with(|| {
                let uid = by_name.get(nickname.trim()).copied().unwrap_or(player);
                let shown = dps.map.get(&uid);
                let job = self.row_job(shown, &nickname, &party);
                DeathRow { actor_id: uid, nickname: nickname.clone(), job, number: shown.map_or(0, |d| d.number), deaths: None }
            });
            if let Some(&n) = deaths.get(&player) {
                row.deaths = Some(row.deaths.unwrap_or(0) + n);
            }
        }
        let mut out: Vec<DeathRow> = rows.into_values().collect();
        out.sort_by_key(|r| r.actor_id);
        out
    }

    /// The class of a taken or death row: its damage row's (`shown`), else
    /// the one known for its name, else the roster's; blank when none says.
    fn row_job(&self, shown: Option<&PersonalData>, nickname: &str, party: &HashMap<String, RosterEntry>) -> String {
        shown
            .map(|d| d.job.clone())
            .or_else(|| self.known_class(nickname))
            .or_else(|| party.get(nickname).and_then(|m| m.job).map(|j| j.class_name().to_string()))
            .unwrap_or_default()
    }
}

/// Your (first hit, last hit) on a target, your summons' hits included:
/// `mine` holds your id and your summons'. TRAIN counts this time, not that
/// of anyone who hit the dummy before or after you.
pub(super) fn own_span(td: &TargetTotals, mine: &HashSet<i32>, summon_data: &HashMap<i32, i32>) -> Option<(i64, i64)> {
    let ours = td.actors.iter()
        .filter(|(a, _)| mine.contains(&owner_chain::resolve(**a, summon_data)));
    let (first, last) = ours.fold((i64::MAX, i64::MIN), |(f, l), (_, ad)| {
        (f.min(ad.first_damage_time), l.max(ad.last_damage_time))
    });
    (first <= last).then_some((first, last))
}

/// Total time covered by `spans` of (first hit, last hit), overlaps counted
/// once, each clipped to start no earlier than `since`.
pub(super) fn active_time(spans: impl Iterator<Item = (i64, i64)>, since: i64) -> i64 {
    let mut spans: Vec<(i64, i64)> = spans
        .filter(|&(_, last)| last >= since)
        .map(|(first, last)| (first.max(since), last))
        .collect();
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

/// The Last N columns, in seconds, longest last.
const LAST_WINDOWS_S: [i64; 3] = [10, 30, 60];

/// Adds what `ad` dealt in each Last N window up to `now`.
fn add_last_windows(sums: &mut [i64; 3], ad: &ActorTotals, now: i64) {
    for (slot, secs) in LAST_WINDOWS_S.iter().enumerate() {
        sums[slot] += ad.damage_since(now - secs * 1000);
    }
}

/// A row's Last N columns from its damage in each window.
fn set_last_columns(row: &mut PersonalData, sums: &[i64; 3]) {
    row.last10_dps = sums[0] as f64 / LAST_WINDOWS_S[0] as f64;
    row.last30_dps = sums[1] as f64 / LAST_WINDOWS_S[1] as f64;
    row.last60_dps = sums[2] as f64 / LAST_WINDOWS_S[2] as f64;
}

/// What the Last N columns of the reading on screen add up, so they move
/// on with the clock while no new damage comes: each row's actors on the
/// targets the reading was built from.
pub(super) struct Rolling {
    targets: Vec<i32>,
    rows: Vec<(i32, Vec<i32>)>,
    /// From here on every window is empty.
    until: i64,
    summed_at: i64,
}

/// A row's own time in combat, its recent damage and its hits, gathered
/// over the actors (player and summons) the row stands for.
#[derive(Default, Clone)]
struct ActorExtra {
    first: Option<i64>,
    last_hit: Option<i64>,
    last: [i64; 3],
    hits: SecondStats,
}

impl ActorExtra {
    fn add_span(&mut self, first: i64, last: i64) {
        if first > last { return; }
        self.first = Some(self.first.map_or(first, |f| f.min(first)));
        self.last_hit = Some(self.last_hit.map_or(last, |l| l.max(last)));
    }

    fn absorb(&mut self, other: &ActorExtra) {
        if let (Some(f), Some(l)) = (other.first, other.last_hit) {
            self.add_span(f, l);
        }
        for i in 0..3 {
            self.last[i] += other.last[i];
        }
        self.hits.add(&other.hits);
    }

    /// Damage over first-to-last hit, at least one second.
    fn active_dps(&self, damage: f64) -> f64 {
        match (self.first, self.last_hit) {
            (Some(f), Some(l)) => damage / ((l - f).max(1000) as f64) * 1000.0,
            _ => 0.0,
        }
    }
}

/// What one actor dealt over the chosen targets.
#[derive(Default, Clone)]
struct ActorTally {
    damage: i64,
    extra: ActorExtra,
    /// Its class on the lowest chosen target that gives one.
    class: Option<JobClass>,
}

/// A row while it is being built: the actors behind it, lowest id first.
struct RowDraft {
    row: PersonalData,
    damage: i64,
    extra: ActorExtra,
    actors: Vec<i32>,
}

/// What `ad` dealt on its target, as `tally_actors` counts it; `None` when
/// nothing from `since` on.
fn tally_one(ad: &ActorTotals, since: Option<i64>, now: i64) -> Option<ActorTally> {
    let hits = since.map_or_else(|| ad.stats_total(), |from| ad.stats_since(from));
    if since.is_some() && hits.damage == 0 {
        return None;
    }
    let mut extra = ActorExtra { hits, ..ActorExtra::default() };
    extra.add_span(since.map_or(ad.first_damage_time, |from| ad.first_damage_time.max(from)), ad.last_damage_time);
    add_last_windows(&mut extra.last, ad, now);
    Some(ActorTally { damage: hits.damage, extra, class: ad.job })
}

/// Each actor's tally on each target, kept between readings with what it
/// was counted from (the row's `seq`, the second the Last N windows end
/// in, the rows' start), so a reading counts again only what changed.
#[derive(Default)]
pub(super) struct Tallied {
    by: HashMap<(i32, i32), TalliedOne>,
    /// Readings so far; an entry the last reading did not take goes.
    reads: u64,
}

/// One actor on one target: what it was counted from, the tally, and the
/// last reading that took it.
struct TalliedOne {
    from: (u64, i64, Option<i64>),
    tally: Option<ActorTally>,
    read: u64,
}

impl Tallied {
    /// `tally_one` of `actor` on `target`, counted again only when its row,
    /// the second or `since` moved.
    fn of(&mut self, target: i32, actor: i32, ad: &ActorTotals, since: Option<i64>, now: i64) -> Option<&ActorTally> {
        let from = (ad.seq, now.div_euclid(1000), since);
        let read = self.reads;
        let kept = self.by.entry((target, actor)).or_insert_with(|| TalliedOne { from, tally: tally_one(ad, since, now), read });
        if kept.from != from {
            *kept = TalliedOne { from, tally: tally_one(ad, since, now), read };
        }
        kept.read = read;
        kept.tally.as_ref()
    }

    /// Drop what this reading did not take.
    fn end_read(&mut self, taken: usize) {
        if self.by.len() > taken {
            let read = self.reads;
            self.by.retain(|_, kept| kept.read == read);
        }
        self.reads += 1;
    }
}

/// Every actor on the chosen targets that counts, by actor id. `targets`
/// must be ascending with no repeats, so each actor's class comes from the
/// lowest target. With `since`, only damage from then on counts and an
/// actor with none is left out; without, every actor counts with its whole
/// damage.
fn tally_actors(
    data: &HashMap<i32, TargetTotals>,
    targets: &[i32],
    since: Option<i64>,
    now: i64,
    kept: &mut Tallied,
) -> BTreeMap<i32, ActorTally> {
    let mut tallies: BTreeMap<i32, ActorTally> = BTreeMap::new();
    let mut taken = 0;
    for (target, td) in targets.iter().filter_map(|t| Some((*t, data.get(t)?))) {
        taken += td.actors.len();
        for (&actor, ad) in &td.actors {
            let Some(one) = kept.of(target, actor, ad, since, now) else { continue };
            let tally = tallies.entry(actor).or_default();
            tally.damage += one.damage;
            tally.extra.absorb(&one.extra);
            tally.class = tally.class.or(one.class);
        }
    }
    kept.end_read(taken);
    tallies
}

impl DpsCalculator {
    /// One reading of the meter. Unless the clock moves the rows (ALL with
    /// a window, ENC), a reading with no new damage since the last is the
    /// last one again, its Last N columns moved on to now.
    pub(super) fn compute_dps(&mut self) -> DpsData {
        let storage = self.data_storage.clone();
        let local = storage.local_player_id();
        if local != self.last_known_local_id {
            self.last_known_local_id = local;
            self.last_damage_gen = -1;
        }
        let mode = self.target_selection_mode;
        let clock_moves_rows = mode == TargetScope::Encounter
            || (mode == TargetScope::AllTargets && self.all_targets_window_ms > 0);
        let generation = storage.damage_generation();
        if !clock_moves_rows && generation == self.last_damage_gen {
            if let Some(same) = self.rolled_reading() {
                return same;
            }
        }
        self.last_damage_gen = generation;
        if mode == TargetScope::Encounter {
            // Rows merged with the encounter's earlier segments carry no `seq`.
            self.tallied = Tallied::default();
            return self.reading_of(&storage.get_encounter_snapshot_light());
        }
        let mut copy = std::mem::take(&mut self.live_copy);
        storage.refresh_brief(&mut copy);
        let reading = self.reading_of(&copy);
        self.live_copy = copy;
        reading
    }

    /// A new reading from `data`, every target the mode may show.
    fn reading_of(&mut self, data: &HashMap<i32, TargetTotals>) -> DpsData {
        let storage = self.data_storage.clone();
        let mode = self.target_selection_mode;
        let now = now_ms();
        let window_start = self.window_since();
        let since = self.rows_since();
        let owners = self.owners_now();
        let picked = self.choose_targets(data, &owners, window_start, now);

        let mut reading = DpsData::new();
        reading.local_player_id = self.last_known_local_id;
        reading.dungeon_id = storage.current_dungeon_id();
        reading.target_mode = mode.id().to_string();
        if !picked.targets.is_empty() {
            self.displayed_targets = picked.targets.clone();
        }
        let tracked = picked.tracked;
        self.current_target = tracked;
        reading.target_id = tracked;
        reading.target_name = picked.name;
        reading.target_mob_code = storage.enemy_code(tracked).unwrap_or(0);
        storage.focus_target(tracked);
        if tracked != 0 {
            reading.target_max_hp = storage.enemy_max_hp(tracked).map_or(0, i64::from);
            reading.target_current_hp = storage.enemy_hp_now(tracked).map_or(-1, i64::from);
            reading.target_out_of_combat = storage.fight_ended_by_server(tracked);
            if let Some(fight) = data.get(&tracked) {
                reading.target_enrage_ms = fight.enrage_at.map_or(0, |at| at - fight.first_damage_time);
            }
        }

        let tallies = tally_actors(data, &picked.targets, since, now, &mut self.tallied);
        if tallies.is_empty() {
            return self.reading_with_no_damage(reading, window_start.is_some());
        }
        let battle_time = self.battle_time_of(data, &owners, &picked.targets, tracked, window_start);
        let (total, rows) = self.fill_rows(&mut reading, &owners, &tallies, battle_time);
        let until = rows.iter()
            .filter_map(|(_, actors)| actors.iter().filter_map(|a| tallies[a].extra.last_hit).max())
            .max()
            .map_or(i64::MIN, |last| last + (LAST_WINDOWS_S[2] + 1) * 1000);
        self.rolling = Some(Rolling { targets: picked.targets, rows, until, summed_at: now });
        self.complete_row_set(&mut reading);
        reading.battle_time = battle_time;
        self.displayed_battle_time = battle_time;
        reading.target_total_damage = total;
        if reading.target_max_hp > 0 && tracked != 0 && storage.has_died(tracked) {
            reading.target_total_damage = reading.target_max_hp;
            reading.target_current_hp = 0;
        }
        self.last_dps_snapshot = Some(reading.clone());
        reading
    }

    /// Nothing counts on the chosen targets: the last rows stay up under
    /// the new target, less anyone who has since turned enemy. A windowed
    /// ALL has nothing to keep.
    fn reading_with_no_damage(&mut self, fresh: DpsData, windowed: bool) -> DpsData {
        if windowed {
            self.last_dps_snapshot = None;
        }
        let storage = self.data_storage.clone();
        let Some(last) = self.last_dps_snapshot.as_mut() else {
            self.rolling = None;
            let mut empty = fresh;
            self.complete_row_set(&mut empty);
            self.last_dps_snapshot = Some(empty.clone());
            return empty;
        };
        last.map.retain(|&id, _| !storage.is_enemy(id));
        last.target_name = fresh.target_name;
        last.target_mode = fresh.target_mode;
        last.target_id = fresh.target_id;
        last.target_max_hp = fresh.target_max_hp;
        last.target_current_hp = fresh.target_current_hp;
        last.target_enrage_ms = fresh.target_enrage_ms;
        last.target_out_of_combat = fresh.target_out_of_combat;
        last.dungeon_id = fresh.dungeon_id;
        last.target_total_damage = 0;
        let mut shown = last.clone();
        self.complete_row_set(&mut shown);
        shown
    }

    /// The last reading, its Last N columns summed again whenever a new
    /// second has begun and a window may still hold damage. `None` before
    /// a first reading.
    fn rolled_reading(&mut self) -> Option<DpsData> {
        let now = now_ms();
        let shown = self.last_dps_snapshot.as_mut()?;
        if let Some(roll) = self.rolling.as_mut() {
            if roll.summed_at < roll.until && now.div_euclid(1000) != roll.summed_at.div_euclid(1000) {
                self.data_storage.refresh_brief(&mut self.live_copy);
                let data = &self.live_copy;
                for (id, actors) in &roll.rows {
                    let Some(row) = shown.map.get_mut(id) else { continue };
                    let mut sums = [0; 3];
                    for td in roll.targets.iter().filter_map(|t| data.get(t)) {
                        for ad in actors.iter().filter_map(|a| td.actors.get(a)) {
                            add_last_windows(&mut sums, ad, now);
                        }
                    }
                    set_last_columns(row, &sums);
                }
                roll.summed_at = now;
            }
        }
        // A reading with no damage keeps the party members with no row out
        // of what it stores, so they join again here.
        let mut same = shown.clone();
        self.add_idle_party_rows(&mut same);
        Some(same)
    }

    /// How long the reading's fight lasted, as the rows' DPS divides by.
    fn battle_time_of(
        &self,
        data: &HashMap<i32, TargetTotals>,
        owners: &Owners,
        targets: &[i32],
        tracked: i32,
        window_start: Option<i64>,
    ) -> i64 {
        let mode = self.target_selection_mode;
        if mode == TargetScope::Encounter {
            if let Some(enc) = self.data_storage.current_encounter().filter(|e| !e.blind) {
                return enc.duration();
            }
        }
        if tracked != 0 {
            return data.get(&tracked).map_or(0, |td| (td.last_damage_time - td.first_damage_time).max(0));
        }
        let fights = targets.iter().filter_map(|t| data.get(t));
        if mode == TargetScope::TrainTargets {
            return active_time(fights.filter_map(|td| own_span(td, &owners.mine, &owners.summons)), i64::MIN);
        }
        active_time(fights.map(|td| (td.first_damage_time, td.last_damage_time)), window_start.unwrap_or(i64::MIN))
    }

    /// The damage rows, one per name, into `reading`. Returns the damage of
    /// every row built, dropped rows included, and the actors behind each
    /// row kept.
    fn fill_rows(
        &mut self,
        reading: &mut DpsData,
        owners: &Owners,
        tallies: &BTreeMap<i32, ActorTally>,
        battle_time: i64,
    ) -> (i64, Vec<(i32, Vec<i32>)>) {
        let storage = self.data_storage.clone();
        let summons = &owners.summons;
        let names = storage.all_names();
        let counted: HashMap<i32, i64> = tallies.iter().map(|(actor, tally)| (*actor, tally.damage)).collect();
        let canonical = merge_name_aliases(&counted, summons, &names, owners.me);

        let mut drafts: BTreeMap<i32, RowDraft> = BTreeMap::new();
        let mut total = 0i64;
        for (&actor, tally) in tallies {
            let owner = owner_chain::resolve(actor, &summons);
            if owner <= 0 {
                continue;
            }
            let name = display_name_for(owner, &names, &summons);
            let id = canonical.get(&name).copied().unwrap_or(owner);
            let draft = drafts.entry(id).or_insert_with(|| RowDraft {
                row: PersonalData::with_job(name.clone(), self.known_class(&name).unwrap_or_default()),
                damage: 0,
                extra: ActorExtra::default(),
                actors: Vec::new(),
            });
            draft.damage += tally.damage;
            draft.extra.absorb(&tally.extra);
            draft.actors.push(actor);
            total += tally.damage;
        }

        let roster = storage.roster_entries();
        let mut kept = Vec::new();
        for (id, mut draft) in drafts {
            if id == UNATTRIBUTED_ID {
                draft.row.job = UNKNOWN_CLASS.to_string();
            } else if draft.row.job.is_empty() {
                // The row's own actor first, then the lowest actor id.
                let own = draft.actors.contains(&id).then(|| tallies[&id].class).flatten();
                let class = own.or_else(|| draft.actors.iter().find_map(|a| tallies[a].class));
                if let Some(class) = class {
                    draft.row.job = class.class_name().to_string();
                    self.remember_class(&draft.row.nickname, &draft.row.job);
                }
            }
            if draft.row.job.is_empty() {
                if !owners.mine.contains(&id) {
                    continue;
                }
                draft.row.job = self.known_class(&draft.row.nickname).unwrap_or_else(|| UNKNOWN_CLASS.to_string());
            }
            let amount = draft.damage as f64;
            let row = &mut draft.row;
            row.combat_power = roster.get(&row.nickname).map_or(0, |m| m.combat_power);
            row.amount = amount;
            row.dps = amount / (battle_time.max(1000) as f64) * 1000.0;
            row.active_dps = draft.extra.active_dps(amount);
            set_last_columns(row, &draft.extra.last);
            row.hits = draft.extra.hits.hits;
            row.crit_hits = draft.extra.hits.crits;
            row.max_hit = draft.extra.hits.max_hit;
            row.damage_contribution = if total == 0 { 0.0 } else { amount / total as f64 * 100.0 };
            reading.map.insert(id, draft.row);
            kept.push((id, draft.actors));
        }
        (total, kept)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_kit::{
        land, meter, meter_on, read_in, storage_for, CLASSLESS_SKILL, GLADIATOR_SKILL, SORCERER_SKILL,
    };
    use super::*;
    use crate::clock::set_override;
    use crate::combat::data_storage::DataStorage;

    /// A fresh meter's MOST DAMAGE reading.
    fn heaviest(storage: &Arc<DataStorage>) -> DpsData {
        read_in(&mut meter(storage), "mostDamage")
    }

    #[test]
    fn a_chosen_target_with_no_fight_adds_no_actor() {
        let s = storage_for(None);
        land(&s, 100, 900, 1_000, GLADIATOR_SKILL, 400);
        let data = s.copy_targets_brief();
        let tallies = tally_actors(&data, &[900, 999], None, 1_000, &mut Tallied::default());
        assert_eq!(tallies.keys().copied().collect::<Vec<_>>(), vec![100]);
        assert_eq!(tallies[&100].damage, 400);
        assert!(tally_actors(&data, &[999], None, 1_000, &mut Tallied::default()).is_empty());
        set_override(None);
    }

    /// The rows on screen are covered in mod.rs; this is what is stored.
    #[test]
    fn a_window_with_nothing_in_it_stores_an_empty_reading() {
        let s = storage_for(Some(100));
        land(&s, 100, 900, 1_000, GLADIATOR_SKILL, 400);
        let mut calc = meter(&s);
        calc.set_all_targets_window_ms(10_000);
        read_in(&mut calc, "allTargets");
        set_override(Some(60_000));
        calc.get_dps();
        assert!(calc.last_dps_snapshot.as_ref().is_some_and(|kept| kept.map.is_empty()));
        set_override(None);
    }

    #[test]
    fn no_new_damage_gives_the_same_reading_back() {
        let s = storage_for(Some(100));
        land(&s, 100, 900, 1_000, GLADIATOR_SKILL, 400);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("mostDamage");
        let first = serde_json::to_string(&calc.get_dps()).unwrap();
        set_override(Some(6_000));
        let second = serde_json::to_string(&calc.get_dps()).unwrap();
        assert_eq!(first, second);
        set_override(None);
    }

    #[test]
    fn no_new_damage_keeps_the_party_members_with_no_row() {
        // A party formed before any fight: the reading after each roster had
        // the members and their power, the next one nothing, and the power on
        // the meter blinked.
        set_override(Some(1_000));
        let s = storage_for(Some(100));
        let mut calc = meter(&s);
        calc.set_target_selection_mode("mostDamage");
        assert!(calc.get_dps().map.is_empty());
        let mate = RosterEntry { slot: 2, combat_power: 60_000, ..Default::default() };
        s.replace_roster(vec![("Mate".into(), mate)], false);
        let first = calc.get_dps();
        assert_eq!(first.map.values().map(|p| p.combat_power).collect::<Vec<_>>(), [60_000]);
        set_override(Some(1_100));
        let second = calc.get_dps();
        assert_eq!(serde_json::to_string(&first).unwrap(), serde_json::to_string(&second).unwrap());
        set_override(None);
    }

    #[test]
    fn the_last_columns_fall_while_nobody_hits() {
        let s = storage_for(Some(100));
        for at in [1_000, 2_000, 3_000] {
            land(&s, 100, 900, at, GLADIATOR_SKILL, 600);
        }
        let mut calc = meter(&s);
        calc.set_target_selection_mode("mostDamage");
        let mut read_at = |at: i64| {
            set_override(Some(at));
            let row = calc.get_dps().map[&100].clone();
            (row.last10_dps, row.last30_dps, row.last60_dps, row.amount)
        };
        assert_eq!(read_at(3_000), (180.0, 60.0, 30.0, 1800.0));
        assert_eq!(read_at(12_500), (120.0, 60.0, 30.0, 1800.0), "second 1 left the 10 s window");
        assert_eq!(read_at(40_000), (0.0, 0.0, 30.0, 1800.0));
        assert_eq!(read_at(64_000), (0.0, 0.0, 0.0, 1800.0), "the rest stays");
        land(&s, 100, 900, 65_000, GLADIATOR_SKILL, 600);
        assert_eq!(read_at(65_000), (60.0, 20.0, 10.0, 600.0), "a hit after the quiet limit is a new fight");
        set_override(None);
    }

    #[test]
    fn two_names_are_never_one_row() {
        let s = storage_for(Some(100));
        s.name_entity(100, "A");
        s.name_entity(200, "B");
        land(&s, 100, 900, 1_000, GLADIATOR_SKILL, 400);
        land(&s, 200, 900, 1_100, SORCERER_SKILL, 300);
        let shown = heaviest(&s);
        let mut rows: Vec<(i32, &str, f64)> = shown.map.iter().map(|(&id, r)| (id, r.nickname.as_str(), r.amount)).collect();
        rows.sort_by_key(|r| r.0);
        assert_eq!(rows, vec![(100, "A", 400.0), (200, "B", 300.0)]);
        set_override(None);
    }

    #[test]
    fn rows_of_no_damage_have_no_dps_and_no_share() {
        let s = storage_for(Some(100));
        land(&s, 100, 900, 1_000, GLADIATOR_SKILL, 0);
        let shown = heaviest(&s);
        let row = &shown.map[&100];
        assert_eq!((row.amount, row.dps, row.damage_contribution), (0.0, 0.0, 0.0));
        assert_eq!(shown.target_total_damage, 0);
        set_override(None);
    }

    #[test]
    fn row_numbers_follow_the_stated_float_order() {
        let s = storage_for(Some(100));
        land(&s, 100, 900, 1_000, GLADIATOR_SKILL, 759);
        land(&s, 200, 900, 1_800, SORCERER_SKILL, 105);
        let shown = heaviest(&s);
        assert_eq!(shown.battle_time, 800);
        assert_eq!(shown.target_total_damage, 864);
        let row = &shown.map[&100];
        assert_eq!((row.dps, row.active_dps), (759.0, 759.0));
        assert_eq!((row.last10_dps, row.last30_dps, row.last60_dps), (75.9, 25.3, 12.65));
        assert_eq!(row.damage_contribution, 87.84722222222221);
        assert_eq!((row.hits, row.crit_hits, row.max_hit), (1, 0, 759));
        set_override(None);
    }

    /// Player 100 and its summons 500 and 501 make one row: its class is
    /// 100's own, else that of the lowest actor id that has one.
    #[test]
    fn a_rows_class_is_its_own_actors_then_the_lowest_actors() {
        let class_of = |classes: [Option<JobClass>; 3]| {
            let s = storage_for(None);
            s.name_entity(100, "A");
            s.link_pet_owner(100, 500);
            s.link_pet_owner(100, 501);
            let tallies: BTreeMap<i32, ActorTally> = [100, 501, 500]
                .into_iter()
                .zip(classes)
                .map(|(actor, class)| (actor, ActorTally { damage: 100, class, ..Default::default() }))
                .collect();
            let mut calc = meter(&s);
            let owners = calc.owners_now();
            let mut reading = DpsData::new();
            assert_eq!(calc.fill_rows(&mut reading, &owners, &tallies, 1_000).0, 300);
            assert_eq!(reading.map.keys().copied().collect::<Vec<_>>(), vec![100]);
            reading.map[&100].job.clone()
        };
        let (glad, sorc) = (Some(JobClass::Gladiator), Some(JobClass::Sorcerer));
        // Classes given for 100, 501, 500 in that order.
        assert_eq!(class_of([glad, sorc, sorc]), JobClass::Gladiator.class_name());
        assert_eq!(class_of([None, glad, sorc]), JobClass::Sorcerer.class_name());
        assert_eq!(class_of([None, glad, None]), JobClass::Gladiator.class_name());
    }

    #[test]
    fn a_class_found_once_stays_with_the_name() {
        let s = storage_for(Some(100));
        s.name_entity(100, "Me");
        s.name_entity(300, "Other");
        land(&s, 300, 900, 1_000, SORCERER_SKILL, 100);
        let mut calc = meter(&s);
        calc.set_target_selection_mode("mostDamage");
        calc.get_dps();
        assert_eq!(calc.known_class("other").as_deref(), Some(JobClass::Sorcerer.class_name()));
        // Classless hits: the other player keeps the class, you show as Unknown.
        land(&s, 300, 901, 2_000, CLASSLESS_SKILL, 500);
        land(&s, 100, 901, 2_000, CLASSLESS_SKILL, 500);
        let shown = calc.get_dps();
        assert_eq!(shown.target_id, 901);
        assert_eq!(shown.map[&300].job, JobClass::Sorcerer.class_name());
        assert_eq!(shown.map[&100].job, "Unknown");
        set_override(None);
    }

    #[test]
    fn a_stranger_with_no_class_is_left_off_but_counts_in_the_total() {
        let s = storage_for(Some(100));
        land(&s, 100, 900, 1_000, GLADIATOR_SKILL, 300);
        land(&s, 300, 900, 1_000, CLASSLESS_SKILL, 100);
        let shown = heaviest(&s);
        assert_eq!(shown.map.keys().copied().collect::<Vec<_>>(), vec![100]);
        assert_eq!(shown.target_total_damage, 400);
        assert_eq!(shown.map[&100].damage_contribution, 75.0);
        set_override(None);
    }

    /// The meter's read at the end of a 25 minute world boss fight, 400
    /// against 400: 400 named players with 12 skills each (50 with a
    /// spirit, 4 in your party), 400 strangers and 50 effects on the boss,
    /// about 450,000 hits, the boss hitting 20 players every 2 s, then 10
    /// new hits between reads 100 ms apart. Run with `cargo test --release
    /// --lib -- --ignored meter_read_time --nocapture`.
    #[test]
    #[ignore]
    fn meter_read_time() {
        use crate::entity::taken::{TakenHit, TakenKind};
        let s = storage_for(Some(1_000));
        s.append_mob(800, 700);
        s.register_boss(800);
        (1_000..1_400).for_each(|p| s.name_entity(p, &format!("P{p}")));
        let party = (1_000..1_004).map(|p| (format!("P{p}"), RosterEntry { slot: (p - 999) as u8, ..Default::default() }));
        s.replace_roster(party.collect(), true);
        (1_000..1_050).for_each(|p| s.link_pet_owner(p, p + 4_000));
        let actors: Vec<i32> = (1_000..1_400).chain(5_000..5_050).chain(2_000..2_400).chain(6_000..6_050).collect();
        let hit = |actor: i32, at: i64, n: i32| {
            let skill = match actor {
                6_000.. => SORCERER_SKILL,
                2_000.. => CLASSLESS_SKILL,
                _ if actor % 2 == 0 => GLADIATOR_SKILL + n % 12 * 10_000,
                _ => SORCERER_SKILL + n % 12 * 10_000,
            };
            land(&s, actor, 800, at, skill, 1_000 + i64::from(actor % 1_000));
        };
        for (n, at) in (1_000..1_501_000).step_by(1_000).enumerate() {
            for &actor in actors.iter().filter(|&&a| (a + n as i32) % 3 == 0) {
                hit(actor, at + i64::from(actor % 997), n as i32);
            }
            if at % 2_000 == 0 {
                for p in (1_000..1_400).skip(n / 2 % 20).step_by(20) {
                    s.append_taken(TakenHit::plain(at, p, 800, 1_800_510, 300, TakenKind::Hit));
                }
            }
        }
        let mut calc = meter_on(&s, r#"{"700":{"name":"Boss","isBoss":true}}"#);
        let reads = 200;
        let mut rows = 0;
        let started = std::time::Instant::now();
        for (n, at) in (1_501_000..).step_by(100).take(reads).enumerate() {
            (0..10).for_each(|i| hit(actors[(n * 10 + i) % actors.len()], at, n as i32));
            rows = calc.get_dps().map.len();
        }
        println!("{:.2} ms per read, {rows} rows", started.elapsed().as_secs_f64() * 1000.0 / reads as f64);
        set_override(None);
    }
}
