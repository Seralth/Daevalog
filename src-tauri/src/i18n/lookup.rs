use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::Path;

use parking_lot::RwLock;
use serde_json::Value;

/// Theostone effect codes. A few tables in en name them only under
/// code * 10 + 1.
const THEOSTONES: RangeInclusive<i32> = 3_000_000..=3_099_999;

/// Languages with their own skill and NPC tables; any other loads English.
const SHIPPED: &str = "de en es fr ja ko pt ru";

/// Dummies known before the tables carried `isDummy`, so a table without
/// the flag still marks them.
fn always_dummy(code: i32) -> bool {
    matches!(
        code,
        2300229 | 2300919 | 2310229 | 2310919 | 2320229 | 2320919 | 2400032
            | 2400035 | 2400392 | 2500075 | 2500076 | 2701376 | 2090773 | 2702605
    )
}

/// Keys that are not an i32 drop out.
fn numeric_keys<V>(raw: impl IntoIterator<Item = (String, V)>) -> impl Iterator<Item = (i32, V)> {
    raw.into_iter().filter_map(|(key, v)| key.parse().ok().map(|code| (code, v)))
}

/// Skill code to name, swapped whole on a language change.
pub struct SkillLookup {
    names: RwLock<HashMap<i32, String>>,
}

impl SkillLookup {
    pub fn new() -> Self {
        SkillLookup { names: RwLock::default() }
    }

    /// Replaces the table. Text that is not an object of strings is ignored.
    pub fn load_from_json(&self, json_text: &str) {
        if let Ok(raw) = serde_json::from_str::<HashMap<String, String>>(json_text) {
            *self.names.write() = numeric_keys(raw).collect();
        }
    }

    fn stored(&self, code: i32) -> Option<String> {
        self.names.read().get(&code).cloned()
    }

    /// The stored name only, or "" when there is none.
    pub fn skill_title(&self, code: i32) -> String {
        self.stored(code).unwrap_or_default()
    }

    /// Like `skill_title`, but a Theostone effect may sit under
    /// code * 10 + 1.
    pub fn lookup_skill_name(&self, code: i32) -> String {
        let alias = THEOSTONES.contains(&code).then(|| code * 10 + 1);
        self.stored(code).or_else(|| alias.and_then(|a| self.stored(a))).unwrap_or_default()
    }

    pub fn contains(&self, code: i32) -> bool {
        self.stored(code).is_some()
    }
}

struct NpcRow {
    name: String,
    boss: bool,
    dummy: bool,
    instance: Option<i32>,
}

impl NpcRow {
    /// A bare string is a plain NPC with that name; anything else but an
    /// object is no NPC.
    fn from_value(v: Value) -> Option<NpcRow> {
        let mut fields = match v {
            Value::String(name) => return Some(NpcRow::plain(name)),
            Value::Object(fields) => fields,
            _ => return None,
        };
        let yes = |f: Option<&Value>| f.and_then(Value::as_bool) == Some(true);
        let name = match fields.remove("name") {
            Some(Value::String(name)) => name,
            _ => String::new(),
        };
        let instance = fields.get("dungeonId").and_then(Value::as_i64);
        Some(NpcRow {
            dummy: yes(fields.get("isDummy")) || dummy_name(&name),
            boss: yes(fields.get("isBoss")),
            instance: instance.and_then(|d| i32::try_from(d).ok()).filter(|d| *d > 0),
            name,
        })
    }

    fn plain(name: String) -> NpcRow {
        NpcRow { dummy: dummy_name(&name), boss: false, instance: None, name }
    }
}

/// Hand-made tables carry no flag, so the name tells.
fn dummy_name(name: &str) -> bool {
    ["Training Scarecrow", "Punching Bag"].iter().any(|part| name.contains(part))
}

/// NPC code to name and kind, swapped whole on a language change.
pub struct NpcLookup {
    rows: RwLock<HashMap<i32, NpcRow>>,
}

impl NpcLookup {
    pub fn new() -> Self {
        NpcLookup { rows: RwLock::default() }
    }

    /// Replaces the table. Text that is not a JSON object is ignored;
    /// entries with a bad key or an odd value are skipped.
    pub fn load_from_json(&self, json_text: &str) {
        if let Ok(raw) = serde_json::from_str::<serde_json::Map<String, Value>>(json_text) {
            let rows = numeric_keys(raw).filter_map(|(code, v)| Some((code, NpcRow::from_value(v)?)));
            *self.rows.write() = rows.collect();
        }
    }

    fn with_row<T>(&self, code: i32, pick: impl FnOnce(&NpcRow) -> T) -> Option<T> {
        self.rows.read().get(&code).map(pick)
    }

    pub fn npc_title(&self, code: i32) -> String {
        self.with_row(code, |r| r.name.clone()).unwrap_or_default()
    }

    pub fn is_boss(&self, code: i32) -> bool {
        self.with_row(code, |r| r.boss) == Some(true)
    }

    pub fn npc_is_dummy(&self, code: i32) -> bool {
        always_dummy(code) || self.with_row(code, |r| r.dummy) == Some(true)
    }

    /// The instance the NPC belongs to, when the table gives one.
    pub fn dungeon_of(&self, code: i32) -> Option<i32> {
        self.with_row(code, |r| r.instance).flatten()
    }

    /// A known NPC that no player's skill spawns.
    pub fn is_no_players_summon(&self, code: i32) -> bool {
        let list = &*PLAYER_SUMMON_NPCS;
        !list.is_empty() && !list.contains(&code) && self.with_row(code, |_| ()).is_some()
    }

    pub fn is_players_summon(&self, code: i32) -> bool {
        PLAYER_SUMMON_NPCS.contains(&code)
    }
}

/// The NPCs a player's skill spawns (NpcData `RelationshipEntity` PC_Summon):
/// spirits, a Sorcerer's ground spells and the like.
static PLAYER_SUMMON_NPCS: std::sync::LazyLock<std::collections::HashSet<i32>> = std::sync::LazyLock::new(|| {
    #[derive(serde::Deserialize)]
    struct Table {
        npcs: Vec<i32>,
    }
    serde_json::from_str::<Table>(include_str!("../../../src/data/player_summon_npcs.json"))
        .map(|t| t.npcs.into_iter().collect())
        .unwrap_or_default()
});

/// Fills both tables from `<data_dir>/i18n/{skills,npcs}/<language>.json`.
/// Each table falls back to English on its own; one with neither file
/// keeps what it had.
pub fn load_language(skill_lookup: &SkillLookup, npc_lookup: &NpcLookup, data_dir: &Path, language: &str) {
    let wanted = SHIPPED.split(' ').find(|l| *l == language).unwrap_or("en");
    let tables: [(&str, &dyn Fn(&str)); 2] = [
        ("skills", &|t| skill_lookup.load_from_json(t)),
        ("npcs", &|t| npc_lookup.load_from_json(t)),
    ];
    for (kind, load) in tables {
        let folder = data_dir.join("i18n").join(kind);
        let found = [wanted, "en"]
            .into_iter()
            .find_map(|lang| Some((lang, std::fs::read_to_string(folder.join(lang).with_extension("json")).ok()?)));
        if let Some((lang, text)) = found {
            load(&text);
            tracing::info!("{kind} names loaded: {lang}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO_DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/data");

    fn shipped_npcs(lang: &str) -> NpcLookup {
        let table = NpcLookup::new();
        load_language(&SkillLookup::new(), &table, Path::new(REPO_DATA), lang);
        assert!(table.rows.read().len() > 1000, "{lang}");
        table
    }

    /// Each pair is (code, name wanted from lookup_skill_name).
    fn skill_names_are(table: &SkillLookup, want: &[(i32, &str)]) {
        for &(code, name) in want {
            assert_eq!((code, table.lookup_skill_name(code).as_str()), (code, name));
        }
    }

    #[test]
    fn empty_tables_know_nothing() {
        let (skill, npc) = (SkillLookup::new(), NpcLookup::new());
        skill_names_are(&skill, &[(1, ""), (3_000_000, "")]);
        assert!(!skill.contains(1) && skill.skill_title(1).is_empty());
        assert!(npc.npc_title(2_000_000).is_empty());
        assert!(!npc.is_boss(2_000_000) && npc.dungeon_of(2_000_000).is_none());
    }

    #[test]
    fn skill_text_swaps_or_is_ignored() {
        let table = SkillLookup::new();
        table.load_from_json(r#"{"1":"A","2":"B"}"#);
        table.load_from_json(r#"{"2":"C"}"#);
        skill_names_are(&table, &[(1, ""), (2, "C")]);

        table.load_from_json(r#"{"x":"A","7":"B"}"#);
        skill_names_are(&table, &[(7, "B"), (2, "")]);
        assert_eq!(table.names.read().len(), 1);

        for junk in ["not json", r#"{"7":"A","2":5}"#, "[]"] {
            table.load_from_json(junk);
            skill_names_are(&table, &[(7, "B"), (2, "")]);
        }
    }

    #[test]
    fn theostone_alias_only_inside_the_range() {
        let table = SkillLookup::new();
        table.load_from_json(
            r#"{"30010081":"T","29999991":"low","31000001":"high","3000005":"D","30000051":"X",
                "30000001":"first","30999991":"last"}"#,
        );
        skill_names_are(&table, &[(3_001_008, "T"), (3_000_005, "D")]);
        skill_names_are(&table, &[(2_999_999, ""), (3_100_000, "")]);
        skill_names_are(&table, &[(3_000_000, "first"), (3_099_999, "last")]);
        assert_eq!(table.skill_title(3_001_008), "");
        assert!(table.contains(30_010_081) && !table.contains(3_001_008));
    }

    #[test]
    fn npc_rows_from_odd_values() {
        let table = NpcLookup::new();
        table.load_from_json(
            r#"{"5":{"isBoss":"yes"},"6":{"name":3,"isDummy":1,"dungeonId":"9"},"7":"Name",
                "8":3,"9":[1],"10":null,"11":true,"x":"Bad","12":{"name":"Q","dungeonId":1.5}}"#,
        );
        let known: Vec<i32> = (5..=12).filter(|c| table.with_row(*c, |_| ()).is_some()).collect();
        assert_eq!(known, [5, 6, 7, 12]);
        assert_eq!(table.rows.read().len(), 4);
        assert_eq!([5, 6, 7].map(|c| table.npc_title(c)), ["", "", "Name"]);
        assert!((5..=12).all(|c| !table.is_boss(c) && table.dungeon_of(c).is_none()));
        assert!(!table.npc_is_dummy(6));
    }

    #[test]
    fn npc_text_swaps_or_is_ignored() {
        let table = NpcLookup::new();
        table.load_from_json(r#"{"1":{"name":"A","isBoss":true},"2":"B"}"#);
        table.load_from_json(r#"{"2":"C"}"#);
        for junk in ["[1,2]", "{oops", "\"text\""] {
            table.load_from_json(junk);
        }
        assert_eq!([1, 2].map(|c| table.npc_title(c)), ["", "C"]);
        assert!(!table.is_boss(1));
    }

    #[test]
    fn dummies_by_flag_name_or_old_code() {
        let table = NpcLookup::new();
        assert!(table.npc_is_dummy(2_400_035) && !table.npc_is_dummy(1));
        table.load_from_json(
            r#"{"1":{"name":"Punching Bag","isBoss":false},"2":{"name":"Odd Target","isBoss":true,"isDummy":true},
                "3":{"name":"Ancient Dragon","isBoss":true},"4":"Big Punching Bag","5":{"name":"punching bag"},
                "6":"A Training Scarecrow","2702605":{"name":"Plain","isDummy":false}}"#,
        );
        let flags = [1, 2, 3, 4, 5, 6, 2_702_605].map(|c| table.npc_is_dummy(c));
        assert_eq!(flags, [true, true, false, true, false, true, true]);
        let old = "2300229 2300919 2310229 2310919 2320229 2320919 2400032 2400035 2400392 2500075 2500076 2701376 2090773 2702605";
        let blank = NpcLookup::new();
        assert!(old.split(' ').all(|c| blank.npc_is_dummy(c.parse().unwrap())));
    }

    #[test]
    fn instance_only_when_above_zero() {
        let table = NpcLookup::new();
        table.load_from_json(
            r#"{"2310218":{"name":"Divine Auldor","isBoss":true,"dungeonId":600011},
                "2300475":{"name":"Gargaum","isBoss":true},"9":{"dungeonId":0},"10":{"dungeonId":-4}}"#,
        );
        let got = [2_310_218, 2_300_475, 9, 10, 77].map(|c| table.dungeon_of(c));
        assert_eq!(got, [Some(600_011), None, None, None, None]);
    }

    #[test]
    fn every_language_marks_the_scarecrows() {
        for lang in SHIPPED.split(' ') {
            let table = shipped_npcs(lang);
            let scarecrows = [2_090_773, 2_400_035, 2_702_605, 2_300_229, 2_090_458];
            assert!(scarecrows.iter().all(|c| table.npc_is_dummy(*c)), "{lang}");
            assert_eq!((table.is_boss(2_301_208), table.npc_is_dummy(2_301_208)), (true, false), "{lang}");
        }
    }

    #[test]
    fn summon_kind_from_list_and_table() {
        assert!(!NpcLookup::new().is_no_players_summon(2_920_063));
        let table = shipped_npcs("en");
        let codes = [2_920_063, 2_920_342, 2_920_091, 2_920_149, 2_920_112, 2_920_011, 1];
        let monster = codes.map(|c| table.is_no_players_summon(c));
        let players = codes.map(|c| table.is_players_summon(c));
        assert_eq!(monster, [true, true, true, false, false, false, false]);
        assert_eq!(players, [false, false, false, true, true, true, false]);
    }

    #[test]
    fn unknown_language_loads_english() {
        let data = Path::new(REPO_DATA);
        let english = SkillLookup::new();
        load_language(&english, &NpcLookup::new(), &data, "en");
        for lang in ["xx", ""] {
            let (skill, npc) = (SkillLookup::new(), NpcLookup::new());
            load_language(&skill, &npc, &data, lang);
            assert!(*skill.names.read() == *english.names.read(), "{lang:?}");
            assert_eq!(skill.skill_title(1_900_001), "HP Regen");
            assert!(npc.rows.read().len() > 1000, "{lang:?}");
        }
    }

    #[test]
    fn each_table_falls_back_to_english_alone() {
        let root = std::env::temp_dir().join("daevalog-name-tables").join(std::process::id().to_string());
        let put = |rel: &str, text: &str| {
            let file = root.join("i18n").join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        };
        put("skills/en.json", r#"{"1":"english skill"}"#);
        put("npcs/de.json", r#"{"2":"deutscher Gegner"}"#);
        let (skill, npc) = (SkillLookup::new(), NpcLookup::new());
        let state = || (skill.skill_title(1), npc.npc_title(2));
        let want = || ("english skill".to_string(), "deutscher Gegner".to_string());

        load_language(&skill, &npc, &root, "de");
        assert_eq!(state(), want());
        // fr: skills from en again, npcs have neither file and stay.
        load_language(&skill, &npc, &root, "fr");
        assert_eq!(state(), want());
        // A readable file with bad JSON does not fall back.
        put("skills/en.json", r#"{"1":"from en"}"#);
        put("skills/de.json", "broken");
        load_language(&skill, &npc, &root, "de");
        assert_eq!(state(), want());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
