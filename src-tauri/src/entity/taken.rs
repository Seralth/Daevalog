//! Damage taken: what the parser reads of a hit on a player, and the numbers
//! the views and saved fights carry.

use serde::{Deserialize, Serialize};

/// Which record a hit taken came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakenKind {
    /// A monster skill's hit with a value (`04 38`).
    Hit,
    /// A damage over time tick of a monster's effect (`05 38` effect 0x02 or
    /// 0x0A).
    Tick,
    /// Damage reflected onto the player: a `04 38` record with hit type 9
    /// and no skill, whose damage arrives as a `05 38` effect 0x4a tick of
    /// the reflecting skill. Read from the tick.
    Reflect,
    /// The player was immune: a `05 38` effect 0x30 record with code 2001.
    Immune,
    /// Hit type 1: the attack missed.
    Miss,
    /// Hit type 6: the skill's effect was resisted. Its hit is a record of
    /// its own.
    Resist,
}

/// One hit taken, as the parser read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakenHit {
    pub at: i64,
    /// Who took it.
    pub target: i32,
    /// The attacker as the record names it: a monster, or a short-lived
    /// skill-effect entity of one.
    pub actor: i32,
    /// The skill or effect that dealt it: the mechanic.
    pub skill: i32,
    pub damage: i64,
    pub kind: TakenKind,
    /// The flags byte (`special_damage::from_hit_flags`); 0 when the record
    /// has none.
    pub flags: u8,
    /// 0x01 back, 0x02 front, 0 neither.
    pub angle: u8,
    pub crit: bool,
    /// HP a Regeneration hit gave back.
    pub restored: i64,
}

impl TakenHit {
    /// A record with no flags, angle or crit: a tick, an immune, a miss.
    pub fn plain(at: i64, target: i32, actor: i32, skill: i32, damage: i64, kind: TakenKind) -> Self {
        Self { at, target, actor, skill, damage, kind, flags: 0, angle: 0, crit: false, restored: 0 }
    }
}

/// Damage one player took and how the hits landed: from one skill, or over all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TakenStats {
    /// Direct hits, damage over time and reflected damage.
    pub damage: i64,
    /// Direct hits with a value: the game's AccuracyCount.
    pub hits: i32,
    /// Damage over time ticks. The game counts none as a hit.
    pub ticks: i32,
    /// Reflected damage, one per tick.
    pub reflects: i32,
    pub immune: i32,
    pub miss: i32,
    pub resist: i32,
    pub crit: i32,
    pub perfect: i32,
    pub double: i32,
    pub front: i32,
    pub back: i32,
    pub shield_block: i32,
    pub parry: i32,
    pub perfect_block: i32,
    pub iron_wall: i32,
    pub regeneration: i32,
    /// HP that Regeneration hits gave back.
    pub restored: i64,
    /// The biggest direct hit.
    pub max_hit: i64,
}

impl TakenStats {
    pub fn add(&mut self, hit: &TakenHit) {
        let up = |n: &mut i32| *n = n.saturating_add(1);
        match hit.kind {
            TakenKind::Hit => {
                up(&mut self.hits);
                self.max_hit = self.max_hit.max(hit.damage);
                if hit.crit {
                    up(&mut self.crit);
                }
                match hit.angle {
                    0x01 => up(&mut self.back),
                    0x02 => up(&mut self.front),
                    _ => {}
                }
                for (bit, count) in [
                    (0x01, &mut self.shield_block),
                    (0x02, &mut self.parry),
                    (0x04, &mut self.perfect),
                    (0x08, &mut self.double),
                    (0x10, &mut self.iron_wall),
                    (0x20, &mut self.regeneration),
                    (0x40, &mut self.perfect_block),
                ] {
                    if hit.flags & bit != 0 {
                        up(count);
                    }
                }
                self.restored = self.restored.saturating_add(hit.restored);
            }
            TakenKind::Tick => up(&mut self.ticks),
            TakenKind::Reflect => up(&mut self.reflects),
            TakenKind::Immune => up(&mut self.immune),
            TakenKind::Miss => up(&mut self.miss),
            TakenKind::Resist => up(&mut self.resist),
        }
        self.damage = self.damage.saturating_add(hit.damage);
    }

    pub fn absorb(&mut self, o: &TakenStats) {
        self.damage = self.damage.saturating_add(o.damage);
        for (mine, theirs) in [
            (&mut self.hits, o.hits),
            (&mut self.ticks, o.ticks),
            (&mut self.reflects, o.reflects),
            (&mut self.immune, o.immune),
            (&mut self.miss, o.miss),
            (&mut self.resist, o.resist),
            (&mut self.crit, o.crit),
            (&mut self.perfect, o.perfect),
            (&mut self.double, o.double),
            (&mut self.front, o.front),
            (&mut self.back, o.back),
            (&mut self.shield_block, o.shield_block),
            (&mut self.parry, o.parry),
            (&mut self.perfect_block, o.perfect_block),
            (&mut self.iron_wall, o.iron_wall),
            (&mut self.regeneration, o.regeneration),
        ] {
            *mine = mine.saturating_add(theirs);
        }
        self.restored = self.restored.saturating_add(o.restored);
        self.max_hit = self.max_hit.max(o.max_hit);
    }

    /// Attacks on the player: the game's TotalCount. Checked on three boss
    /// records (2026-10-07): it counts hits with a value, reflects and
    /// immunes, and no tick and no resist. Misses are counted too, unchecked:
    /// no record has had one yet.
    pub fn total(&self) -> i32 {
        self.hits.saturating_add(self.reflects).saturating_add(self.immune).saturating_add(self.miss)
    }
}

/// What one player took from one skill in a fight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakenSkillEntry {
    /// Who took it.
    pub actor_id: i32,
    /// The skill or effect that dealt it.
    pub code: i32,
    pub name: String,
    /// The NPC that used it, by its code; 0 when the attacker was never seen
    /// spawning.
    #[serde(default)]
    pub source_code: i32,
    #[serde(flatten)]
    pub stats: TakenStats,
}

/// One player's damage taken on the meter, beside the damage rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakenRow {
    /// The row id, as in `DpsData::map` when the player has a damage row.
    pub actor_id: i32,
    pub nickname: String,
    #[serde(default)]
    pub job: String,
    /// See `PersonalData::number`; 0 for a player with no damage row, who
    /// has none yet.
    #[serde(default)]
    pub number: u32,
    #[serde(flatten)]
    pub stats: TakenStats,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_counts_its_flags_and_angle() {
        let mut s = TakenStats::default();
        let mut hit = TakenHit::plain(0, 1, 2, 1_800_510, 1246, TakenKind::Hit);
        hit.angle = 0x02;
        hit.flags = 0x20 | 0x02;
        hit.restored = 177;
        hit.crit = true;
        s.add(&hit);
        s.add(&TakenHit::plain(0, 1, 2, 1_200_010, 305, TakenKind::Tick));
        s.add(&TakenHit::plain(0, 1, 2, 1_605_910, 500, TakenKind::Reflect));
        s.add(&TakenHit::plain(0, 1, 2, 1_800_559, 0, TakenKind::Immune));
        assert_eq!((s.damage, s.hits, s.ticks, s.reflects, s.immune), (2051, 1, 1, 1, 1));
        assert_eq!((s.front, s.back, s.crit, s.parry, s.regeneration, s.restored), (1, 0, 1, 1, 1, 177));
        assert_eq!(s.total(), 3);
        let mut twice = s;
        twice.absorb(&s);
        assert_eq!((twice.damage, twice.total(), twice.max_hit), (4102, 6, 1246));
    }

    #[test]
    fn a_skill_entry_is_flat_camel_case() {
        let e = TakenSkillEntry {
            actor_id: 9,
            code: 1_800_510,
            name: "x".into(),
            source_code: 0,
            stats: TakenStats { damage: 5, shield_block: 1, ..TakenStats::default() },
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains(r#""damage":5"#) && json.contains(r#""shieldBlock":1"#), "{json}");
        assert_eq!(serde_json::from_str::<TakenSkillEntry>(&json).unwrap(), e);
        let old: TakenSkillEntry = serde_json::from_str(r#"{"actorId":1,"code":2,"name":"y"}"#).unwrap();
        assert_eq!(old.stats, TakenStats::default());
    }
}
