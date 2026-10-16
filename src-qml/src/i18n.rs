// SPDX-License-Identifier: AGPL-3.0-only

//! The UI strings: the 1.x meter's translation files (src/data/i18n/ui), all
//! eight languages built into the program, looked up by key ("tray.quit")
//! with English where a language lacks one. `Strings` is the QML side;
//! I18n.qml wraps it so that bindings follow a change of language.

use std::collections::HashMap;
use std::sync::OnceLock;

use cxx_qt_lib::QString;
use daevalog::history::history_view::Clock;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("daevalog-qml/cpp/shell.h");

        /// The system's languages, most wanted first ("de-DE", "de").
        #[cxx_name = "systemLanguages"]
        fn system_languages() -> Vec<String>;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Strings = super::StringsRust;

        /// The language to show: `choice` (the Language setting) when it is
        /// one of ours, else the system's, else English.
        #[qinvokable]
        fn resolve(self: &Strings, choice: &QString) -> QString;

        /// The string for `key` in `language`, or the English one.
        #[qinvokable]
        fn text(self: &Strings, language: &QString, key: &QString) -> QString;

        /// The header's target; see `target_name`.
        #[qinvokable]
        fn target_name(self: &Strings, language: &QString, mode: &QString, target: &QString) -> QString;

        /// A time of day as Display Time writes it; see `clock`.
        #[qinvokable]
        fn clock(self: &Strings, language: &QString, hour: i32, minute: i32, hour24: bool) -> QString;

        /// How a meter row names its player; see `player_name`.
        #[qinvokable]
        fn player_name(
            self: &Strings,
            language: &QString,
            name: &QString,
            job: &QString,
            number: i32,
            you: bool,
            identifying: bool,
            unattributed: bool,
            hide_others: bool,
            show_you: bool,
        ) -> QString;
    }
}

/// The UI languages: the eight of the global client.
pub const LANGUAGES: [&str; 8] = ["en", "de", "es", "fr", "ja", "ko", "pt", "ru"];

const FILES: [(&str, &str); 8] = [
    ("en", include_str!("../../src/data/i18n/ui/en.json")),
    ("de", include_str!("../../src/data/i18n/ui/de.json")),
    ("es", include_str!("../../src/data/i18n/ui/es.json")),
    ("fr", include_str!("../../src/data/i18n/ui/fr.json")),
    ("ja", include_str!("../../src/data/i18n/ui/ja.json")),
    ("ko", include_str!("../../src/data/i18n/ui/ko.json")),
    ("pt", include_str!("../../src/data/i18n/ui/pt.json")),
    ("ru", include_str!("../../src/data/i18n/ui/ru.json")),
];

/// Each class's key in the meter's rows, and its name's key in the files.
const CLASS_KEYS: [(&str, &str); 9] = [
    ("gladiator", "classes.GLADIATOR"),
    ("templar", "classes.TEMPLAR"),
    ("ranger", "classes.RANGER"),
    ("assassin", "classes.ASSASSIN"),
    ("sorcerer", "classes.SORCERER"),
    ("spiritmaster", "classes.ELEMENTALIST"),
    ("cleric", "classes.CLERIC"),
    ("chanter", "classes.CHANTER"),
    ("fighter", "classes.FIGHTER"),
];

/// Where the mode and tab names come from: the game's own text, by its key
/// in the game's localised strings, in the global client's languages other
/// than English. Each word is the
/// game's, or the part of the game's text that names the thing ("Training
/// Scarecrow" gives TRAINING); `other` lists languages that use another
/// key.
/// - ENC: the game has no word for an encounter; its word for a fight
///   ("Combat") stands in, also where the header and Settings speak of an
///   encounter (`TERM_USES`). English keeps ENC and "encounter".
/// - Heal: ko and ja take the word of the game's meter (Combat Analysis,
///   "Total Heal"). In the other five that word means regeneration or
///   recovery, and in de, es and pt it is also the Regeneration hit type's
///   (String_StatName_Restoration_body), so they take the heal stat's word.
/// - DPS: the game writes DPS in every language.
/// - Tank: the party finder's role name; ja and ko write it the way the
///   game does (タンカー, 탱커).
/// - TRAIN in Spanish: ENTRENO, chosen because the game's
///   ENTRENAMIENTO did not fit the footer; the game never writes ENTRENO.
#[cfg(test)]
struct GameTerm {
    key: &'static str,
    source: &'static str,
    other: &'static [(&'static str, &'static str)],
    /// Languages where the word is chosen, not the game's: (language, word).
    chosen: &'static [(&'static str, &'static str)],
}

#[cfg(test)]
const GAME_TERMS: [GameTerm; 8] = [
    GameTerm { key: "meter.modes.bossTargets", source: "String_UI_BOSSCHALLENGE_POPUP_RECORD_HEADER_BOSS_body", other: &[], chosen: &[] },
    GameTerm { key: "meter.modes.lastHitByMe", source: "String_UI_DAMAGEANALYZER_RECORD_TARGET_body", other: &[], chosen: &[] },
    GameTerm { key: "meter.modes.allTargets", source: "String_UI_DAMAGEANALYZER_RECORD_TARGET_ALL_body", other: &[], chosen: &[] },
    GameTerm {
        key: "meter.modes.trainTargets",
        source: "String_STR_M_TraDummy_body",
        other: &[("ru", "String_UI_JOURNAL_MENU_QUEST_TAB_TUTORIAL_body")],
        chosen: &[("es", "ENTRENO")],
    },
    GameTerm { key: "meter.modes.encounter", source: "String_UI_SETTING_TAB_BATTLE_body", other: &[], chosen: &[] },
    GameTerm { key: "meter.tabs.dps", source: "String_UI_DAMAGEANALYZER_DPS_body", other: &[], chosen: &[] },
    GameTerm {
        key: "meter.tabs.heal",
        source: "String_StatName_AmplifyHpHealGet_body",
        other: &[("ja", "String_UI_DAMAGEANALYZER_TOTAL_HEAL_body"), ("ko", "String_UI_DAMAGEANALYZER_TOTAL_HEAL_body")],
        chosen: &[],
    },
    GameTerm { key: "meter.tabs.tank", source: "String_UI_PARTYDUNGEON_WAITING_LIST_TOOLIP_TANKER_body", other: &[], chosen: &[] },
];

/// Other strings that use a mode's word, so the mode reads the same in the
/// meter, its header and Settings: each holds the word of the mode named
/// here, and so the game's (`GAME_TERMS`), in every language but English.
/// "ENC mode" in Settings names the mode by its label ("KAMPF-Modus"): the
/// game's "Kampfmodus" is its PvP setting.
#[cfg(test)]
const TERM_USES: [(&str, &str); 5] = [
    ("target.encounter", "meter.modes.encounter"),
    ("settings.encounterTimeout.label", "meter.modes.encounter"),
    ("settings.encounterTimeout.hint", "meter.modes.encounter"),
    ("settings.encColumns.title", "meter.modes.encounter"),
    ("settings.encColumns.hint", "meter.modes.encounter"),
];

/// Strings that are the game's own text, each with the game string it comes
/// from, which the Qt screens and the 1.x windows both show:
/// - the Combat Analysis's words for damage received. English 1.x Details
///   keeps "Damage Received" so it fits.
/// - the Combat Analysis's column codes, as the heads of the Details skill
///   columns and the meter's (PERF, DOUB, BACK, FRNT, BLOC, MULT, ENDR, RSTO,
///   HIT). The critical head is CRIT, not the game's CRI, which looked odd;
///   Spanish keeps the game's CRÍT. Most are the same in every
///   language; Spanish, Portuguese and Russian have some of their own.
///   Spanish PERF is the start of the game's
///   "Perforación", which does not fit a head. The game's Block (BLOC) heads
///   Parry: its records count a parried hit as a Block, and
///   the game has no Shield Block count of its own. HIT's full name is the
///   game's "Total Hits".
/// - Max HP, the game's stat name.
/// - The class names, the training dummy's name, Damage, Total Damage,
///   History and Delete: the game's words for each.
/// - The Details overview's Total Hits and battle time, the Skill head, the
///   target list's All and the Reset button: the game's words. English keeps
///   "Combat Time".
/// Strings from one game string say the same in every language but English.
#[cfg(test)]
const GAME_STRINGS: [(&str, &str); 38] = [
    ("details.stats.hits", "String_UI_DAMAGEANALYZER_TOTAL_DAMAGE_COUNT_body"),
    ("details.stats.combatTime", "String_UI_DAMAGEANALYZER_BATTLE_TIME_body"),
    ("details.skills.skill", "String_UI_HUD_EDIT_M_DETAILED_TITLE_SKILL_body"),
    ("details.all", "String_UI_DAMAGEANALYZER_RECORD_TARGET_ALL_body"),
    ("settings.resetDetection", "String_UI_SETTING_BASE_RESET_body"),
    ("details.stats.takenTotal", "String_UI_DAMAGEANALYZER_GET_TOTAL_DAMAGE_body"),
    ("details.skills.takenTooltip", "String_UI_DAMAGEANALYZER_GET_TOTAL_DAMAGE_body"),
    ("details.stats.damageReceived", "String_UI_DAMAGEANALYZER_GET_TOTAL_DAMAGE_body"),
    ("details.stats.takenHits", "String_UI_DAMAGEANALYZER_GET_TOTAL_DAMAGE_COUNT_body"),
    ("details.skills.hitsTakenTooltip", "String_UI_DAMAGEANALYZER_GET_TOTAL_DAMAGE_COUNT_body"),
    ("details.stats.hitsReceived", "String_UI_DAMAGEANALYZER_GET_TOTAL_DAMAGE_COUNT_body"),
    ("details.taken.monster", "String_UI_MINIMAP_LIST_FILTER_MONSTER_body"),
    ("details.skills.hits", "String_UI_DAMAGEANALYZER_SKILL_TOTAL_DAMAGE_COUNT_body"),
    ("meter.columns.hits", "String_UI_DAMAGEANALYZER_SKILL_TOTAL_DAMAGE_COUNT_body"),
    ("details.skills.hitsTooltip", "String_UI_DAMAGEANALYZER_TOTAL_DAMAGE_COUNT_body"),
    ("details.skills.perfect", "String_UI_DAMAGEANALYZER_SKILL_PERFECT_COUNT_body"),
    ("details.skills.double", "String_UI_DAMAGEANALYZER_SKILL_HARDHIT_COUNT_body"),
    ("details.skills.back", "String_UI_DAMAGEANALYZER_SKILL_BACKATTACK_COUNT_body"),
    ("details.skills.frontal", "String_UI_DAMAGEANALYZER_SKILL_FRONTATTACK_COUNT_body"),
    ("details.skills.multiHit", "String_UI_DAMAGEANALYZER_SKILL_ADDITIONALHIT_COUNT_body"),
    ("details.skills.parry", "String_UI_DAMAGEANALYZER_SKILL_BLOCK_COUNT_body"),
    ("details.skills.ironWall", "String_UI_DAMAGEANALYZER_SKILL_IRONWALL_COUNT_body"),
    ("details.skills.regeneration", "String_UI_DAMAGEANALYZER_SKILL_RESTORATION_COUNT_body"),
    ("details.stats.maxHp", "String_AttrStatName_MaxHP_body"),
    ("classes.ASSASSIN", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_ASSASSIN_body"),
    ("classes.CHANTER", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_CHANTER_body"),
    ("classes.CLERIC", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_CLERIC_body"),
    ("classes.ELEMENTALIST", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_ELEMENTALIST_body"),
    ("classes.FIGHTER", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_FIGHTER_body"),
    ("classes.GLADIATOR", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_GLADIATOR_body"),
    ("classes.RANGER", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_RANGER_body"),
    ("classes.SORCERER", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_SORCERER_body"),
    ("classes.TEMPLAR", "String_UI_ITEM_TOOLTIP_MOUSEOVER_TEXT_TEMPLAR_body"),
    ("target.train", "String_STR_M_TraDummy_body"),
    ("details.skills.dmgTooltip", "String_STR_Chat_TypeCombat_Damage_body"),
    ("details.stats.totalDamage", "String_UI_DAMAGEANALYZER_SKILL_TOTAL_DAMAGE_body"),
    ("details.historyBtn", "String_UI_DAMAGEANALYZER_BUTTON_RECORD_body"),
    ("history.delete", "String_UI_DELETE_BTN_DELETE_body"),
];

/// Our languages that the game has, with the game's folder for each.
#[cfg(test)]
const GAME_LOCALES: [(&str, &str); 7] =
    [("de", "de-DE"), ("es", "es-ES"), ("fr", "fr-FR"), ("ja", "ja-JP"), ("ko", "ko-KR"), ("pt", "pt-BR"), ("ru", "ru-RU")];

const UNATTRIBUTED: &str = "meter.unattributed";
const YOU: &str = "meter.you";
const IDENTIFYING: &str = "meter.identifyingPlayer";
const HIDDEN: &str = "meter.hiddenPlayer";
const HIDDEN_NO_CLASS: &str = "meter.hiddenPlayerNoClass";
const ALL_TARGETS: &str = "target.all";
const TRAINING: &str = "target.train";
const ENCOUNTER: &str = "target.encounter";
const WAITING_BOSS: &str = "battleTime.waitingBoss";

/// The keys this file looks up; the QML files' own are found in them.
#[cfg(test)]
fn rust_keys() -> Vec<&'static str> {
    let mut keys = vec![UNATTRIBUTED, YOU, IDENTIFYING, HIDDEN, HIDDEN_NO_CLASS];
    keys.extend([ALL_TARGETS, TRAINING, ENCOUNTER, WAITING_BOSS]);
    keys.extend(CLASS_KEYS.iter().map(|(_, key)| *key));
    keys
}

type Table = HashMap<String, String>;

/// Every file as key -> string, nested keys joined with dots.
fn tables() -> &'static HashMap<&'static str, Table> {
    static TABLES: OnceLock<HashMap<&'static str, Table>> = OnceLock::new();
    TABLES.get_or_init(|| {
        FILES
            .iter()
            .map(|(language, text)| {
                let mut table = Table::new();
                match serde_json::from_str::<serde_json::Value>(text) {
                    Ok(json) => flatten("", &json, &mut table),
                    Err(error) => eprintln!("daevalog-dps-meter: the {language} strings do not load: {error}"),
                }
                (*language, table)
            })
            .collect()
    })
}

fn flatten(prefix: &str, value: &serde_json::Value, out: &mut Table) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
                flatten(&key, value, out);
            }
        }
        serde_json::Value::String(text) => {
            out.insert(prefix.to_string(), text.clone());
        }
        _ => {}
    }
}

/// The string for `key` in `language`, else the English one, else the key
/// itself, so a missing string shows as what it is.
pub fn text(language: &str, key: &str) -> String {
    let tables = tables();
    tables
        .get(language)
        .and_then(|table| table.get(key))
        .or_else(|| tables.get("en").and_then(|table| table.get(key)))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

/// `template` with each `{name}` replaced by its value, as the 1.x page's
/// `format`; a name without a value becomes empty.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if after[..close].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && close > 0 => {
                let name = &after[..close];
                out.push_str(values.iter().find(|(n, _)| *n == name).map_or("", |(_, v)| v));
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

static SYSTEM: OnceLock<Option<&'static str>> = OnceLock::new();

/// Notes the system's languages; once, after Qt has started.
pub fn set_system_languages(tags: &[String]) {
    let _ = SYSTEM.set(from_tags(tags));
}

/// Ours for the first of `tags` (BCP 47, "pt-BR") that we have.
fn from_tags(tags: &[String]) -> Option<&'static str> {
    tags.iter().find_map(|tag| {
        let parts: Vec<String> = tag.split(['-', '_']).map(str::to_ascii_lowercase).collect();
        let first = parts.first()?;
        LANGUAGES.iter().copied().find(|language| language.eq_ignore_ascii_case(first))
    })
}

/// The language to show for the Language setting `choice`.
pub fn resolve(choice: Option<&str>) -> &'static str {
    choice
        .and_then(|choice| LANGUAGES.iter().copied().find(|language| *language == choice))
        .or_else(|| SYSTEM.get().copied().flatten())
        .unwrap_or("en")
}

/// The header's target for the backend's `mode` and target name. In the
/// modes that follow no one target the backend names the fight in English
/// ("All Targets"), so the 1.x page's strings name it instead; BOSS mode
/// waits for a boss until it has one.
pub fn target_name(language: &str, mode: &str, target: &str) -> String {
    let key = match mode {
        "allTargets" => ALL_TARGETS,
        "trainTargets" => TRAINING,
        "encounter" => ENCOUNTER,
        "bossTargets" if target.is_empty() => WAITING_BOSS,
        _ => return target.to_string(),
    };
    text(language, key)
}

/// `hour`:`minute` as Display Time writes a time of day in `language`
/// (history_view `Clock`); Settings shows each choice so.
pub fn clock(language: &str, hour: u32, minute: u32, hour24: bool) -> String {
    let time = chrono::NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default();
    Clock::new(hour24, language).text(&time, false)
}

/// One meter row, for naming its player.
#[derive(Debug, Clone, Copy, Default)]
pub struct Player<'a> {
    pub name: &'a str,
    /// The class's key ("cleric"); empty when unknown.
    pub job: &'a str,
    /// The backend's number for another player; 0 for you.
    pub number: u32,
    pub you: bool,
    /// No name yet: `name` is the player's id.
    pub identifying: bool,
    pub unattributed: bool,
}

/// How a row names its player, as the 1.x page's `playerLabel`: the name;
/// "#id" while the game has not named them; "YOU" for you when chosen; and
/// with `hide_others`, other players as their class and number, so a
/// screenshot names no one. You are never hidden.
pub fn player_name(language: &str, player: Player, hide_others: bool, show_you: bool) -> String {
    if player.unattributed {
        return text(language, UNATTRIBUTED);
    }
    if player.you && show_you {
        return text(language, YOU);
    }
    if player.you || !hide_others {
        return if player.identifying {
            fill(&text(language, IDENTIFYING), &[("id", player.name)])
        } else {
            player.name.to_string()
        };
    }
    let n = player.number.to_string();
    let class = CLASS_KEYS.iter().find(|(job, _)| *job == player.job).map(|(_, key)| text(language, key));
    match class {
        // The backend numbers every other player; without one the class
        // alone still names no one.
        Some(class) if player.number == 0 => class,
        Some(class) => fill(&text(language, HIDDEN), &[("class", &class), ("n", &n)]),
        None => fill(&text(language, HIDDEN_NO_CLASS), &[("n", &n)]),
    }
}

#[derive(Default)]
pub struct StringsRust;

impl qobject::Strings {
    fn resolve(&self, choice: &QString) -> QString {
        let choice = choice.to_string();
        QString::from(resolve(Some(choice.as_str()).filter(|c| !c.is_empty())))
    }

    fn text(&self, language: &QString, key: &QString) -> QString {
        QString::from(&text(&language.to_string(), &key.to_string()))
    }

    fn target_name(&self, language: &QString, mode: &QString, target: &QString) -> QString {
        QString::from(&target_name(&language.to_string(), &mode.to_string(), &target.to_string()))
    }

    fn clock(&self, language: &QString, hour: i32, minute: i32, hour24: bool) -> QString {
        let part = |n: i32| u32::try_from(n).unwrap_or(0);
        QString::from(&clock(&language.to_string(), part(hour), part(minute), hour24))
    }

    #[allow(clippy::too_many_arguments)]
    fn player_name(
        &self,
        language: &QString,
        name: &QString,
        job: &QString,
        number: i32,
        you: bool,
        identifying: bool,
        unattributed: bool,
        hide_others: bool,
        show_you: bool,
    ) -> QString {
        let (name, job) = (name.to_string(), job.to_string());
        let player = Player {
            name: &name,
            job: &job,
            number: u32::try_from(number).unwrap_or(0),
            you,
            identifying,
            unattributed,
        };
        QString::from(&player_name(&language.to_string(), player, hide_others, show_you))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Every `I18n.tr("key")` in the QML files (in I18n.qml itself, `tr("key")`).
    /// A key built at run time would escape this, so any other use of `tr(`
    /// fails, as does a `qsTr`, which would bypass the files.
    fn qml_keys() -> BTreeSet<String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut keys = BTreeSet::new();
        let mut files = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("qml") {
                continue;
            }
            files += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(!text.contains("qsTr("), "{file}: qsTr bypasses the translation files; use I18n.tr");
            for (at, _) in text.match_indices("tr(") {
                // Only the word tr: not "Str(" or "attr(".
                if text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let rest = &text[at + 3..];
                if rest.starts_with("key)") && text[..at].ends_with("function ") {
                    continue;
                }
                let Some(literal) = rest.strip_prefix('"') else {
                    panic!("{file}: a key that is not written out whole: tr({}", &rest[..rest.len().min(30)]);
                };
                keys.insert(literal[..literal.find('"').unwrap()].to_string());
            }
        }
        assert!(files >= 6, "found {files} QML files in {}", dir.display());
        keys
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            rest = &rest[open + 1..];
            if let Some(close) = rest.find('}') {
                names.insert(rest[..close].to_string());
                rest = &rest[close + 1..];
            }
        }
        names
    }

    #[test]
    fn every_key_the_meter_uses_is_in_all_ten_languages() {
        let mut keys = qml_keys();
        assert!(keys.len() >= 15, "only {} keys found in the QML files: {keys:?}", keys.len());
        keys.extend(rust_keys().into_iter().map(String::from));
        let tables = tables();
        assert_eq!(tables.len(), LANGUAGES.len());
        let mut missing = Vec::new();
        for key in &keys {
            let Some(english) = tables["en"].get(key) else {
                missing.push(format!("en: {key}"));
                continue;
            };
            for language in LANGUAGES {
                match tables[language].get(key) {
                    None => missing.push(format!("{language}: {key}")),
                    Some(text) if text.trim().is_empty() => missing.push(format!("{language}: {key} is empty")),
                    Some(text) if placeholders(text) != placeholders(english) => {
                        missing.push(format!("{language}: {key} has other placeholders than English"));
                    }
                    Some(_) => {}
                }
            }
        }
        assert!(missing.is_empty(), "{} missing:\n{}", missing.len(), missing.join("\n"));
    }

    #[test]
    fn every_mode_and_tab_name_has_its_game_source() {
        let named: BTreeSet<&str> = GAME_TERMS.iter().map(|term| term.key).collect();
        let wanted: BTreeSet<&str> = tables()["en"]
            .keys()
            .filter(|key| key.starts_with("meter.modes.") || key.starts_with("meter.tabs."))
            .map(String::as_str)
            .collect();
        assert_eq!(named, wanted);
        for term in &GAME_TERMS {
            for (language, _) in term.other {
                assert!(GAME_LOCALES.iter().any(|(ours, _)| ours == language), "{}: {language}", term.key);
            }
        }
    }

    #[test]
    fn strings_that_speak_of_a_mode_use_its_word() {
        let mut wrong = Vec::new();
        for (key, mode) in TERM_USES {
            assert!(GAME_TERMS.iter().any(|term| term.key == mode), "{mode} has no game source");
            for language in LANGUAGES.into_iter().filter(|language| *language != "en") {
                let (ours, word) = (text(language, key), text(language, mode));
                if !ours.to_lowercase().contains(&word.to_lowercase()) {
                    wrong.push(format!("{language} {key}: {ours:?} does not say {word:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn strings_from_one_game_string_say_the_same() {
        let mut wrong = Vec::new();
        for (at, (key, source)) in GAME_STRINGS.iter().enumerate() {
            assert!(tables()["en"].contains_key(*key), "{key} is not in the English file");
            for (other, _) in GAME_STRINGS[..at].iter().filter(|(_, from)| from == source) {
                for language in LANGUAGES.into_iter().filter(|language| *language != "en") {
                    let (theirs, ours) = (text(language, other), text(language, key));
                    if theirs != ours {
                        wrong.push(format!("{language}: {other} says {theirs:?}, {key} says {ours:?}"));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// Each mode and tab name, and each of `GAME_STRINGS`, is in the game's
    /// text it comes from. Needs the game's strings: DAEVALOG_GAME_L10N, a
    /// folder with one folder per locale (`de-DE`, ...), each holding the
    /// game's one JSON file of strings for it.
    #[test]
    fn mode_and_tab_names_are_the_games_words() {
        let Some(folder) = std::env::var_os("DAEVALOG_GAME_L10N") else {
            eprintln!("DAEVALOG_GAME_L10N is not set: the game's words are not checked");
            return;
        };
        let mut wrong = Vec::new();
        for (language, locale) in GAME_LOCALES {
            let dir = Path::new(&folder).join(locale);
            let files: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            assert_eq!(files.len(), 1, "{}: {files:?}", dir.display());
            let game: serde_json::Value = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
            for term in &GAME_TERMS {
                if let Some((_, word)) = term.chosen.iter().find(|(l, _)| *l == language) {
                    assert_eq!(text(language, term.key), *word, "{language} {}", term.key);
                    continue;
                }
                let source = term.other.iter().find(|(l, _)| *l == language).map_or(term.source, |(_, key)| key);
                let said = game["Entries"][source].as_str().unwrap_or_else(|| panic!("{locale}: no {source}"));
                let ours = text(language, term.key);
                if !said.to_lowercase().contains(&ours.to_lowercase()) {
                    wrong.push(format!("{language} {}: {ours:?} is not in {source} {said:?}", term.key));
                }
            }
            for (key, source) in GAME_STRINGS {
                let said = game["Entries"][source].as_str().unwrap_or_else(|| panic!("{locale}: no {source}"));
                let ours = text(language, key);
                if !said.to_lowercase().contains(&ours.to_lowercase()) {
                    wrong.push(format!("{language} {key}: {ours:?} is not in {source} {said:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn a_missing_string_falls_back_to_english_then_to_its_key() {
        assert_eq!(text("ko", "tray.quit"), "종료");
        assert_eq!(text("xx", "tray.quit"), "Quit");
        assert_eq!(text("de", "no.such.key"), "no.such.key");
    }

    #[test]
    fn the_header_names_a_fight_without_one_target_in_the_language() {
        // The backend's English names for these modes are not shown.
        assert_eq!(target_name("de", "allTargets", "All Targets"), "Alle Gegner");
        assert_eq!(target_name("ja", "trainTargets", "Train"), "訓練用カカシ");
        assert_eq!(target_name("ru", "encounter", "Encounter"), "Бой");
        assert_eq!(target_name("en", "encounter", "Encounter"), "Encounter");
        // A target's own name is the backend's, already in the language.
        assert_eq!(target_name("ru", "bossTargets", "Boss"), "Boss");
        assert_eq!(target_name("ru", "lastHitByMe", "Mob"), "Mob");
        assert_eq!(target_name("fr", "bossTargets", ""), "Pas encore de boss");
        assert_eq!(target_name("fr", "lastHitByMe", ""), "");
    }

    #[test]
    fn placeholders_are_filled_by_name() {
        assert_eq!(fill("{class} {n}", &[("n", "2"), ("class", "Cleric")]), "Cleric 2");
        assert_eq!(fill("#{id}", &[]), "#");
        assert_eq!(fill("{ not a name } {}", &[]), "{ not a name } {}");
    }

    #[test]
    fn the_language_is_the_setting_else_the_systems_else_english() {
        let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(from_tags(&tags(&["de-DE", "de"])), Some("de"));
        assert_eq!(from_tags(&tags(&["pt-BR"])), Some("pt"));
        // Chinese is not shipped: the next language the system lists, else English.
        assert_eq!(from_tags(&tags(&["zh-CN", "zh"])), None);
        assert_eq!(from_tags(&tags(&["zh-Hant-TW", "ja-JP"])), Some("ja"));
        assert_eq!(from_tags(&tags(&["nl-NL", "fr-BE"])), Some("fr"));
        assert_eq!(from_tags(&tags(&["nl-NL"])), None);
        // No system languages noted in tests: English.
        assert_eq!(resolve(Some("ja")), "ja");
        assert_eq!(resolve(Some("xx")), resolve(None));
        assert_eq!(resolve(Some("zh-Hans")), resolve(None), "a Chinese setting left from before");
    }

    fn row<'a>(name: &'a str, job: &'a str, number: u32) -> Player<'a> {
        Player { name, job, number, ..Player::default() }
    }

    #[test]
    fn players_show_by_name_unless_hidden() {
        let other = row("Someone", "cleric", 2);
        assert_eq!(player_name("en", other, false, false), "Someone");
        assert_eq!(player_name("en", other, true, false), "Cleric 2");
        assert_eq!(player_name("ko", other, true, false), "치유성 2");
        assert_eq!(player_name("en", row("Someone", "fighter", 3), true, false), "Brawler 3");
        assert_eq!(player_name("en", row("Someone", "", 4), true, false), "Player 4");
        assert_eq!(player_name("en", row("Someone", "cleric", 0), true, false), "Cleric");

        // While the game has not named a player.
        let unnamed = Player { identifying: true, ..row("1234", "", 5) };
        assert_eq!(player_name("en", unnamed, false, false), "#1234");
        assert_eq!(player_name("en", unnamed, true, false), "Player 5");

        // You are never hidden; "YOU" when chosen.
        let you = Player { you: true, ..row("Myself", "templar", 0) };
        assert_eq!(player_name("en", you, true, false), "Myself");
        assert_eq!(player_name("en", you, true, true), "YOU");

        let summons = Player { unattributed: true, ..row("80000000", "", 0) };
        assert_eq!(player_name("en", summons, true, true), "Unattributed summons and effects");
    }

    #[test]
    fn display_time_choices_show_a_time_in_their_format() {
        assert_eq!(clock("en", 15, 30, false), "3:30 PM");
        assert_eq!(clock("en", 15, 30, true), "15:30");
        assert_eq!(clock("ko", 15, 30, false), "오후 3:30");
        assert_eq!(clock("de", 15, 30, true), "15:30");
        assert_eq!(clock("en", 0, 5, false), "12:05 AM");
    }
}
