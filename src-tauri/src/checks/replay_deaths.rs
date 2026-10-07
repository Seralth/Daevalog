//! The deaths part of the replay report: every death of you and your party in
//! the window, with the hit before it, and the deaths in each fight the meter
//! would save.

use std::collections::BTreeMap;

use crate::combat::data_storage::{
    deaths_between, DataStorage, DeathTick, TakenTick, TargetCombatData, DEATH_SLACK_MS, MIN_SAVED_FIGHT_MS,
};
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// Deaths and hits taken, read from storage after each line of the capture,
/// so none is lost to pruning, and the fight segments seen.
#[derive(Default)]
pub(crate) struct Gather {
    deaths: Vec<(DeathTick, bool)>,
    death_seq: u64,
    taken: Vec<TakenTick>,
    taken_seq: u64,
    /// (target, first hit) -> (last hit, ours, damage)
    fights: BTreeMap<(i32, i64), (i64, bool, i64)>,
}

impl Gather {
    pub fn after_line(&mut self, storage: &DataStorage) {
        let local = storage.local_player_id().map(|v| v as i32);
        for d in storage.deaths_since(self.death_seq) {
            self.death_seq = d.seq;
            self.deaths.push((d, Some(d.player) == local));
        }
        for t in storage.taken_since(self.taken_seq) {
            self.taken_seq = t.seq;
            self.taken.push(t);
        }
    }

    pub fn note_fights<'a>(&mut self, live: impl Iterator<Item = &'a TargetCombatData>) {
        for td in live {
            self.fights.insert((td.target_id, td.first_damage_time), (td.last_damage_time, td.ours, td.total_damage));
        }
    }

    pub fn report(
        &self,
        window: (i64, i64),
        storage: &DataStorage,
        skills: &SkillLookup,
        npcs: &NpcLookup,
        out: &mut dyn FnMut(String),
    ) {
        let deaths: Vec<&(DeathTick, bool)> = self.deaths.iter().filter(|(d, _)| (window.0..=window.1).contains(&d.at)).collect();
        let known = storage.hp_known();
        let mobs = storage.get_mob_data();
        let fights: Vec<_> = self
            .fights
            .iter()
            .filter(|&(&(target, first), &(last, ours, damage))| {
                let code = mobs.get(&target).copied().unwrap_or(0);
                (npcs.is_boss(code) || npcs.is_training_dummy(code))
                    && ours
                    && damage > 0
                    && last - first >= MIN_SAVED_FIGHT_MS
                    && first <= window.1
                    && last >= window.0
            })
            .collect();
        if deaths.is_empty() && fights.is_empty() {
            return;
        }
        out(format!("\n== deaths of you and your party in the window: {}", deaths.len()));
        for (d, you) in &deaths {
            let who = if *you { "you" } else { "party" };
            let hit = self
                .taken
                .iter()
                .rev()
                .find(|t| t.hit.target == d.player && t.hit.damage > 0 && t.hit.at <= d.at && d.at - t.hit.at <= 1_000);
            let hit = match hit {
                Some(t) => format!(
                    "{} {} from {} for {}",
                    t.hit.skill,
                    skills.get_skill_name(t.hit.skill),
                    if t.source_code == 0 { "?".to_string() } else { npcs.get_npc_name(t.source_code) },
                    t.hit.damage
                ),
                None => "no hit seen".to_string(),
            };
            out(format!("death {} player {} ({who}) from {} HP: {hit}", d.at, d.player, d.before));
        }
        for (&(target, first), &(last, _, _)) in fights {
            let by = deaths_between(self.deaths.iter().map(|(d, _)| d), &known, first, last + DEATH_SLACK_MS);
            let mut players: Vec<_> = by.into_iter().collect();
            players.sort();
            let counts = if players.is_empty() {
                "no HP from you or your party".to_string()
            } else {
                players.iter().map(|(p, n)| format!("{p} {n}")).collect::<Vec<_>>().join(", ")
            };
            let name = npcs.get_npc_name(mobs.get(&target).copied().unwrap_or(0));
            out(format!("deaths in fight {target} {name} {first}..{last}: {counts}"));
        }
    }
}
