//! Target selection: the modes, and which targets each one puts on the meter.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use crate::clock::now_ms;
use crate::combat::data_storage::{TargetTotals, DAMAGE_HISTORY_MS};
use crate::entity::owner_chain;

use super::DpsCalculator;

/// How long TARGET mode keeps its target after your hits moved on to another
/// one: long enough to bridge the gaps in a rotation, short enough to follow
/// a real switch.
pub(super) const TARGET_HOLD_MS: i64 = 3_000;

/// How often idle targets are retired. See `retire_idle_targets`.
const RETIRE_EVERY_MS: i64 = 5_000;

/// BOSS mode weighs bosses by your and your party's damage over this long,
/// the meter's shortest "last N s" window.
const RECENT_MS: i64 = 10_000;
/// Another boss takes the meter from the one on screen only with more than
/// this many times its recent damage from you and your party.
const TAKE_OVER_FACTOR: i64 = 2;

/// The shortest ALL window; anything shorter shows nothing between hits.
const MIN_WINDOW_MS: i64 = 10_000;

/// Target names of the modes that show more than one target. The front end
/// translates them.
const ENCOUNTER_NAME: &str = "Encounter";
const ALL_TARGETS_NAME: &str = "All Targets";
const TRAIN_NAME: &str = "Train";

/// Which targets the meter's rows are about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetScope {
    BossTargets,
    MostDamage,
    MostRecent,
    LastHitByMe,
    AllTargets,
    TrainTargets,
    Encounter,
}

/// The boss BOSS mode shows: the one you and your party hit hardest lately,
/// then the one you hit last, then the lowest id. The boss on screen stays
/// until another clearly took over. Newest hit by anyone used to win, so
/// two boss-flagged scarecrows flipped several times a second: your spirit's
/// spill-over and a stranger's hits on the next one made it newest, and hits
/// in the same ms fell to map order.
fn steady_boss(
    candidates: &[i32],
    shown: i32,
    combat_data: &HashMap<i32, TargetTotals>,
    is_ours_actor: impl Fn(i32) -> bool,
    now: i64,
) -> Option<i32> {
    let weigh = |tid: i32| {
        let mut recent = 0i64;
        let mut last = i64::MIN;
        if let Some(td) = combat_data.get(&tid) {
            for (_, a) in td.actors.iter().filter(|(id, _)| is_ours_actor(**id)) {
                recent += a.damage_since(now - RECENT_MS);
                last = last.max(a.last_damage_time);
            }
        }
        (recent, last)
    };
    let best = candidates.iter().copied().max_by_key(|&tid| {
        let (recent, last) = weigh(tid);
        (recent, last, Reverse(tid))
    })?;
    if best != shown && candidates.contains(&shown) && weigh(best).0 <= TAKE_OVER_FACTOR * weigh(shown).0 {
        return Some(shown);
    }
    Some(best)
}

impl DpsCalculator {
    /// The mode's id, as `set_target_selection_mode` takes it.
    pub fn target_selection_mode_id(&self) -> &'static str {
        self.target_selection_mode.id()
    }

    /// Save & Clear (the meter's reset): ends every fight and starts the
    /// meter afresh. The ended fights go to the ended segments, which the
    /// next save writes as finished. Capture, the game port and the ping go
    /// on; names are learned again from the packets that follow, and you
    /// stay named.
    pub fn reset_combat(&mut self) {
        self.reset_focus(true);
        self.data_storage.forget_names();
        self.data_storage.drop_ghost_rows();
    }

    /// Retire the idle targets this mode does not show, every few seconds:
    /// in the open world they piled up for as long as the meter ran. The
    /// rows on screen stay; so does everything ALL shows, which without a
    /// window is everything since the zone change.
    pub(super) fn retire_idle_targets(&mut self) {
        let now = now_ms();
        if now.saturating_sub(self.last_retire_ms) < RETIRE_EVERY_MS {
            return;
        }
        self.last_retire_ms = now;
        let shown: HashSet<i32> = self.displayed_targets.iter().copied().collect();
        let mode = self.target_selection_mode;
        let since = self.window_since();
        let mob_data = if mode == TargetScope::TrainTargets { self.live.enemy_code_map() } else { HashMap::new() };
        let npc_lookup = &self.npc_lookup;
        self.live.retire_idle(now, |tid, td| {
            shown.contains(&tid)
                || match mode {
                    TargetScope::AllTargets => since.is_none_or(|since| td.last_damage_time >= since),
                    TargetScope::TrainTargets => mob_data.get(&tid).is_some_and(|&code| npc_lookup.npc_is_dummy(code)),
                    _ => false,
                }
        });
    }
}

impl TargetScope {
    const EVERY: [Self; 7] = [
        Self::BossTargets,
        Self::MostDamage,
        Self::MostRecent,
        Self::LastHitByMe,
        Self::AllTargets,
        Self::TrainTargets,
        Self::Encounter,
    ];

    /// The id the settings file stores for the mode; the front end and the
    /// replay command line use the same ids.
    pub fn id(&self) -> &'static str {
        match self {
            Self::BossTargets => "bossTargets",
            Self::MostDamage => "mostDamage",
            Self::MostRecent => "mostRecent",
            Self::LastHitByMe => "lastHitByMe",
            Self::AllTargets => "allTargets",
            Self::TrainTargets => "trainTargets",
            Self::Encounter => "encounter",
        }
    }

    /// The mode a saved id names. Text that names none is `LastHitByMe`.
    pub fn from_id(id: &str) -> Self {
        Self::EVERY.into_iter().find(|mode| mode.id() == id).unwrap_or(Self::LastHitByMe)
    }
}

/// What a mode puts on the meter in one reading.
pub(super) struct Picked {
    /// Ascending.
    pub(super) targets: Vec<i32>,
    pub(super) name: String,
    /// The one target the reading is about; 0 for a mode of many.
    pub(super) tracked: i32,
}

impl Picked {
    fn many(mut targets: Vec<i32>, name: &str) -> Self {
        targets.sort_unstable();
        Picked { targets, name: name.to_string(), tracked: 0 }
    }
}

/// Who is who for one reading: read once, used by the choice and the rows.
pub(super) struct Owners {
    /// Summon to owner.
    pub(super) summons: HashMap<i32, i32>,
    /// You, or `None` before the meter knows you.
    pub(super) me: Option<i32>,
    /// You and your summons; empty before the meter knows you.
    pub(super) mine: HashSet<i32>,
}

/// `me` and every summon whose chain of owners ends at `me`.
fn ids_ending_at(me: i32, summon_data: &HashMap<i32, i32>) -> HashSet<i32> {
    let ends_at_me = |summon: &&i32| owner_chain::resolve(**summon, summon_data) == me;
    std::iter::once(me).chain(summon_data.keys().filter(ends_at_me).copied()).collect()
}

impl DpsCalculator {
    /// Switches the mode by its id. The target on screen stays tracked, so
    /// TARGET's hold and BOSS's keep start from it; the old rows do not.
    pub fn set_target_selection_mode(&mut self, id: &str) {
        let mode = TargetScope::from_id(id);
        if mode != self.target_selection_mode {
            self.target_selection_mode = mode;
            self.forget_reading(false);
        }
        self.publish_view();
    }

    /// ALL's window in ms; 0 or less turns it off.
    pub fn set_all_targets_window_ms(&mut self, ms: i64) {
        let ms = if ms > 0 { ms.clamp(MIN_WINDOW_MS, DAMAGE_HISTORY_MS) } else { 0 };
        if ms != self.all_targets_window_ms {
            self.all_targets_window_ms = ms;
            self.last_damage_gen = -1;
        }
    }

    /// Where ALL's window starts now, when the mode is ALL and it has one.
    pub(super) fn window_since(&self) -> Option<i64> {
        let windowed = self.target_selection_mode == TargetScope::AllTargets && self.all_targets_window_ms > 0;
        windowed.then(|| now_ms() - self.all_targets_window_ms)
    }

    /// Forgets the target and the rows on screen; with `clear_damage` the
    /// fights too. Names and classes stay.
    pub fn reset_focus(&mut self, clear_damage: bool) {
        self.forget_reading(true);
        if clear_damage {
            self.data_storage.flush();
        }
        self.publish_view();
    }

    /// Drops the stored reading and the targets on screen, so the next read
    /// builds anew; with `drop_target` the tracked target goes too.
    pub(super) fn forget_reading(&mut self, drop_target: bool) {
        self.held = None;
        self.last_shown = None;
        self.data_storage = self.live.clone();
        self.last_dps_snapshot = None;
        self.rolling = None;
        self.displayed_targets.clear();
        self.last_damage_gen = -1;
        if drop_target {
            self.current_target = 0;
            self.data_storage.focus_target(0);
        }
    }

    /// A target's name from the NPC table; empty when unknown.
    pub(super) fn target_label(&self, target_id: i32) -> String {
        match self.data_storage.enemy_code(target_id) {
            Some(code) => self.npc_lookup.npc_title(code),
            None => String::new(),
        }
    }

    /// You and everything you summoned, or `None` before the meter knows you.
    pub(super) fn own_side_ids(&self, summon_data: &HashMap<i32, i32>) -> Option<HashSet<i32>> {
        let local = self.data_storage.local_player_id()?;
        Some(ids_ending_at(local as i32, summon_data))
    }

    /// Who is who now: the summon links, you, and your summons.
    pub(super) fn owners_now(&self) -> Owners {
        let summons = self.data_storage.pet_owners();
        let me = self.data_storage.local_player_id().map(|v| v as i32);
        let mine = me.map(|me| ids_ending_at(me, &summons)).unwrap_or_default();
        Owners { summons, me, mine }
    }

    /// The targets the mode shows in this reading, from `data` (ENC: the
    /// encounter's fights; otherwise every fight).
    pub(super) fn choose_targets(
        &self,
        data: &HashMap<i32, TargetTotals>,
        owners: &Owners,
        window_start: Option<i64>,
        now: i64,
    ) -> Picked {
        let one = |target: Option<i32>| match target {
            Some(t) => Picked { targets: vec![t], name: self.target_label(t), tracked: t },
            None => Picked { targets: Vec::new(), name: String::new(), tracked: 0 },
        };
        match self.target_selection_mode {
            TargetScope::MostDamage => one(
                data.iter()
                    .max_by_key(|&(&t, td)| (td.total_damage, td.last_damage_time, Reverse(t)))
                    .map(|(&t, _)| t),
            ),
            TargetScope::MostRecent => one(
                data.iter()
                    .max_by_key(|&(&t, td)| (td.last_damage_time, td.total_damage, Reverse(t)))
                    .map(|(&t, _)| t),
            ),
            TargetScope::LastHitByMe => one(self.my_latest_target(data, owners)),
            TargetScope::BossTargets => one(self.boss_or_trash(data, owners, now)),
            TargetScope::Encounter => {
                let fought = match self.data_storage.current_encounter() {
                    Some(enc) if !enc.blind => enc.targets.iter().copied().filter(|t| data.contains_key(t)).collect(),
                    _ => Vec::new(),
                };
                Picked::many(fought, ENCOUNTER_NAME)
            }
            TargetScope::AllTargets => {
                let recent = data.iter()
                    .filter(|(_, td)| window_start.is_none_or(|start| td.last_damage_time >= start))
                    .map(|(&t, _)| t)
                    .collect();
                Picked::many(recent, ALL_TARGETS_NAME)
            }
            TargetScope::TrainTargets => Picked::many(self.my_dummies(data, owners), TRAIN_NAME),
        }
    }

    /// TARGET: the target you hit last, held while you go on hitting it.
    fn my_latest_target(&self, data: &HashMap<i32, TargetTotals>, owners: &Owners) -> Option<i32> {
        owners.me?;
        let my_hits: HashMap<i32, i64> = data
            .iter()
            .filter_map(|(&t, td)| {
                td.actors
                    .iter()
                    .filter(|&(&a, _)| owners.mine.contains(&owner_chain::resolve(a, &owners.summons)))
                    .map(|(_, ad)| ad.last_damage_time)
                    .max()
                    .map(|at| (t, at))
            })
            .collect();
        // Several targets hit in one ms: the highest id, as recorded.
        let (newest, newest_at) = my_hits.iter().map(|(&t, &at)| (t, at)).max_by_key(|&(t, at)| (at, t))?;
        match my_hits.get(&self.current_target) {
            Some(&held_at) if newest_at - held_at <= TARGET_HOLD_MS => Some(self.current_target),
            _ => Some(newest),
        }
    }

    /// BOSS: your party's boss; in the open world, with no boss, your
    /// heaviest mob.
    fn boss_or_trash(&self, data: &HashMap<i32, TargetTotals>, owners: &Owners, now: i64) -> Option<i32> {
        let storage = &self.data_storage;
        // `None`: the meter does not know you yet, so every actor counts as
        // ours; it also keeps a stranger's mob off the meter below.
        let party: Option<HashSet<i32>> = owners.me.map(|_| {
            let mut ids = owners.mine.clone();
            ids.extend(storage.party_ids());
            ids
        });
        let ours_actor =
            |actor: i32| party.as_ref().is_none_or(|p| p.contains(&owner_chain::resolve(actor, &owners.summons)));
        let ours: Vec<(i32, &TargetTotals)> = data
            .iter()
            .filter(|(_, td)| td.actors.keys().any(|&a| ours_actor(a)))
            .map(|(&t, td)| (t, td))
            .collect();
        let newest = ours.iter().map(|(_, td)| td.last_damage_time).max().unwrap_or(i64::MIN);
        let boss = |t: i32| storage.enemy_code(t).is_some_and(|code| self.npc_lookup.is_boss(code));
        // A dead boss gives way once you have hit something after it; until
        // then its rows stay up.
        let alive_bosses: Vec<i32> = ours
            .iter()
            .filter(|&&(t, td)| boss(t) && !(storage.has_died(t) && td.last_damage_time < newest))
            .map(|&(t, _)| t)
            .collect();
        if !alive_bosses.is_empty() {
            return steady_boss(&alive_bosses, self.current_target, data, &ours_actor, now);
        }
        if storage.current_dungeon_id() > 0 || party.is_none() {
            return None;
        }
        ours.iter()
            .filter(|&&(t, _)| !boss(t))
            .max_by_key(|&&(t, td)| (td.total_damage, t == self.current_target, Reverse(t)))
            .map(|&(t, _)| t)
    }

    /// TRAIN: the dummies you hit yourself, else those your summons hit.
    fn my_dummies(&self, data: &HashMap<i32, TargetTotals>, owners: &Owners) -> Vec<i32> {
        let Some(me) = owners.me else {
            return Vec::new();
        };
        let dummies: Vec<(i32, &TargetTotals)> = data
            .iter()
            .filter(|&(&t, _)| self.data_storage.enemy_code(t).is_some_and(|code| self.npc_lookup.npc_is_dummy(code)))
            .map(|(&t, td)| (t, td))
            .collect();
        let by_me: Vec<i32> = dummies.iter().filter(|(_, td)| td.actors.contains_key(&me)).map(|&(t, _)| t).collect();
        if !by_me.is_empty() {
            return by_me;
        }
        let my_summon_hit =
            |td: &TargetTotals| td.actors.keys().any(|a| owners.mine.contains(&owner_chain::resolve(*a, &owners.summons)));
        dummies.into_iter().filter(|&(_, td)| my_summon_hit(td)).map(|(t, _)| t).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_kit::{land, meter_on, read_in, storage_for, GLADIATOR_SKILL};
    use super::*;
    use crate::clock::set_override;
    use crate::combat::data_storage::DataStorage;

    /// A calculator whose NPC table has boss code 700 named "Boss".
    fn calc_on(storage: &Arc<DataStorage>) -> DpsCalculator {
        meter_on(storage, r#"{"700":{"name":"Boss","isBoss":true}}"#)
    }

    #[test]
    fn every_mode_id_reads_back_and_strange_text_means_target() {
        let ids = ["bossTargets", "mostDamage", "mostRecent", "lastHitByMe", "allTargets", "trainTargets", "encounter"];
        for (mode, id) in TargetScope::EVERY.into_iter().zip(ids) {
            assert_eq!((mode.id(), TargetScope::from_id(id)), (id, mode));
        }
        for odd in ["", "BossTargets", "x"] {
            assert_eq!(TargetScope::from_id(odd), TargetScope::LastHitByMe, "{odd:?}");
        }
    }

    #[test]
    fn the_all_window_is_off_or_between_ten_seconds_and_fifteen_minutes() {
        let mut calc = calc_on(&storage_for(None));
        for (asked, kept) in [(-5, 0), (0, 0), (5_000, 10_000), (60_000, 60_000), (2_000_000, 900_000)] {
            calc.set_all_targets_window_ms(asked);
            assert_eq!(calc.all_targets_window_ms, kept, "{asked}");
        }
        calc.last_damage_gen = 7;
        calc.set_all_targets_window_ms(2_000_000);
        assert_eq!(calc.last_damage_gen, 7, "the same window reads nothing again");
        calc.set_all_targets_window_ms(30_000);
        assert_eq!(calc.last_damage_gen, -1, "a new window reads again");
    }

    #[test]
    fn a_restart_without_clearing_keeps_the_damage_and_picks_again() {
        let s = storage_for(Some(2259));
        land(&s, 2259, 900, 1_000, GLADIATOR_SKILL, 500);
        let mut calc = calc_on(&s);
        assert_eq!(read_in(&mut calc, "lastHitByMe").target_id, 900);
        calc.reset_focus(false);
        assert_eq!((calc.current_target, s.focused_target()), (0, 0));
        assert!(calc.displayed_targets.is_empty() && calc.last_dps_snapshot.is_none());
        let again = calc.get_dps();
        assert_eq!(again.target_id, 900);
        assert_eq!(again.map[&2259].amount, 500.0);

        calc.reset_focus(true);
        let cleared = calc.get_dps();
        assert_eq!(cleared.target_id, 0);
        assert!(cleared.map.is_empty());
        set_override(None);
    }

    #[test]
    fn a_target_is_named_by_its_npc_code_or_not_at_all() {
        let s = storage_for(None);
        s.append_mob(800, 700);
        s.append_mob(801, 9_999);
        let calc = calc_on(&s);
        assert_eq!(calc.target_label(800), "Boss");
        assert_eq!(calc.target_label(801), "", "a code the table lacks");
        assert_eq!(calc.target_label(802), "", "no code");
    }

    #[test]
    fn your_ids_are_you_and_whatever_ends_at_you() {
        let s = storage_for(None);
        let calc = calc_on(&s);
        let summons: HashMap<i32, i32> = [(500, 100), (501, 500), (600, 222)].into_iter().collect();
        assert_eq!(calc.own_side_ids(&summons), None);
        s.set_local_player_id(Some(100));
        assert_eq!(calc.own_side_ids(&summons), Some([100, 500, 501].into_iter().collect()));
    }

    #[test]
    fn target_mode_holds_for_exactly_the_hold_and_no_longer() {
        let s = storage_for(Some(2259));
        land(&s, 2259, 900, 5_000, GLADIATOR_SKILL, 500);
        let mut calc = calc_on(&s);
        assert_eq!(read_in(&mut calc, "lastHitByMe").target_id, 900);
        land(&s, 2259, 901, 5_000 + TARGET_HOLD_MS, GLADIATOR_SKILL, 500);
        assert_eq!(calc.get_dps().target_id, 900);
        land(&s, 2259, 901, 5_001 + TARGET_HOLD_MS, GLADIATOR_SKILL, 500);
        assert_eq!(calc.get_dps().target_id, 901);
        set_override(None);
    }

    #[test]
    fn target_mode_needs_to_know_you() {
        let s = storage_for(None);
        land(&s, 2259, 900, 1_000, GLADIATOR_SKILL, 500);
        let shown = read_in(&mut calc_on(&s), "lastHitByMe");
        assert_eq!((shown.target_id, shown.target_name.as_str()), (0, ""));
        assert!(shown.map.is_empty());
        set_override(None);
    }

    #[test]
    fn another_boss_takes_over_only_past_twice_the_recent_damage() {
        let s = storage_for(Some(2259));
        s.append_mob(801, 700);
        s.append_mob(802, 700);
        let mut calc = calc_on(&s);
        land(&s, 2259, 801, 1_000, GLADIATOR_SKILL, 1_000);
        let shown = calc.get_dps();
        assert_eq!((shown.target_id, shown.target_name.as_str()), (801, "Boss"));
        land(&s, 2259, 802, 2_000, GLADIATOR_SKILL, 2_000);
        assert_eq!(calc.get_dps().target_id, 801, "twice is not enough");
        land(&s, 2259, 802, 2_000, GLADIATOR_SKILL, 1);
        assert_eq!(calc.get_dps().target_id, 802);
        set_override(None);
    }

    #[test]
    fn a_dead_boss_rows_stay_up_while_trash_in_an_instance_is_not_shown() {
        let s = storage_for(Some(2259));
        s.enter_dungeon(600_011);
        s.append_mob(801, 700);
        land(&s, 2259, 801, 1_000, GLADIATOR_SKILL, 500);
        land(&s, 2259, 801, 3_000, GLADIATOR_SKILL, 500);
        let mut calc = calc_on(&s);
        let fought = read_in(&mut calc, "bossTargets");
        assert_eq!((fought.target_id, fought.battle_time), (801, 2_000));
        s.note_death(801);
        land(&s, 2259, 900, 4_000, GLADIATOR_SKILL, 500);
        let after = calc.get_dps();
        assert_eq!((after.target_id, after.target_name.as_str()), (0, ""));
        assert_eq!((after.target_max_hp, after.target_current_hp, after.target_total_damage), (0, -1, 0));
        assert_eq!((after.battle_time, after.map[&2259].amount), (2_000, 1_000.0));
        assert_eq!(calc.displayed_targets, [801]);
        set_override(None);
    }

    #[test]
    fn picking_the_mode_already_on_keeps_the_reading() {
        let s = storage_for(Some(2259));
        land(&s, 2259, 900, 1_000, GLADIATOR_SKILL, 500);
        let mut calc = calc_on(&s);
        read_in(&mut calc, "mostDamage");
        calc.set_target_selection_mode("mostDamage");
        assert!(calc.last_dps_snapshot.is_some() && calc.displayed_targets == [900]);
        calc.set_target_selection_mode("mostRecent");
        assert!(calc.last_dps_snapshot.is_none() && calc.displayed_targets.is_empty());
        assert_eq!(calc.current_target, 900, "the tracked target stays");
        set_override(None);
    }

    #[test]
    fn enc_with_no_encounter_shows_nothing_under_its_name() {
        let s = storage_for(None);
        let shown = read_in(&mut calc_on(&s), "encounter");
        assert_eq!((shown.target_name.as_str(), shown.target_mode.as_str(), shown.target_id), ("Encounter", "encounter", 0));
        assert!(shown.map.is_empty());
    }

    #[test]
    fn all_targets_takes_every_target_hit_in_the_window() {
        let s = storage_for(Some(2259));
        land(&s, 2259, 900, 1_000, GLADIATOR_SKILL, 500);
        land(&s, 3000, 901, 30_000, GLADIATOR_SKILL, 500);
        let mut calc = calc_on(&s);
        calc.set_target_selection_mode("allTargets");
        let data = s.copy_targets_brief();
        let owners = calc.owners_now();
        let open = calc.choose_targets(&data, &owners, None, 30_000);
        assert_eq!((open.targets, open.name.as_str(), open.tracked), (vec![900, 901], "All Targets", 0));
        let windowed = calc.choose_targets(&data, &owners, Some(20_000), 30_000);
        assert_eq!(windowed.targets, vec![901]);
        set_override(None);
    }
}
