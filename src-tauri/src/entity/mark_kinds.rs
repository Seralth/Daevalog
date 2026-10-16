use serde::{Deserialize, Serialize};

/// A mark on a hit: where it landed from, how it hit, how it was taken.
/// The parser reads the angle (back, frontal), the damage type (critical)
/// and the flags byte of a damage record into these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HitMark {
    Back,
    Critical,
    ShieldBlock,
    Parry,
    Perfect,
    Double,
    Frontal,
    IronWall,
    Regeneration,
    PerfectBlock,
}

/// The flags byte of a damage record, as `HitMark` values.
pub fn from_hit_flags(flags: u8) -> Vec<HitMark> {
    [
        (0x01, HitMark::ShieldBlock),
        (0x02, HitMark::Parry),
        (0x04, HitMark::Perfect),
        (0x08, HitMark::Double),
        (0x10, HitMark::IronWall),
        (0x20, HitMark::Regeneration),
        (0x40, HitMark::PerfectBlock),
    ]
    .into_iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, s)| s)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::HitMark::{self, *};

    #[test]
    fn marks_are_written_as_their_names() {
        let marks = [Back, Critical, ShieldBlock, Parry, Perfect, Double, Frontal, IronWall, Regeneration, PerfectBlock];
        let text = serde_json::to_string(&marks).unwrap();
        assert_eq!(
            text,
            r#"["Back","Critical","ShieldBlock","Parry","Perfect","Double","Frontal","IronWall","Regeneration","PerfectBlock"]"#
        );
        let back: Vec<HitMark> = serde_json::from_str(&text).unwrap();
        assert_eq!(back, marks);
    }
}
