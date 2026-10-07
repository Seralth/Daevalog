//! The damage-taken part of the replay report (`A2_REPLAY_TAKEN=1`): every hit
//! a player took in the window, per player and per skill, the mechanic.

use std::collections::BTreeMap;

use crate::combat::data_storage::{add_tick, DataStorage, TakenBy, TakenTick};
use crate::entity::taken::TakenStats;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// The hits taken, read from storage after each line of the capture, so none
/// is lost to pruning.
#[derive(Default)]
pub(crate) struct Gather {
    seen: u64,
    pub ticks: Vec<TakenTick>,
}

impl Gather {
    pub fn after_line(&mut self, storage: &DataStorage) {
        for t in storage.taken_since(self.seen) {
            self.seen = t.seq;
            self.ticks.push(t);
        }
    }

    /// The hits from `from` to `until` (ms since the epoch), per player and skill.
    pub fn between(&self, from: i64, until: i64) -> (TakenBy, Vec<&TakenTick>) {
        let ticks: Vec<&TakenTick> = self.ticks.iter().filter(|t| (from..=until).contains(&t.hit.at)).collect();
        let mut by = TakenBy::new();
        for t in &ticks {
            add_tick(&mut by, t);
        }
        (by, ticks)
    }

    pub fn report(
        &self,
        window: (i64, i64),
        local: Option<i32>,
        skills: &SkillLookup,
        npcs: &NpcLookup,
        out: &mut dyn FnMut(String),
    ) {
        let (by, ticks) = self.between(window.0, window.1);
        let total: i64 = ticks.iter().map(|t| t.hit.damage).sum();
        let effects = ticks.iter().filter(|t| t.source != t.hit.actor).count();
        let unseen = ticks.iter().filter(|t| t.source_code == 0).count();
        out(format!(
            "\n== damage taken in the window: {total} by {} players, {} records ({effects} from a monster's skill-effect entity, {unseen} from an attacker never seen spawning)",
            by.len(),
            ticks.len()
        ));
        let players: BTreeMap<i32, TakenStats> = by
            .iter()
            .map(|(&p, s)| {
                let mut sum = TakenStats::default();
                s.values().for_each(|d| sum.absorb(&d.stats));
                (p, sum)
            })
            .collect();
        for (p, s) in &players {
            let you = if Some(*p) == local { " (you)" } else { "" };
            out(format!("taken by {p}{you}: {}", line(s)));
        }
        for (p, s) in &by {
            let mut rows: Vec<_> = s.iter().collect();
            rows.sort_by_key(|(code, d)| (-d.stats.damage, **code));
            for (code, d) in rows {
                let from = if d.source_code == 0 { "?".to_string() } else { npcs.get_npc_name(d.source_code) };
                out(format!("taken {p} {code} {} from {from}: {}", skills.get_skill_name(*code), line(&d.stats)));
            }
        }
    }
}

/// The numbers of `s` that are not zero.
pub(crate) fn line(s: &TakenStats) -> String {
    let mut parts = vec![format!("{} damage", s.damage), format!("{} attacks", s.total())];
    for (n, what) in [
        (s.hits, "hits"),
        (s.reflects, "reflects"),
        (s.immune, "immune"),
        (s.miss, "misses"),
        (s.ticks, "ticks"),
        (s.resist, "resists"),
        (s.crit, "crit"),
        (s.perfect, "perfect"),
        (s.double, "double"),
        (s.front, "front"),
        (s.back, "back"),
        (s.shield_block, "shield block"),
        (s.parry, "parry"),
        (s.perfect_block, "perfect block"),
        (s.iron_wall, "endurance"),
        (s.regeneration, "regeneration"),
    ] {
        if n != 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    if s.restored != 0 {
        parts.push(format!("{} HP restored", s.restored));
    }
    if s.max_hit != 0 {
        parts.push(format!("max hit {}", s.max_hit));
    }
    parts.join(", ")
}
