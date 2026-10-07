use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobClass {
    Gladiator,
    Templar,
    Ranger,
    Assassin,
    Sorcerer,
    Cleric,
    Elementalist,
    Chanter,
    Fighter,
}

impl JobClass {
    pub fn class_name(&self) -> &'static str {
        match self {
            JobClass::Gladiator => "검성",
            JobClass::Templar => "수호성",
            JobClass::Ranger => "궁성",
            JobClass::Assassin => "살성",
            JobClass::Sorcerer => "마도성",
            JobClass::Cleric => "치유성",
            JobClass::Elementalist => "정령성",
            JobClass::Chanter => "호법성",
            JobClass::Fighter => "권성",
        }
    }

    pub fn class_prefix(&self) -> i32 {
        match self {
            JobClass::Gladiator => 11,
            JobClass::Templar => 12,
            JobClass::Assassin => 13,
            JobClass::Ranger => 14,
            JobClass::Sorcerer => 15,
            JobClass::Elementalist => 16,
            JobClass::Cleric => 17,
            JobClass::Chanter => 18,
            JobClass::Fighter => 19,
        }
    }

    /// The class field of a party roster record (`0x9702`), the u32 right
    /// after the member's name. Each class has a block of four values, which
    /// differ by something else about the character: two Gladiators in one
    /// party read 7 and 8. Matched against the classes the meter detected in
    /// 14 players over 12 captures: Gladiator 5–8, Templar 9/12, Ranger 14,
    /// Assassin 17, Elementalist 22/24, Sorcerer 28, Cleric 32, Chanter 36.
    pub fn from_roster_class(value: u32) -> Option<JobClass> {
        match value {
            5..=8 => Some(JobClass::Gladiator),
            9..=12 => Some(JobClass::Templar),
            13..=16 => Some(JobClass::Ranger),
            17..=20 => Some(JobClass::Assassin),
            21..=24 => Some(JobClass::Elementalist),
            25..=28 => Some(JobClass::Sorcerer),
            29..=32 => Some(JobClass::Cleric),
            33..=36 => Some(JobClass::Chanter),
            _ => None,
        }
    }

    fn from_prefix(prefix: i32) -> Option<JobClass> {
        match prefix {
            11 => Some(JobClass::Gladiator),
            12 => Some(JobClass::Templar),
            13 => Some(JobClass::Assassin),
            14 => Some(JobClass::Ranger),
            15 => Some(JobClass::Sorcerer),
            16 => Some(JobClass::Elementalist),
            17 => Some(JobClass::Cleric),
            18 => Some(JobClass::Chanter),
            19 => Some(JobClass::Fighter),
            _ => None,
        }
    }

    /// Strict job detection from skill code.
    pub fn convert_from_skill(skill_code: i32) -> Option<JobClass> {
        // PC Elementalist specific 6-digit skills
        if (100510..=103500).contains(&skill_code)
            || (109300..=109362).contains(&skill_code)
        {
            return Some(JobClass::Elementalist);
        }

        // 8-digit standard player skills
        if (10_000_000..=19_999_999).contains(&skill_code) {
            let prefix = skill_code / 1_000_000;
            let sub = (skill_code / 10000) % 100;

            // Exclude generic mob skills (sub 00) for ALL classes
            if sub == 0 {
                if prefix == 16 {
                    let command_range = (skill_code / 100) % 100;
                    if (11..=13).contains(&command_range) {
                        return Some(JobClass::Elementalist);
                    }
                }
                return None;
            }

            // Elementalist strict whitelist
            if prefix == 16 {
                let is_pc_range = matches!(sub,
                    1..=8 | 14 | 15 | 17 | 19 | 21..=26 |
                    30 | 31 | 32 | 34 | 35 | 36 | 37 |
                    70..=76 | 80
                );
                return if is_pc_range { Some(JobClass::Elementalist) } else { None };
            }

            return Self::from_prefix(prefix);
        }

        None
    }

    /// A player's class from the skills they used, as (skill code, hits): the
    /// class most of the hits are of, the lower class number on a tie. A few
    /// skills are used by more than one class: in the Kasia capture of
    /// 2026-10-06 a Templar, a Cleric and a Sorcerer hit with Lifestealing
    /// Blade (11340000, a Gladiator code). The class of whichever skill a hash
    /// map gave first named one player Gladiator in one read and Templar in
    /// the next.
    pub fn by_hits(skills: impl IntoIterator<Item = (i32, i32)>) -> Option<JobClass> {
        let mut hits: Vec<(JobClass, i64)> = Vec::new();
        for (code, n) in skills {
            let Some(job) = Self::convert_from_skill(code) else { continue };
            match hits.iter_mut().find(|(j, _)| *j == job) {
                Some((_, total)) => *total += i64::from(n.max(1)),
                None => hits.push((job, i64::from(n.max(1)))),
            }
        }
        hits.into_iter()
            .max_by_key(|&(job, n)| (n, std::cmp::Reverse(job.class_prefix())))
            .map(|(job, _)| job)
    }
}

#[cfg(test)]
mod tests {
    use super::JobClass;

    #[test]
    fn a_player_is_the_class_most_hits_are_of_in_any_order() {
        // A Templar's rotation and one Gladiator-coded skill used 52 times.
        let skills = [(12_020_000, 300), (12_010_000, 120), (11_340_000, 52), (12_060_000, 40)];
        for turn in 0..skills.len() {
            let mut order = skills.to_vec();
            order.rotate_left(turn);
            assert_eq!(JobClass::by_hits(order.iter().copied()), Some(JobClass::Templar));
            order.reverse();
            assert_eq!(JobClass::by_hits(order), Some(JobClass::Templar));
        }
        // A tie goes to the lower class number, whatever the order.
        assert_eq!(JobClass::by_hits([(12_010_000, 5), (11_340_000, 5)]), Some(JobClass::Gladiator));
        assert_eq!(JobClass::by_hits([(11_340_000, 5), (12_010_000, 5)]), Some(JobClass::Gladiator));
        // Mob skills name no class.
        assert_eq!(JobClass::by_hits([(1_000_100, 9)]), None);
    }
}
