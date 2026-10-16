//! The player classes, and how the meter tells which one a player is.

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

/// Every class in number order (the number is the first two digits of the
/// class's skill ids), with the Korean name the game gives it.
const TABLE: [(JobClass, i32, &str); 9] = [
    (JobClass::Gladiator, 11, "검성"),
    (JobClass::Templar, 12, "수호성"),
    (JobClass::Assassin, 13, "살성"),
    (JobClass::Ranger, 14, "궁성"),
    (JobClass::Sorcerer, 15, "마도성"),
    (JobClass::Elementalist, 16, "정령성"),
    (JobClass::Cleric, 17, "치유성"),
    (JobClass::Chanter, 18, "호법성"),
    (JobClass::Fighter, 19, "권성"),
];

/// The roster's class field is a `PcData` id. Each class there has a block
/// of four ids (male and female, light and dark), the first block starting
/// at 5, in this order. Fighter's block (45-48) is not mapped.
const PC_DATA_BLOCKS: [JobClass; 8] = [
    JobClass::Gladiator,
    JobClass::Templar,
    JobClass::Ranger,
    JobClass::Assassin,
    JobClass::Elementalist,
    JobClass::Sorcerer,
    JobClass::Cleric,
    JobClass::Chanter,
];

impl JobClass {
    fn entry(self) -> &'static (JobClass, i32, &'static str) {
        TABLE.iter().find(|e| e.0 == self).expect("every class is in TABLE")
    }

    fn numbered(number: i32) -> Option<JobClass> {
        TABLE.iter().find(|e| e.1 == number).map(|e| e.0)
    }

    /// The class's Korean name, as meter rows and saved fights carry it.
    pub fn class_name(&self) -> &'static str {
        self.entry().2
    }

    /// The class number: the first two digits of the class's skill ids.
    pub fn job_code_head(&self) -> i32 {
        self.entry().1
    }

    /// The class from the class field of a roster or self record.
    pub fn job_from_party_list(value: u32) -> Option<JobClass> {
        let block = value.checked_sub(5)? / 4;
        PC_DATA_BLOCKS.get(usize::try_from(block).ok()?).copied()
    }

    /// The class a skill id belongs to, if it names one.
    pub fn job_of_skill(skill_code: i32) -> Option<JobClass> {
        match skill_code {
            100_510..=103_500 | 109_300..=109_362 => return Some(JobClass::Elementalist),
            10_000_000..=19_999_999 => {}
            _ => return None,
        }
        let (number, group) = (skill_code / 1_000_000, skill_code / 10_000 % 100);
        let named = match (number, group) {
            // Group 0: weapon equip and spirit commands. Only the spirit
            // commands 11 to 13 belong to the Spiritmaster alone.
            (16, 0) => matches!(skill_code / 100 % 100, 11..=13),
            (_, 0) => false,
            // Spiritmaster groups that name the class. The rest, the
            // summons (10-13) and the spirits' attacks (99) among them, do not.
            (16, g) => matches!(g, 1..=8 | 14 | 15 | 17 | 19 | 21..=26 | 30..=32 | 34..=37 | 70..=76 | 80),
            // 10 holds skills every class has; no class has that number.
            _ => true,
        };
        if named { Self::numbered(number) } else { None }
    }

    /// The class most of a player's hits are of, from (skill id, hits)
    /// pairs. Some skills are used by more than one class, so every hit
    /// votes. A tie goes to the lower class number.
    pub fn by_hits(skills: impl IntoIterator<Item = (i32, i32)>) -> Option<JobClass> {
        let mut totals: Vec<(i32, i64)> = Vec::new();
        for (code, hits) in skills {
            let Some(class) = Self::job_of_skill(code) else { continue };
            let number = class.job_code_head();
            let weight = i64::from(hits.max(1));
            match totals.iter_mut().find(|t| t.0 == number) {
                Some(t) => t.1 += weight,
                None => totals.push((number, weight)),
            }
        }
        let winner = totals.into_iter().min_by_key(|&(number, total)| (std::cmp::Reverse(total), number))?;
        Self::numbered(winner.0)
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

    use JobClass::*;

    fn skill(id: i32) -> Option<JobClass> {
        JobClass::job_of_skill(id)
    }

    fn all_none(ids: &[i32]) {
        for &id in ids {
            assert_eq!(skill(id), None, "{id}");
        }
    }

    fn all_spiritmaster(ids: &[i32]) {
        for &id in ids {
            assert_eq!(skill(id), Some(Elementalist), "{id}");
        }
    }

    #[test]
    fn each_class_has_its_korean_name_and_number() {
        let table = [
            (Gladiator, "검성", 11),
            (Templar, "수호성", 12),
            (Assassin, "살성", 13),
            (Ranger, "궁성", 14),
            (Sorcerer, "마도성", 15),
            (Elementalist, "정령성", 16),
            (Cleric, "치유성", 17),
            (Chanter, "호법성", 18),
            (Fighter, "권성", 19),
        ];
        for (class, name, number) in table {
            assert_eq!(class.class_name(), name);
            assert_eq!(class.job_code_head(), number);
        }
    }

    #[test]
    fn classes_print_and_serialize_as_their_names() {
        assert_eq!(format!("{:?}", Elementalist), "Elementalist");
        assert_eq!(serde_json::to_string(&Fighter).unwrap(), r#""Fighter""#);
        assert_eq!(serde_json::from_str::<JobClass>(r#""Templar""#).unwrap(), Templar);
    }

    #[test]
    fn roster_values_map_in_blocks_of_four() {
        let edges = [
            (4, None),
            (5, Some(Gladiator)),
            (8, Some(Gladiator)),
            (9, Some(Templar)),
            (12, Some(Templar)),
            (13, Some(Ranger)),
            (16, Some(Ranger)),
            (17, Some(Assassin)),
            (20, Some(Assassin)),
            (21, Some(Elementalist)),
            (24, Some(Elementalist)),
            (25, Some(Sorcerer)),
            (28, Some(Sorcerer)),
            (29, Some(Cleric)),
            (32, Some(Cleric)),
            (33, Some(Chanter)),
            (36, Some(Chanter)),
            (37, None),
        ];
        for (value, class) in edges {
            assert_eq!(JobClass::job_from_party_list(value), class, "{value}");
        }
    }

    #[test]
    fn roster_values_outside_the_blocks_give_no_class() {
        // 45-48 are Fighter in the game data, but the meter maps no Fighter.
        for value in (0..=4).chain(37..=60).chain([1000, u32::MAX]) {
            assert_eq!(JobClass::job_from_party_list(value), None, "{value}");
        }
    }

    #[test]
    fn six_digit_spiritmaster_ids() {
        all_spiritmaster(&[100510, 100901, 103500, 109300, 109330, 109362]);
        all_none(&[100509, 103501, 109299, 109363]);
    }

    #[test]
    fn group_zero_is_spiritmaster_only_for_spirit_commands() {
        all_spiritmaster(&[16001101, 16001117, 16001301, 16001399]);
        all_none(&[16001000, 16001400, 16000000, 11000100, 15000000, 19000100, 10000001]);
    }

    #[test]
    fn spiritmaster_groups_that_name_the_class() {
        all_spiritmaster(&[
            16010000, 16080000, 16140000, 16150000, 16170000, 16190000, 16210000, 16260000, 16300000,
            16320000, 16340000, 16370000, 16700000, 16760000, 16800001,
        ]);
        all_none(&[
            16090000, 16100000, 16110000, 16120000, 16130000, 16160000, 16180000, 16200000, 16270000,
            16290000, 16330000, 16380000, 16690000, 16770000, 16790000, 16810000, 16990002,
        ]);
    }

    #[test]
    fn every_spiritmaster_group_is_listed_or_not() {
        let spans = [1..=8, 14..=15, 17..=17, 19..=19, 21..=26, 30..=32, 34..=37, 70..=76, 80..=80];
        for group in 1..100 {
            let want = spans.iter().any(|s| s.contains(&group)).then_some(Elementalist);
            assert_eq!(skill(16_000_000 + group * 10_000 + 5), want, "group {group}");
        }
    }

    #[test]
    fn other_class_numbers_name_their_class() {
        let cases = [
            (11340000, Gladiator),
            (12010000, Templar),
            (13070130, Assassin),
            (14020450, Ranger),
            (15210000, Sorcerer),
            (17800000, Cleric),
            (18040000, Chanter),
            (19010000, Fighter),
            (19999999, Fighter),
        ];
        for (id, class) in cases {
            assert_eq!(skill(id), Some(class), "{id}");
        }
    }

    #[test]
    fn shared_and_foreign_ids_name_no_class() {
        all_none(&[10010000, 0, -1, -16010000, i32::MIN, i32::MAX, 1, 100011, 1000100, 3001015]);
        all_none(&[30000171, 9999999, 20000000]);
    }

    #[test]
    fn hits_below_one_count_once_and_big_totals_do_not_wrap() {
        assert_eq!(JobClass::by_hits([]), None);
        assert_eq!(JobClass::by_hits([(11340000, 0)]), Some(Gladiator));
        assert_eq!(JobClass::by_hits([(12010000, -5), (11340000, 1)]), Some(Gladiator));
        assert_eq!(JobClass::by_hits([(15210000, 2), (17800000, 1), (17800000, 1)]), Some(Sorcerer));
        let big = [(12010000, i32::MAX), (12020000, i32::MAX), (11340000, i32::MAX)];
        assert_eq!(JobClass::by_hits(big), Some(Templar));
    }
}
