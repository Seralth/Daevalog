use serde::{Deserialize, Serialize};

use super::damage_packet::ParsedDamagePacket;
use super::special_damage::SpecialDamage;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzedSkill {
    #[serde(skip)]
    pub skill_code: i32,
    pub damage_amount: i64,
    pub dot_damage_amount: i64,
    pub dot_times: i32,
    pub crit_times: i32,
    pub times: i32,
    pub skill_name: String,
    pub back_times: i32,
    pub frontal_times: i32,
    pub perfect_times: i32,
    pub double_times: i32,
    pub parry_times: i32,
    pub heal_amount: i64,
}

impl AnalyzedSkill {
    pub fn new(skill_code: i32, skill_name: String) -> Self {
        Self {
            skill_code,
            damage_amount: 0,
            dot_damage_amount: 0,
            dot_times: 0,
            crit_times: 0,
            times: 0,
            skill_name,
            back_times: 0,
            frontal_times: 0,
            perfect_times: 0,
            double_times: 0,
            parry_times: 0,
            heal_amount: 0,
        }
    }

    pub fn process_pdp(&mut self, pdp: &ParsedDamagePacket) {
        // Counts saturate rather than overflow on a very long fight.
        if pdp.heal_amount() > 0 {
            self.heal_amount = self.heal_amount.saturating_add(pdp.heal_amount());
        }
        if pdp.is_dot() {
            self.dot_times = self.dot_times.saturating_add(1);
            self.dot_damage_amount = self.dot_damage_amount.saturating_add(pdp.total_damage());
        } else {
            self.times = self.times.saturating_add(1);
            self.damage_amount = self.damage_amount.saturating_add(pdp.total_damage());
            if pdp.is_crit() { self.crit_times = self.crit_times.saturating_add(1); }
            if pdp.specials().contains(&SpecialDamage::Back) { self.back_times = self.back_times.saturating_add(1); }
            if pdp.specials().contains(&SpecialDamage::Frontal) { self.frontal_times = self.frontal_times.saturating_add(1); }
            if pdp.specials().contains(&SpecialDamage::Parry) { self.parry_times = self.parry_times.saturating_add(1); }
            if pdp.specials().contains(&SpecialDamage::Double) { self.double_times = self.double_times.saturating_add(1); }
            if pdp.specials().contains(&SpecialDamage::Perfect) { self.perfect_times = self.perfect_times.saturating_add(1); }
        }
    }

    pub fn merge_from(&mut self, other: &AnalyzedSkill) {
        self.times = self.times.saturating_add(other.times);
        self.damage_amount = self.damage_amount.saturating_add(other.damage_amount);
        self.crit_times = self.crit_times.saturating_add(other.crit_times);
        self.back_times = self.back_times.saturating_add(other.back_times);
        self.frontal_times = self.frontal_times.saturating_add(other.frontal_times);
        self.parry_times = self.parry_times.saturating_add(other.parry_times);
        self.double_times = self.double_times.saturating_add(other.double_times);
        self.perfect_times = self.perfect_times.saturating_add(other.perfect_times);
        self.dot_times = self.dot_times.saturating_add(other.dot_times);
        self.dot_damage_amount = self.dot_damage_amount.saturating_add(other.dot_damage_amount);
        self.heal_amount = self.heal_amount.saturating_add(other.heal_amount);
    }
}
