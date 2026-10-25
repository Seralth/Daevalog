// SPDX-License-Identifier: AGPL-3.0-only

//! Checks on the QML text that no QML test reaches: every control has an
//! action, and every call from QML into a Rust object passes as many
//! arguments as the Rust function takes. Both kinds of bug look finished on
//! screen and do nothing (a menu with no handler, or 3 arguments passed to a
//! 4-argument `Overlay.capture`).

use std::{collections::HashMap, path::{Path, PathBuf}};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A `#rrggbb` colour (quoted or not) as red, green and blue from 0 to 1.
fn rgb(hex: &str) -> [f64; 3] {
    let n = u32::from_str_radix(hex.trim().trim_matches('"').trim_start_matches('#'), 16).unwrap();
    [n >> 16, n >> 8 & 0xff, n & 0xff].map(|c| f64::from(c) / 255.0)
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("source folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .collect();
    out.sort();
    out
}

/// The text with string contents and comments blanked (same length, same
/// lines), so braces and parentheses count only as code.
fn code_only(text: &str) -> Vec<char> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' | '\'' | '`' => {
                let quote = chars[i];
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' {
                        out[i] = ' ';
                        i += 1;
                    }
                    if i < chars.len() && chars[i] != '\n' {
                        out[i] = ' ';
                    }
                    i += 1;
                }
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    out[i] = ' ';
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    if chars[i] != '\n' {
                        out[i] = ' ';
                    }
                    i += 1;
                }
                if i < chars.len() {
                    out[i] = ' ';
                    out[i + 1] = ' ';
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    out
}

fn line_of(chars: &[char], at: usize) -> usize {
    chars[..at].iter().filter(|&&c| c == '\n').count() + 1
}

/// The index of the bracket that closes the one at `open`.
fn closing(chars: &[char], open: usize) -> Option<usize> {
    let (o, c) = (chars[open], match chars[open] { '{' => '}', '(' => ')', '[' => ']', _ => return None });
    let mut depth = 0;
    for (i, &ch) in chars.iter().enumerate().skip(open) {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Whether `word` starts at `at` as a whole word.
fn word_at(chars: &[char], at: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    let before = at == 0 || !(chars[at - 1].is_alphanumeric() || chars[at - 1] == '_' || chars[at - 1] == '.');
    let after = chars.get(at + w.len()).is_none_or(|c| !(c.is_alphanumeric() || *c == '_'));
    before && after && chars.get(at..at + w.len()) == Some(&w[..])
}

/// The text of a block's own level: nested blocks left out.
fn own_level(chars: &[char], open: usize, close: usize) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for &ch in &chars[open + 1..close] {
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Each control type and the handlers that give it an action.
const CONTROLS: [(&str, &[&str]); 8] = [
    ("UiButton", &["onClicked"]),
    ("UiMenuItem", &["onTriggered"]),
    ("UiSwitch", &["onToggled"]),
    ("UiSlider", &["onMoved"]),
    ("UiDropdown", &["onPicked"]),
    ("UiSegmented", &["onPicked"]),
    ("UiField", &["onEdited", "onTyping"]),
    ("Platform.MenuItem", &["onTriggered"]),
];

/// A `control { ... }` block: where its name and its braces are, and the
/// text of its own level.
struct Block {
    at: usize,
    open: usize,
    close: usize,
    own: String,
}

/// Every `control { ... }` block in `chars` (code only).
fn blocks_of(chars: &[char], control: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let rest: String = chars[at..].iter().collect();
        let Some(byte) = rest.find(control) else { break };
        let pos = at + rest[..byte].chars().count();
        at = pos + control.chars().count();
        if !word_at(chars, pos, control) {
            continue;
        }
        let rest: String = chars[at..].iter().take_while(|c| c.is_whitespace() || **c == '{').collect();
        let Some(open) = rest.find('{').map(|b| at + b) else { continue };
        let Some(close) = closing(chars, open) else { continue };
        out.push(Block { at: pos, open, close, own: own_level(chars, open, close) });
    }
    out
}

/// Whether a block's own level sets `handler`.
fn sets(own: &str, handler: &str) -> bool {
    own.match_indices(handler).any(|(i, _)| own[i + handler.len()..].trim_start().starts_with(':'))
}

/// Controls with no action: `file:line type`. A control meant to do
/// nothing says so with a `// no action:` comment inside its block.
pub fn controls_without_action(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for path in files(dir, "qml") {
        let text = std::fs::read_to_string(&path).expect("qml file");
        let raw: Vec<char> = text.chars().collect();
        let chars = code_only(&text);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for (control, handlers) in CONTROLS {
            for block in blocks_of(&chars, control) {
                let exempt = raw[block.open..block.close].iter().collect::<String>().contains("// no action:");
                if !handlers.iter().any(|h| sets(&block.own, h)) && !exempt {
                    found.push(format!("{name}:{} {control}", line_of(&chars, block.at)));
                }
            }
        }
    }
    found
}

/// Meter rows without both of a row's actions, opening Details and the
/// hover card: `file:line`. The split view's healers once had only the
/// first.
pub fn meter_rows_missing_an_action(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for path in files(dir, "qml") {
        let chars = code_only(&std::fs::read_to_string(&path).expect("qml file"));
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for block in blocks_of(&chars, "MeterRow") {
            for handler in ["onClicked", "onPointerOver"] {
                if !sets(&block.own, handler) {
                    found.push(format!("{name}:{} MeterRow {handler}", line_of(&chars, block.at)));
                }
            }
        }
    }
    found
}

/// The value a block's own level gives `property`, trimmed; `None` when it
/// gives none.
fn value_of(own: &str, property: &str) -> Option<String> {
    own.split(['\n', ';']).find_map(|line| {
        let rest = line.trim().strip_prefix(property)?.trim_start().strip_prefix(':')?;
        Some(rest.trim().to_string())
    })
}

/// Tables whose bars can come out stronger than the windows' cap, under
/// text that is not white: `file:line`. UiTable's own default must be the
/// cap, and a table with bars may set no other strength. History's tables
/// drew the meter's 40% under the blue names (2.9:1 to 4.2:1).
pub fn tables_with_strong_bars(dir: &Path) -> Vec<String> {
    const CAP: &str = "Theme.tableBarStrength";
    let mut found = Vec::new();
    for path in files(dir, "qml") {
        let chars = code_only(&std::fs::read_to_string(&path).expect("qml file"));
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if name == "UiTable.qml" {
            let text: String = chars.iter().collect();
            if !text.lines().any(|l| l.trim() == format!("property real barStrength: {CAP}")) {
                found.push(format!("{name} barStrength default"));
            }
            continue;
        }
        for block in blocks_of(&chars, "UiTable") {
            if !sets(&block.own, "barFill") {
                continue;
            }
            if value_of(&block.own, "barStrength").is_some_and(|v| v != CAP) {
                found.push(format!("{name}:{} UiTable barStrength", line_of(&chars, block.at)));
            }
        }
    }
    found
}

fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for ch in snake.chars() {
        if ch == '_' {
            up = true;
        } else if up {
            out.extend(ch.to_uppercase());
            up = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Splits `text` at commas outside brackets; `angle`: `<` and `>` are
/// brackets too (Rust types; in JavaScript they compare).
fn split_top(text: &str, angle: bool) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut depth = 0;
    for ch in text.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' if angle => depth += 1,
            '>' if angle => depth -= 1,
            ',' if depth == 0 => {
                parts.push(String::new());
                continue;
            }
            _ => {}
        }
        parts.last_mut().unwrap().push(ch);
    }
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// The QML-callable functions of the Rust objects: (object, camelCase name)
/// to the number of arguments after `self`.
pub fn invokables(src: &Path) -> HashMap<(String, String), usize> {
    let mut out = HashMap::new();
    // This file names the attribute in its own code and test.
    for path in files(src, "rs").into_iter().filter(|p| !p.ends_with("qml_checks.rs")) {
        let text = std::fs::read_to_string(&path).expect("rust file");
        let mut rest = text.as_str();
        while let Some(at) = rest.find("#[qinvokable]") {
            rest = &rest[at + "#[qinvokable]".len()..];
            let Some(fn_at) = rest.find("fn ") else { break };
            let after = &rest[fn_at + 3..];
            let Some(paren) = after.find('(') else { break };
            let name = after[..paren].trim().to_string();
            let chars: Vec<char> = after.chars().collect();
            let open = after[..paren].chars().count();
            let Some(close) = closing(&chars, open) else { break };
            let params: String = chars[open + 1..close].iter().collect();
            let params = split_top(&params, true);
            let Some(self_param) = params.first() else { continue };
            let object = self_param.split(|c: char| !c.is_alphanumeric() && c != '_')
                .filter(|w| w.chars().next().is_some_and(char::is_uppercase) && *w != "Pin")
                .last()
                .unwrap_or_default()
                .to_string();
            if !object.is_empty() {
                out.insert((object, camel(&name)), params.len() - 1);
            }
        }
    }
    out
}

/// QML calls into Rust objects whose argument count differs from the Rust
/// function's, or that name no function of that object: `file:line message`.
pub fn calls_with_wrong_arguments(qml: &Path, src: &Path) -> Vec<String> {
    let known = invokables(src);
    let objects: Vec<String> = known.keys().map(|(o, _)| o.clone()).collect();
    let mut found = Vec::new();
    for path in files(qml, "qml") {
        let text = std::fs::read_to_string(&path).expect("qml file");
        let chars = code_only(&text);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let flat: String = chars.iter().collect();
        for object in objects.iter().collect::<std::collections::BTreeSet<_>>() {
            let needle = format!("{object}.");
            for (byte_at, _) in flat.match_indices(&needle) {
                let at = flat[..byte_at].chars().count();
                if !word_at(&chars, at, object) {
                    continue;
                }
                let start = at + needle.chars().count();
                let method: String = chars[start..].iter().take_while(|c| c.is_alphanumeric() || **c == '_').collect();
                let open = start + method.chars().count();
                if chars.get(open) != Some(&'(') {
                    continue;
                }
                let Some(close) = closing(&chars, open) else { continue };
                let args = split_top(&chars[open + 1..close].iter().collect::<String>(), false).len();
                let line = line_of(&chars, at);
                match known.get(&(object.clone(), method.clone())) {
                    Some(&n) if n == args => {}
                    Some(&n) => found.push(format!("{name}:{line} {object}.{method} takes {n}, given {args}")),
                    None => found.push(format!("{name}:{line} {object}.{method} is no function of {object}")),
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_has_an_action() {
        let found = controls_without_action(&root());
        assert!(found.is_empty(), "controls with no action:\n{}", found.join("\n"));
    }

    #[test]
    fn every_meter_row_opens_details_and_shows_the_card() {
        let found = meter_rows_missing_an_action(&root());
        assert!(found.is_empty(), "meter rows:\n{}", found.join("\n"));
    }

    #[test]
    fn every_call_into_rust_passes_the_arguments_it_takes() {
        let found = calls_with_wrong_arguments(&root(), &root().join("src"));
        assert!(found.is_empty(), "calls into Rust objects:\n{}", found.join("\n"));
    }

    #[test]
    fn every_table_keeps_its_bars_under_the_cap() {
        let found = tables_with_strong_bars(&root());
        assert!(found.is_empty(), "tables:\n{}", found.join("\n"));
    }

    #[test]
    fn keep_out_of_the_taskbar_shows_only_where_it_can_act() {
        // It showed on Wayland, where no app can ask.
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let raw: Vec<char> = text.chars().collect();
        let chars = code_only(&text);
        let rows: Vec<Block> = blocks_of(&chars, "SwitchRow")
            .into_iter()
            .filter(|b| raw[b.open..b.close].iter().collect::<String>().contains("\"settings.hideFromTaskbar\""))
            .collect();
        assert_eq!(rows.len(), 1, "one row for the setting");
        assert_eq!(value_of(&rows[0].own, "visible").as_deref(), Some("Overlay.taskbarSetting"));
        // It takes every window off the taskbar, and says so.
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        assert!(main.contains("for (const window of [app.meter, app.details, app.history, app.settings, app.character])"));
        assert!(crate::i18n::text("en", "settings.hideFromTaskbarHint").starts_with("Every Daevalog window"));
    }

    #[test]
    fn auto_hide_shows_only_where_it_can_act() {
        // On Linux the game always counted as in front, so it never hid.
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let raw: Vec<char> = text.chars().collect();
        let chars = code_only(&text);
        let rows: Vec<Block> = blocks_of(&chars, "SwitchRow")
            .into_iter()
            .filter(|b| raw[b.open..b.close].iter().collect::<String>().contains("\"settings.autoHideMeter\""))
            .collect();
        assert_eq!(rows.len(), 1, "one row for the setting");
        assert_eq!(value_of(&rows[0].own, "visible").as_deref(), Some("Overlay.autoHideSetting"));
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        assert!(main.contains("enabled: Settings.autoHide && Overlay.autoHideSetting && !app.screenshotMode"));
        assert_eq!(daevalog::platform::window_detector::SEES_FOREGROUND, cfg!(windows));
    }

    #[test]
    fn details_font_size_reaches_every_table_list_and_chart() {
        // It moved the labels and left the tables at 13 and 11 px.
        let text = std::fs::read_to_string(root().join("DetailsWindow.qml")).unwrap();
        let chars = code_only(&text);
        for control in ["UiTable", "UiDropdown", "DpsChart", "Timeline"] {
            let blocks = blocks_of(&chars, control);
            assert!(!blocks.is_empty(), "{control}");
            for b in blocks {
                let size = value_of(&b.own, "textSize");
                let want = if control == "UiTable" || control == "UiDropdown" { "details.body" } else { "details.small" };
                assert_eq!(size.as_deref(), Some(want), "{control} at line {}", line_of(&chars, b.open));
                if control == "UiTable" {
                    assert_eq!(value_of(&b.own, "headSize").as_deref(), Some("details.small"));
                }
            }
        }
        let table = std::fs::read_to_string(root().join("UiTable.qml")).unwrap();
        assert!(table.contains("font.pixelSize: cell.head ? table.headSize : table.textSize"));
        for file in ["UiTable.qml", "UiDropdown.qml", "DpsChart.qml", "Timeline.qml", "TimeAxis.qml"] {
            let code: String = code_only(&std::fs::read_to_string(root().join(file)).unwrap()).iter().collect();
            assert!(!code.contains("font.pixelSize: Theme."), "{file}: a fixed size");
        }
    }

    #[test]
    fn a_locked_meter_shows_the_padlock() {
        // Locked from the ⋯ menu, the meter looked the same and seemed frozen.
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        let chars = code_only(&text);
        let lock = blocks_of(&chars, "HeaderButton").into_iter().find(|b| b.own.contains("id: lockButton")).expect("the lock");
        let own: String = text.chars().skip(lock.open).take(lock.close - lock.open).collect();
        assert!(own.contains("visible: Settings.locked || (Settings.showLockButton && meter.canLock)"));
        assert_eq!(value_of(&lock.own, "clickable").as_deref(), Some("Settings.showLockButton"));
        assert!(text.contains("TapHandler { enabled: button.clickable; onTapped: button.clicked() }"));
    }

    #[test]
    fn every_header_button_of_the_meter_has_a_tooltip() {
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        let chars = code_only(&text);
        let buttons = blocks_of(&chars, "HeaderButton");
        assert_eq!(buttons.len(), 4, "pause, lock, ⋯ and ✕");
        for b in &buttons {
            let own: String = text.chars().skip(b.open).take(b.close - b.open).collect();
            assert!(own.contains("UiTip {"), "line {}", line_of(&chars, b.open));
        }
        // ✕ says what it will do: hide to the tray, or quit without one.
        assert!(text.contains("text: meter.closeHides ? I18n.tr(\"meter.hideToTray\") : I18n.tr(\"tray.quit\")"));
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        assert!(main.contains("meter.closeHides = Qt.binding(() => tray.available && tray.visible)"));
        assert!(main.contains("function closeMeter() {\n        if (tray.available && tray.visible)\n            meter.shown = false"));
    }

    #[test]
    fn a_paused_meter_shows_a_play_mark() {
        // Paused, the button kept its two bars, only dimmer.
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        let chars = code_only(&text);
        let pause = blocks_of(&chars, "HeaderButton").into_iter().find(|b| b.own.contains("id: suspendButton")).expect("the pause button");
        let own: String = text.chars().skip(pause.open).take(pause.close - pause.open).collect();
        assert_eq!(own.matches("model: Capture.suspended ? [] : [-3, 3]").count(), 2, "the bars only while running");
        assert!(own.contains("Canvas {\n                            visible: Capture.suspended"), "the play mark while paused");
    }

    #[test]
    fn auto_upload_needs_a_sign_in_and_shows_its_last_failure() {
        // It could be on while signed out, and failures went only to stderr.
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let raw: Vec<char> = text.chars().collect();
        let chars = code_only(&text);
        let rows: Vec<Block> = blocks_of(&chars, "SwitchRow")
            .into_iter()
            .filter(|b| raw[b.open..b.close].iter().collect::<String>().contains("\"settings.account.autoUpload\""))
            .collect();
        assert_eq!(rows.len(), 1, "one row for the setting");
        assert_eq!(value_of(&rows[0].own, "enabled").as_deref(), Some("signedIn"));
        let row: String = raw[rows[0].open..rows[0].close].iter().collect();
        assert!(row.contains("I18n.tr(\"settings.account.autoUploadFailed\").replace(\"{error}\", Account.uploadFailure)"));
        let switch = std::fs::read_to_string(root().join("UiSwitch.qml")).unwrap();
        assert!(switch.contains("opacity: enabled ? 1 : Theme.disabledOpacity"), "a disabled switch looks it");
    }

    #[test]
    fn the_meters_screenshot_says_what_happened() {
        // Its result was thrown away: no note either way.
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        assert!(main.contains("onTriggered: app.meter.screenshot()"));
        assert!(!main.contains("Overlay.capture("), "the meter captures itself");
        let meter = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        assert!(meter.contains("shotNote = result.clipboard ? I18n.tr(\"details.screenshot.savedToClipboard\") : I18n.tr(\"details.screenshot.failed\")"));
        assert!(meter.contains("text: meter.shotNote !== \"\" ? meter.shotNote"), "shown in the header");
        assert!(meter.contains("interval: 2000\n        onTriggered: meter.shotNote = \"\""), "for 2 s");
    }

    #[test]
    fn a_skill_column_switch_shows_its_change_on_dmg() {
        // On HEAL and TAKEN every switch looked dead.
        let text = std::fs::read_to_string(root().join("DetailsWindow.qml")).unwrap();
        assert!(text.contains("Settings.skillColumns = JSON.stringify(list)\n                            if (details.mode !== \"dmg\") {\n                                details.mode = \"dmg\""));
    }

    #[test]
    fn a_damage_over_time_row_counts_ticks() {
        // Its panel said "Normal hits" for ticks.
        let text = std::fs::read_to_string(root().join("DetailsWindow.qml")).unwrap();
        assert!(text.contains("readonly property bool ticks: shown && details.skill.dot === true"));
        assert!(text.contains("label: hitList.ticks ? I18n.tr(\"details.hits.ticks\") : I18n.tr(\"details.hits.normal\")"));
        assert!(text.contains("label: hitList.ticks ? I18n.tr(\"details.hits.criticalTicks\") : I18n.tr(\"details.hits.critical\")"));
    }

    #[test]
    fn only_tables_with_a_row_action_shade_their_rows() {
        // Every row lit up under the pointer, where most clicks did nothing.
        let table = std::fs::read_to_string(root().join("UiTable.qml")).unwrap();
        assert!(table.contains("rowHover.hovered && table.clickable ? Theme.hover"));
        assert!(table.contains("enabled: table.clickable\n                onTapped: table.rowClicked"));
        let mut acting = 0;
        for path in files(&root(), "qml") {
            let text = std::fs::read_to_string(&path).unwrap();
            let chars = code_only(&text);
            for b in blocks_of(&chars, "UiTable") {
                let own: String = text.chars().skip(b.open).take(b.close - b.open).collect();
                let acts = own.contains("onRowClicked:");
                assert_eq!(value_of(&b.own, "clickable").as_deref() == Some("true"), acts, "{}:{}", path.display(), line_of(&chars, b.open));
                acting += usize::from(acts);
            }
        }
        assert_eq!(acting, 2, "Details' skills and History's players");
    }

    #[test]
    fn historys_filters_read_as_lists() {
        // "Boss name..." and "Date (YYYY-MM-DD)..." were 1.x's text fields.
        let text = std::fs::read_to_string(root().join("HistoryWindow.qml")).unwrap();
        for key in ["allBosses", "allClasses", "allDays"] {
            assert!(text.contains(&format!("options: [{{ \"label\": I18n.tr(\"history.{key}\"), \"value\": \"\" }}]")), "{key}");
        }
        assert_eq!(crate::i18n::text("en", "history.allBosses"), "All bosses");
        assert_eq!(crate::i18n::text("en", "history.allDays"), "All days");
    }

    #[test]
    fn the_train_setting_says_it_is_for_details() {
        // "Training Mode target" read as if it changed the TRAIN rows.
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        assert!(text.contains("label: I18n.tr(\"settings.trainingMode.label\").replace(\"{mode}\", I18n.modeName(\"trainTargets\"))"));
        assert!(text.contains("hint: I18n.tr(\"settings.trainingMode.hint\").replace(\"{mode}\", I18n.modeName(\"trainTargets\"))"));
        assert_eq!(crate::i18n::text("en", "settings.trainingMode.label"), "Details from {mode} rows");
    }

    #[test]
    fn default_mode_says_it_switches_the_meter_now() {
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        assert!(text.contains("label: I18n.tr(\"settings.defaultMeterMode.label\")\n                        hint: I18n.tr(\"settings.defaultMeterMode.hint\")"));
        let live = std::fs::read_to_string(root().join("src/live.rs")).unwrap();
        assert!(live.contains("if follow_default_mode(&mut default_mode, &values, &mut feed.meter.lock()) {\n                request_clear();"), "what the hint says");
    }

    #[test]
    fn the_name_field_says_the_game_name_wins() {
        // It was labelled "Name#ID", and a typed name changed under the player.
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        assert!(text.contains("label: I18n.tr(\"settings.characterName\")\n                        hint: I18n.tr(\"settings.characterNameHint\")"));
        assert_eq!(crate::i18n::text("en", "settings.characterName"), "Character name");
    }

    #[test]
    fn split_view_shows_its_healers_line_before_a_healer_qualifies() {
        // It showed nothing until a Cleric or Chanter had 10% of the healing.
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        assert!(text.contains("readonly property bool splitShown: Settings.splitView && tab === 0"));
        let chars = code_only(&text);
        let heads: Vec<String> = blocks_of(&chars, "ColumnHeads").iter().filter_map(|b| value_of(&b.own, "visible")).collect();
        assert!(heads.contains(&"meter.splitShown".to_string()), "the healers' heads");
        let raw: Vec<char> = text.chars().collect();
        let notes: Vec<Block> = blocks_of(&chars, "SmallText")
            .into_iter()
            .filter(|b| raw[b.open..b.close].iter().collect::<String>().contains("\"meter.noHealer\""))
            .collect();
        assert_eq!(notes.len(), 1);
        assert_eq!(value_of(&notes[0].own, "visible").as_deref(), Some("meter.splitShown && !meter.split"));
    }

    #[test]
    fn settings_that_act_in_some_modes_say_so() {
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let raw: Vec<char> = text.chars().collect();
        let chars = code_only(&text);
        let row = |key: &str| -> String {
            let rows: Vec<Block> = blocks_of(&chars, "SettingRow")
                .into_iter()
                .filter(|b| raw[b.open..b.close].iter().collect::<String>().contains(&format!("\"settings.{key}.label\"")))
                .collect();
            assert_eq!(rows.len(), 1, "{key}");
            raw[rows[0].open..rows[0].close].iter().collect()
        };
        // Greyed where they cannot show.
        assert!(row("groupDamage").contains("enabled: Settings.showTotal"));
        assert!(row("healthText").contains("enabled: !Settings.slimMode"));
        // The mode named where only one acts on them.
        assert!(row("healthText").contains("settings.modesOnly([\"bossTargets\", \"lastHitByMe\"])"));
        assert!(row("encounterTimeout").contains("settings.modesOnly([\"encounter\"])"));
        assert!(row("allTargetsWindow").contains("settings.modesOnly([\"allTargets\"])"));
        let segmented = std::fs::read_to_string(root().join("UiSegmented.qml")).unwrap();
        assert!(segmented.contains("opacity: enabled ? 1 : Theme.disabledOpacity"));
    }

    #[test]
    fn start_in_the_tray_says_it_needs_a_tray() {
        // Without a tray the meter shows anyway (Main.qml), and the switch stays on.
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        assert!(main.contains("if (Settings.startInTray && tray.available && tray.visible)"));
        assert!(crate::i18n::text("en", "settings.startInTrayHint").contains("Needs a system tray"));
    }

    #[test]
    fn bold_dps_is_as_bold_as_bold_names() {
        // Medium on every number column barely showed.
        let row = std::fs::read_to_string(root().join("MeterRow.qml")).unwrap();
        assert!(row.contains("font.weight: Settings.boldNames ? Font.DemiBold : Font.Normal"));
        assert!(row.contains("font.weight: Settings.boldDps && row.kinds[index] === \"main\" ? Font.DemiBold : Font.Normal"));
    }

    #[test]
    fn settings_hints_say_what_else_they_do() {
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        assert!(text.contains("hint: I18n.tr(\"settings.barStrength.hint\") + \" \" + I18n.tr(\"settings.barStrength.tableHint\")"), "the tables' cap");
        assert!(text.contains(": I18n.tr(\"settings.detailsMonitor.closeHint\")"), "closing Details turns it off");
        assert!(text.contains("label: I18n.tr(\"settings.showTotalDps\")\n                        hint: I18n.tr(\"settings.showTotalHint\")"), "every tab's total");
        let theme = std::fs::read_to_string(root().join("Theme.qml")).unwrap();
        assert!(theme.contains("tableBarStrength: Math.min(barStrength, 0.12)"), "the 12% the hint names");
    }

    #[test]
    fn history_looks_busy_and_offers_no_one_way_choice() {
        // Upload ignored clicks while busy and looked the same.
        let history = std::fs::read_to_string(root().join("HistoryWindow.qml")).unwrap();
        assert_eq!(history.matches("enabled: !history.busy").count(), 3, "Upload, Upload again, Preview");
        let button = std::fs::read_to_string(root().join("UiButton.qml")).unwrap();
        assert!(button.contains("opacity: enabled ? 1 : Theme.disabledOpacity"));
        // "Numbers shown" with the one choice "Game record" was no choice.
        let record = std::fs::read_to_string(root().join("GameRecordView.qml")).unwrap();
        let chars = code_only(&record);
        let segmented = blocks_of(&chars, "UiSegmented");
        assert_eq!(segmented.len(), 1);
        assert_eq!(value_of(&segmented[0].own, "visible").as_deref(), Some("view.compared"));
    }

    #[test]
    fn the_meters_heads_say_how_the_rows_are_ordered() {
        // Details' heads sort; the meter's do not, so their tips say why.
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        assert!(text.contains("text: heads.firstTip + \"\\n\" + meter.orderTip"));
        assert!(text.contains("text: head.modelData.tip + \"\\n\" + meter.orderTip"));
        assert!(crate::i18n::text("en", "meter.columnTips.order").contains("{lowest}"));
    }

    #[test]
    fn no_string_points_players_to_a_file_they_do_not_have() {
        // The capture notice named docs/linux.md, which no package ships.
        for language in crate::i18n::LANGUAGES {
            assert!(!crate::i18n::text(language, "connection.captureUnavailable").contains("docs/"), "{language}");
        }
    }

    #[test]
    fn every_source_file_names_its_license() {
        let dirs = [("src", "rs"), ("cpp", "cpp"), ("cpp", "h"), (".", "qml"), ("tests", "qml")];
        for (dir, ext) in dirs {
            for file in files(&root().join(dir), ext) {
                let text = std::fs::read_to_string(&file).unwrap();
                assert!(text.starts_with("// SPDX-License-Identifier: AGPL-3.0-only\n"), "{}", file.display());
            }
        }
    }

    #[test]
    fn log_lines_start_with_the_installed_programs_name() {
        // The binary is built as daevalog-qml and installed as daevalog-dps-meter.
        for (dir, ext) in [("src", "rs"), ("cpp", "cpp")] {
            for file in files(&root().join(dir), ext) {
                let text = std::fs::read_to_string(&file).unwrap();
                assert!(!text.contains(&["daevalog-qml", ": "].concat()), "{}", file.display());
            }
        }
    }

    #[test]
    fn the_health_text_follows_its_setting() {
        // 1.x's two looks, as a setting; Centred is the default
        // and the first choice.
        assert_eq!(crate::settings_store::Values::default().health_text, "centred");
        let settings = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let centred = settings.find("\"settings.healthText.centred\"").expect("the Centred choice");
        let split = settings.find("\"settings.healthText.split\"").expect("the Split choice");
        assert!(centred < split, "Centred first");
        assert!(settings.contains("onPicked: value => Settings.healthText = value"));
        // The bar: one text for Centred, three for Split, picked by the setting.
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        assert!(text.contains("readonly property bool centred: Settings.healthText === \"centred\""));
        let chars = code_only(&text);
        let shown: Vec<String> = blocks_of(&chars, "HudText").iter().filter_map(|b| value_of(&b.own, "visible")).collect();
        assert_eq!(shown.iter().filter(|v| *v == "healthBar.centred").count(), 1);
        assert_eq!(shown.iter().filter(|v| *v == "!healthBar.centred").count(), 2);
        assert!(text.contains("text: healthBar.percent + \"  \" + Meter.bossHpText\n"));
    }

    #[test]
    fn slim_shows_a_thin_health_bar_without_text() {
        // Slim had no health bar at all.
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        assert!(text.contains("readonly property bool hasBoss: Meter.bossHealth >= 0\n"));
        let sizes = std::fs::read_to_string(root().join("MeterSizes.qml")).unwrap();
        assert!(sizes.contains("readonly property int bossBand: px(slim ? 3 : 16)\n"));
        assert!(text.contains("+ (hasBoss ? Theme.meter.bossBand : 0) +"), "the meter's height");
        let chars = code_only(&text);
        let bars: Vec<Block> = blocks_of(&chars, "Rectangle").into_iter().filter(|b| value_of(&b.own, "id").as_deref() == Some("healthBar")).collect();
        assert_eq!(bars.len(), 1);
        let bar = &bars[0];
        assert_eq!(value_of(&bar.own, "visible").as_deref(), Some("meter.hasBoss"));
        assert_eq!(value_of(&bar.own, "Layout.preferredHeight").as_deref(), Some("Theme.meter.bossBand"));
        assert_eq!(value_of(&bar.own, "color").as_deref(), Some("Theme.bossTrack"));
        // Every text on the bar sits in the one part slim hides.
        let inside = |b: &Block, outer: &Block| b.open > outer.open && b.close < outer.close;
        let hidden: Vec<Block> = blocks_of(&chars, "Item")
            .into_iter()
            .filter(|b| inside(b, bar) && value_of(&b.own, "visible").as_deref() == Some("!Theme.slim"))
            .collect();
        assert_eq!(hidden.len(), 1);
        let texts: Vec<Block> = blocks_of(&chars, "HudText").into_iter().filter(|b| inside(b, bar)).collect();
        assert_eq!(texts.len(), 3);
        assert!(texts.iter().all(|t| inside(t, &hidden[0])));
    }

    #[test]
    fn the_time_to_kill_sits_after_the_enrage_countdown_with_its_mark() {
        // In the header, as it was too small on the health bar; a tick or a
        // cross, not a second number, after the enrage countdown, not beside
        // the fight time.
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        let fight = text.find("text: Meter.fightTime").expect("the fight time");
        let name = text.find("I18n.targetName(Meter.mode, Meter.target)").expect("the boss name");
        let countdown = text.find("text: Meter.enrage\n").expect("the countdown");
        let eta = text.find("text: Meter.bossEta + (Meter.killMark !== \"\" ? \" \" + Meter.killMark : \"\")").expect("the time to kill");
        assert!(fight < name && name < countdown && countdown < eta);
        assert_eq!(text[countdown..eta].matches("HudText {").count(), 1, "right after the countdown");
        assert!(text.contains("visible: Meter.bossEta !== \"\" && !Capture.suspended"));
        assert!(text.contains("color: Meter.killMark !== \"\" ? Theme.pingColors[Meter.killMarkLevel] : Theme.number"));
        let tip = text.find("text: I18n.tr(\"meter.timeToKillTip\")").expect("its tooltip");
        assert!(eta < tip && !text[eta..tip].contains("HudText {"), "its tooltip");
        // The time to kill is no longer on the health bar.
        assert!(text.contains("Layout.maximumWidth: Math.ceil(implicitWidth)"), "the name whole where it fits");
        assert_eq!(text.matches("Meter.bossEta").count(), 2, "the item and its visibility");
    }

    #[test]
    fn the_open_notice_list_reads_at_row_size() {
        // At 11 px the list was too small to read on a dense screen; the
        // bar's text and the empty line are the row size too, the meter's,
        // so they grow with the Meter size setting.
        let text = std::fs::read_to_string(root().join("NoticeTray.qml")).unwrap();
        let chars = code_only(&text);
        let list = blocks_of(&chars, "Repeater").into_iter().find(|b| b.own.contains("id: rows")).expect("the list");
        let inside = |b: &Block| b.at > list.open && b.at < list.close;
        let texts = blocks_of(&chars, "HudText");
        let (rows, others): (Vec<&Block>, Vec<&Block>) = texts.iter().partition(|b| inside(b));
        assert_eq!(rows.len(), 3, "title, text and time");
        for b in rows {
            assert_eq!(value_of(&b.own, "font.pixelSize").as_deref(), Some("Theme.meter.sizeRow"), "line {}", line_of(&chars, b.open));
        }
        for b in others {
            assert_eq!(value_of(&b.own, "font.pixelSize").as_deref(), Some("Theme.meter.sizeRow"), "line {}", line_of(&chars, b.open));
        }
        let icons: Vec<Block> = blocks_of(&chars, "NoticeIcon").into_iter().filter(inside).collect();
        assert_eq!(icons.len(), 1);
        let mark: String = chars[icons[0].open..icons[0].close].iter().collect();
        assert!(mark.contains("width: Theme.meter.px(28)\n") && mark.contains("height: Theme.meter.px(28)\n"), "{mark}");
        assert!(icons[0].own.contains("anchors.verticalCenter: parent.verticalCenter"), "centred on the notice");
        assert!(text.contains("height: Math.max(tray.rowMin, words.implicitHeight + Theme.meter.px(10))"));
        assert!(text.contains("readonly property int rowMin: Theme.meter.px(38)\n"));
        assert!(text.contains(
            "readonly property int listMax: (1 + countdownLines + (maxRows - 1) * (1 + textLines)) * Math.ceil(rowFont.height)\n        + maxRows * Theme.meter.px(10) + moreHeight"
        ));
        // The same line counts cut the rows' text.
        assert!(text.contains("maximumLineCount: line.modelData.until > 0 ? tray.countdownLines : tray.textLines"));
        assert!(text.contains("readonly property int countdownLines: 6\n    readonly property int textLines: 3\n"));
        assert!(text.contains("font.pixelSize: Theme.meter.sizeRow"), "the row font, at the meter's size");
        // The bar's icons keep their own size.
        let icon = std::fs::read_to_string(root().join("NoticeIcon.qml")).unwrap();
        assert!(icon.contains("implicitWidth: 14\n") && icon.contains("implicitHeight: 14\n"));
    }

    #[test]
    fn the_open_notice_list_shows_whole_notices() {
        // At a 260 px cap the fourth pet notice was cut in half at the
        // meter's edge, and locked it could not be scrolled to. The card
        // shows at most three whole notices; a line pages on.
        let text = std::fs::read_to_string(root().join("NoticeTray.qml")).unwrap();
        assert!(text.contains("readonly property int maxRows: 3"));
        assert!(text.contains("readonly property var page: items.slice(first, first + maxRows)"));
        assert!(text.contains("visible: index < tray.shown"));
        assert!(text.contains("if (first > 0 || first + n < items.length)\n            n = fitting(listRoom - moreHeight)"));
        assert!(text.contains("onTapped: tray.showFrom(tray.later > 0 ? tray.first + tray.shown : 0)"));
        // The first page stays on the newest, also on a locked meter; a later
        // page keeps its top notice.
        assert!(text.contains("const at = first > 0 ? items.findIndex(n => n.id === firstId) : 0"));
        assert!(text.contains("onOpenChanged: showFrom(0)"));
        assert!(text.contains("onLockedChanged: {\n        shapeInput()\n        showFrom(0)\n    }"));
    }

    #[test]
    fn the_low_share_colours_follow_their_setting() {
        // Row is the default: 1.x always recoloured the bar.
        assert_eq!(crate::settings_store::Values::default().low_share, "row");
        let settings = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let at = |key: &str| settings.find(&format!("\"settings.lowShare.{key}\"")).expect(key);
        assert!(at("off") < at("number") && at("number") < at("row"), "Off, Number, Row");
        assert!(settings.contains("onPicked: value => Settings.lowShare = value"));
        let row = std::fs::read_to_string(root().join("MeterRow.qml")).unwrap();
        assert!(row.contains("readonly property string lowLook: lowShare > 0 ? Settings.lowShare : \"off\""));
        for look in ["row.lowLook === \"row\"", "lowLook === \"row\"", "row.lowLook === \"number\""] {
            assert!(row.contains(look), "{look}");
        }
        // The DPS rows mark it by 1.x's thresholds.
        let meter = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        assert!(meter.contains("lowShare: shareValue < 3 ? 2 : shareValue < 5 ? 1 : 0"));
        // Number, with % switched off: the first number, not nothing.
        assert!(meter.contains("shareCell: Math.max(0, meter.tabKeys.indexOf(\"pct\"))"));
    }

    #[test]
    fn the_group_damage_follows_its_setting() {
        // The footer is the default, as 1.x's Beta layout.
        assert_eq!(crate::settings_store::Values::default().group_damage, "footer");
        let settings = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let footer = settings.find("\"settings.groupDamage.footer\"").expect("Footer");
        let line = settings.find("\"settings.groupDamage.line\"").expect("Own line");
        assert!(footer < line, "Footer first");
        assert!(settings.contains("onPicked: value => Settings.groupDamage = value"));
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        assert!(text.contains("readonly property bool damageLine: showsDamage && Settings.groupDamage === \"line\""));
        let chars = code_only(&text);
        let shown: Vec<String> = blocks_of(&chars, "SmallText").iter().filter_map(|b| value_of(&b.own, "visible")).collect();
        // In the footer: the dot and the damage; the rate there, unless the
        // own line has it.
        assert_eq!(shown.iter().filter(|v| *v == "meter.showsDamage && !meter.damageLine").count(), 2);
        assert_eq!(shown.iter().filter(|v| *v == "Settings.showTotal && !meter.damageLine").count(), 1);
        assert!(text.contains("visible: meter.damageLine"), "the own line");
    }

    #[test]
    fn the_tray_edge_follows_its_setting() {
        // Auto is the default: on top, or at the bottom when the open list
        // has no room above.
        assert_eq!(crate::settings_store::Values::default().tray_edge, "auto");
        let settings = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let auto = settings.find("\"settings.trayEdge.auto\"").expect("Auto");
        let top = settings.find("\"settings.trayEdge.top\"").expect("Top");
        let bottom = settings.find("\"settings.trayEdge.bottom\"").expect("Bottom");
        assert!(auto < top && top < bottom, "Auto, Top, Bottom");
        assert!(settings.contains("onPicked: value => Settings.trayEdge = value"));
    }

    #[test]
    fn the_meters_text_and_rows_follow_the_meter_size() {
        // The meter's text in the windows' type (Theme.sizeRow, HudText's
        // own default) would stay small at 150%. The notice tray grows with
        // the meter it sits on.
        for file in ["MeterWindow.qml", "MeterRow.qml", "NoticeTray.qml"] {
            let chars = code_only(&std::fs::read_to_string(root().join(file)).unwrap());
            let code: String = chars.iter().collect();
            assert!(!code.contains("Theme.size"), "{file}: the windows' type scale");
            for block in blocks_of(&chars, "HudText") {
                let size = value_of(&block.own, "font.pixelSize").unwrap_or_default();
                assert!(size.starts_with("Theme.meter."), "{file}:{} HudText font.pixelSize {size:?}", line_of(&chars, block.at));
            }
        }
        // The rows, bands and columns live in Theme.meter only.
        let theme = std::fs::read_to_string(root().join("Theme.qml")).unwrap();
        assert!(theme.contains("scale: Settings.meterSize / 100"));
        for size in ["rowHeight", "rowGap", "headerHeight", "headsHeight", "footerHeight", "iconSize", "colIcon", "colGap", "padLeft", "padRight"] {
            assert!(!theme.contains(&format!("property int {size}:")), "Theme.{size}");
            for path in files(&root(), "qml") {
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(!text.contains(&format!("Theme.{size}")), "{} Theme.{size}", path.display());
            }
        }
        // The setting, by the slim mode; only the meter and its tray have it.
        let settings = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let layout = settings.find("\"settings.meterLayout.label\"").expect("Meter layout");
        let size = settings.find("\"settings.meterSize.label\"").expect("Meter size");
        assert!(layout < size && !settings[layout..size].contains("SwitchRow"), "right after Meter layout");
        assert!(settings.contains("options: [100, 115, 130, 150].map(n => ({ \"label\": n + \"%\", \"value\": n }))"));
        assert!(settings.contains("onPicked: value => Settings.meterSize = value"));
        for path in files(&root(), "qml") {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let text = std::fs::read_to_string(&path).unwrap();
            if !["MeterWindow.qml", "MeterRow.qml", "NoticeTray.qml", "Theme.qml", "SettingsWindow.qml"].contains(&name.as_str()) {
                assert!(!text.contains("Theme.meter.") && !text.contains("Settings.meterSize"), "{name}");
            }
        }
    }

    #[test]
    fn no_window_reads_switches_from_the_command_line() {
        // Mock-up switches (bare arguments such as "mock-classic") picked
        // looks before they were settings; none may ship.
        for path in files(&root(), "qml") {
            let text = code_only(&std::fs::read_to_string(&path).unwrap()).iter().collect::<String>();
            assert!(!text.contains("Qt.application.arguments"), "{}", path.display());
        }
    }

    #[test]
    fn placing_details_on_a_monitor_shows_which_one() {
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        let place = main.find("function applyDetailsMonitor").unwrap();
        let end = main[place..].find("\n    }\n").unwrap();
        assert!(main[place..place + end].contains("window.showMonitorBadge(false)"), "the badge, faded after 3 s");
        let details = std::fs::read_to_string(root().join("DetailsWindow.qml")).unwrap();
        assert!(details.contains("function showMonitorBadge(hold)"));
        assert!(details.contains("I18n.tr(\"settings.detailsMonitor.badge\")"));
    }

    #[test]
    fn the_lock_key_is_the_one_key_on_linux_and_can_be_recorded_where_it_works() {
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let raw: Vec<char> = text.chars().collect();
        let chars = code_only(&text);
        let row = |key: &str| -> Block {
            let rows: Vec<Block> = blocks_of(&chars, "SettingRow")
                .into_iter()
                .filter(|b| raw[b.open..b.close].iter().collect::<String>().contains(&format!("\"settings.keybind.{key}\"")))
                .collect();
            assert_eq!(rows.len(), 1, "one row for {key}");
            rows.into_iter().next().unwrap()
        };
        let row_text = |b: &Block| raw[b.open..b.close].iter().collect::<String>();
        for key in ["reload", "toggle"] {
            let hidden = "visible: Qt.platform.os !== \"linux\" && Qt.platform.os !== \"windows\"";
            assert!(row_text(&row(key)).contains(hidden), "{key}: not on Linux or Windows");
        }
        let lock = row("lock");
        assert_eq!(value_of(&lock.own, "visible"), None, "the lock key shows everywhere");
        let lock_text = row_text(&lock);
        // The key the desktop gave; nothing to record without the portal.
        assert!(lock_text.contains("keys: Hotkey.state === \"bound\" && Hotkey.trigger !== \"\" ? Hotkey.trigger : Settings.lockHotkey"));
        assert!(lock_text.contains("recordable: Hotkey.state !== \"missing\""));
        assert!(lock_text.contains("Settings.lockHotkey = label") && lock_text.contains("Hotkey.offer(label)"), "a new key goes to the desktop");
        // Every state the portal thread reports has its note, in English at least.
        let english = std::fs::read_to_string(root().join("../src/data/i18n/ui/en.json")).unwrap();
        for (state, note) in [("missing", "portalMissing"), ("waiting", "portalWaiting"), ("bound", "portalBound"),
            ("unbound", "portalUnbound"), ("refused", "portalRefused"), ("failed", "portalFailed")] {
            assert!(text.contains(&format!("case \"{state}\": return I18n.tr(\"settings.keybind.{note}\")")), "{state}");
            assert!(english.contains(&format!("\"{note}\":")), "{note}");
        }
        // And every state of the Windows key (hotkey.rs).
        for (state, note) in [("bound", "windowsBound"), ("taken", "windowsTaken"), ("unbound", "windowsUnbound")] {
            assert!(text.contains(&format!("case \"{state}\": return I18n.tr(\"settings.keybind.{note}\")")), "{state}");
            assert!(english.contains(&format!("\"{note}\":")), "{note}");
        }
        // The key locks the meter, and counts as a way back from the lock.
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        assert!(main.contains("target: Hotkey\n        function onPressed() {\n            Settings.locked = !Settings.locked"));
        assert!(main.contains("|| Hotkey.state === \"bound\""));
        assert!(main.contains("Hotkey.start(Settings.lockHotkey)"));
    }

    #[test]
    fn details_history_settings_and_character_keep_their_place() {
        for (file, name) in [("DetailsWindow.qml", "details"), ("HistoryWindow.qml", "history"), ("SettingsWindow.qml", "settings"), ("CharacterWindow.qml", "character")] {
            let text = std::fs::read_to_string(root().join(file)).unwrap();
            assert!(text.contains(&format!("placeName: \"{name}\"")), "{file}");
        }
        let window = std::fs::read_to_string(root().join("AppWindow.qml")).unwrap();
        let present = window.find("function present").unwrap();
        assert!(window[present..].find("restorePlace()").unwrap() < window[present..].find("show()").unwrap(), "placed before it shows");
        // Show Details on monitor places it for the setting: not kept.
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        assert!(main.contains("details.keepPlace = Qt.binding(() => !app.monitorDetails)"));
    }

    #[test]
    fn the_details_skill_table_sorts_and_opens_damage_over_time() {
        let text = std::fs::read_to_string(root().join("DetailsWindow.qml")).unwrap();
        let chars = code_only(&text);
        let table = blocks_of(&chars, "UiTable").into_iter().find(|b| b.own.contains("id: skillTable")).expect("the skill table");
        assert_eq!(value_of(&table.own, "rows").as_deref(), Some("details.shownRows"));
        assert_eq!(value_of(&table.own, "sortable").as_deref(), Some("true"));
        let own: String = text.chars().skip(table.open).take(table.close - table.open).collect();
        assert!(own.contains("onHeadClicked: key => details.sortBy(key)"));
        assert!(own.contains("onFoldClicked: (index, row) => details.fold(row.name)"));
        // The chosen skill and its hits come from the rows as shown.
        assert!(text.contains("shownRows.findIndex(r => rowKey(r) === selectedKey)"));
        assert!(text.contains("readonly property var skill: shownRows[selectedSkill] || null"));
    }

    #[test]
    fn the_details_rows_are_read_by_their_own_table() {
        // A change of mode or player reaches each binding in turn, so one
        // that read the mode beside the rows saw the new table with the old
        // rows: a saved fight's mechanics view logged errors.
        let text = std::fs::read_to_string(root().join("DetailsWindow.qml")).unwrap();
        assert!(text.contains("source.table, sort, tableColumns.map(c => c.key)"), "the rows arranged for the table they are");
        assert!(text.contains("if (row.table === \"taken\" || row.table === \"mechanics\")"), "cells by the row's table");
        assert!(text.contains("details.skill !== null && details.skill.table === \"dmg\""), "hit types of a damage row only");
        assert!(text.contains("details.skill && details.skill.table === \"mechanics\" ? details.skill.hit.map("), "whom a mechanic hit");
        assert!(!text.contains("allPlayers: details.allPlayers"));
        let chart = std::fs::read_to_string(root().join("DpsChart.qml")).unwrap();
        assert!(chart.contains("readonly property bool allPlayers: player === null"), "the legend by its player alone");
    }

    #[test]
    fn the_enrage_countdown_sits_beside_the_boss_name_in_the_health_colours() {
        let text = std::fs::read_to_string(root().join("MeterWindow.qml")).unwrap();
        let name = text.find("I18n.targetName(Meter.mode, Meter.target)").expect("the boss name");
        let countdown = text.find("text: Meter.enrage\n").expect("the countdown");
        assert!(name < countdown && text[name..countdown].matches("HudText {").count() == 1, "right after the name");
        assert!(text.contains("visible: Meter.enrage !== \"\" && !Capture.suspended"));
        assert!(text.contains("color: Theme.pingColors[Meter.enrageLevel]"));
        // The health bar's colours by the same steps.
        assert!(text.contains("color: Theme.pingColors[healthBar.share > 0.5 ? 1 : healthBar.share >= 0.25 ? 2 : 3]"));
    }

    #[test]
    fn every_share_has_one_decimal_under_10_percent() {
        // As snapshot.rs `share`, which the meter's rows take.
        assert_eq!((crate::snapshot::share(4.66).as_str(), crate::snapshot::share(43.6).as_str()), ("4.7%", "44%"));
        let format = std::fs::read_to_string(root().join("Format.qml")).unwrap();
        assert!(format.contains("return tenths < 10 ? tenths.toFixed(1) + \"%\" : v.toFixed(0) + \"%\""));
        // Shares in the windows go through it, not whole percent.
        let history = std::fs::read_to_string(root().join("HistoryWindow.qml")).unwrap();
        assert!(history.contains("\"share\": history.noData.tankRow(r, Format.share(r.share))"));
        let card = std::fs::read_to_string(root().join("SkillCard.qml")).unwrap();
        assert!(card.contains("Format.share(skill.modelData.damage / card.view.total * 100)"));
        for path in files(&root(), "qml") {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(!text.contains("Format.pct(r.share)"), "{}", path.display());
        }
    }

    #[test]
    fn a_saved_device_that_is_missing_or_idle_says_so() {
        let settings = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let start = settings.find("label: I18n.tr(\"settings.device.label\")").expect("the device row");
        let row = &settings[start..start + settings[start..].find("\n                    }\n").unwrap()];
        // Missing: the pick stays in the list, marked.
        assert!(row.contains("I18n.tr(\"settings.device.notFound\").replace(\"{device}\", Capture.device)"));
        assert!(row.contains("Capture.device !== \"\" && !listed.includes(Capture.device)"));
        // Idle: the hint names the device the meter uses.
        assert!(row.contains("hint: Capture.following !== \"\""));
        assert!(row.contains("I18n.tr(\"settings.device.following\").replace(\"{found}\", Capture.following).replace(\"{device}\", Capture.device)"));
        assert!(row.contains("onPicked: value => Capture.device = value"));
        // The list is read again while the page shows.
        assert!(settings.contains("onTriggered: Capture.loadDevices()"));
        assert_eq!(crate::i18n::text("en", "settings.device.notFound"), "{device} (not found)");
    }

    #[test]
    fn the_wayland_layer_switch_shows_only_where_it_can_act() {
        // It showed on X11, where it does nothing.
        let text = std::fs::read_to_string(root().join("SettingsWindow.qml")).unwrap();
        let raw: Vec<char> = text.chars().collect();
        let chars = code_only(&text);
        let rows: Vec<Block> = blocks_of(&chars, "SwitchRow")
            .into_iter()
            .filter(|b| raw[b.open..b.close].iter().collect::<String>().contains("\"settings.waylandLayer\""))
            .collect();
        assert_eq!(rows.len(), 1, "one row for the setting");
        assert_eq!(value_of(&rows[0].own, "visible").as_deref(), Some("Overlay.layerSetting"));
    }

    #[test]
    fn history_pages_its_list_and_folds_its_groups() {
        let text = std::fs::read_to_string(root().join("HistoryWindow.qml")).unwrap();
        assert!(text.contains("model: history.shownList.groups"));
        assert!(text.contains("onClicked: pages.more()"));
        assert!(text.contains("onTapped: pages.toggle(group.modelData.title)"));
        assert!(text.contains("turn: group.modelData.folded ? -90 : 0"));
        // Every filter, and closing the window, starts over.
        for filter in ["Boss", "Job", "Date", "OnlyRecords", "ShowTraining", "Layout"] {
            assert!(text.contains(&format!("function on{filter}Changed() {{\n            pages.reset()")), "{filter}");
        }
        assert!(text.contains("History.onlyRecords = false\n        pages.reset()"));
    }

    #[test]
    fn the_tray_takes_the_edge_with_room() {
        // With the meter near the top of the screen, the open card went off
        // the screen (X11) or over the meter (Wayland). Auto picks the
        // bottom edge where the largest card has no room above.
        let text = std::fs::read_to_string(root().join("NoticeTray.qml")).unwrap();
        assert!(text.contains("readonly property string edge: Settings.trayEdge"));
        assert!(text.contains("readonly property int fullHeight: barHeight + listMax + Theme.meter.px(4)"));
        assert!(text.contains("const wantsBelow = edge === \"bottom\"\n            || (edge === \"auto\" && (room ? room.above < fullHeight && room.below > room.above : !open && below))"));
        // The list is no taller than the room on its side.
        assert!(text.contains("height: Math.min(column.implicitHeight, tray.listRoom)"));
        assert!(text.contains("listRoom = room ? Math.max(rowMin, (below ? room.below : room.above) - barHeight - Theme.meter.px(4)) : noLimit"));
        assert!(text.contains("y = wantsBelow ? at.y : at.y - h"));
        // The bar on the card's far edge, the chevron away from the meter
        // to open.
        assert!(text.contains("y: tray.below ? tray.height - height : 0"));
        assert!(text.contains("onYChanged: tray.shapeInput()"), "locked, the chevron takes clicks where it is");
        assert!(text.contains("turn: tray.open !== tray.below ? 0 : 180"));
        // Wayland: exactly where asked when the room is known (Hyprland slid
        // it over the header); the compositor flips it when not.
        assert!(text.contains("Overlay.placePopup(tray, Qt.rect(inset, 0, w, meter.height), wantsBelow, !room, flip)"));
        assert!(text.contains("const flip = !room && edge === \"auto\""));
        // It follows the bottom edge and a layer surface's margins.
        assert!(text.contains("function onHeightChanged() { tray.placeSoon() }"));
        assert!(text.contains("function onLayerMarginsChanged() {\n            tray.placeSoon()"));
        assert!(text.contains("onEdgeChanged: placeSoon()"));
        // Hyprland leaves a still popup part of the way when the meter
        // moves: shown again once the move has settled.
        assert!(text.contains("if (tray.meter && tray.meter.layerSurface && Overlay.layerPopupsLag && tray.visible)\n                moveSettled.restart()"));
        assert!(text.contains("            tray.visible = false\n            tray.place()"));
    }

    #[test]
    fn the_notice_bar_reads_at_row_size() {
        // Its 14 px icons and 11 px text were too small to read.
        let text = std::fs::read_to_string(root().join("NoticeTray.qml")).unwrap();
        let chars = code_only(&text);
        let code: String = chars.iter().collect();
        assert!(code.contains("readonly property int barHeight: Theme.meter.px(24)"));
        let bar = blocks_of(&chars, "Item").into_iter().find(|b| b.own.contains("id: bar")).expect("the bar");
        assert_eq!(value_of(&bar.own, "height").as_deref(), Some("tray.barHeight"));
        let inside = |b: &Block| b.open > bar.open && b.close < bar.close;
        let icons: Vec<Block> = blocks_of(&chars, "NoticeIcon").into_iter().filter(inside).collect();
        assert_eq!(icons.len(), 1, "one icon per kind");
        let mark: String = chars[icons[0].open..icons[0].close].iter().collect();
        assert!(mark.contains("width: Theme.meter.px(18)\n") && mark.contains("height: Theme.meter.px(18)\n"), "{mark}");
        let texts: Vec<Block> = blocks_of(&chars, "HudText").into_iter().filter(inside).collect();
        assert_eq!(texts.len(), 3, "the count, the title and Clear all");
        for t in &texts {
            assert_eq!(value_of(&t.own, "font.pixelSize").as_deref(), Some("Theme.meter.sizeRow"), "line {}", line_of(&chars, t.at));
        }
        // The card, the line under the bar and the list start from the bar's height.
        assert!(code.contains("readonly property int wantedHeight: barHeight + (open ? listHeight + Theme.meter.px(4) : 0)"));
        assert!(code.contains("    height: barHeight\n"));
        let starts: Vec<String> = ["Rectangle", "HudText", "Item"]
            .iter()
            .flat_map(|c| blocks_of(&chars, c))
            .filter(|b| !inside(b) && b.open > bar.close)
            .filter_map(|b| value_of(&b.own, "y"))
            .filter(|y| y.contains("barHeight"))
            .collect();
        assert_eq!(
            starts,
            vec![
                "tray.below ? bar.y - 1 : tray.barHeight",
                "tray.below ? Theme.meter.px : tray.barHeight + Theme.meter.px",
                "tray.below ? Theme.meter.px : tray.barHeight + Theme.meter.px"
            ]
        );
        assert!(code.contains("y: tray.below ? Theme.meter.px(8) : tray.barHeight + Theme.meter.px(8)\n"));
        assert!(code.contains("y: tray.below ? Theme.meter.px(2) : tray.barHeight + Theme.meter.px(2)\n"));
        // Other places size the icon themselves.
        let icon = std::fs::read_to_string(root().join("NoticeIcon.qml")).unwrap();
        assert!(icon.contains("implicitWidth: 14\n    implicitHeight: 14"));
    }

    /// The notice marks' colours by theme, from Theme.qml: (theme, window,
    /// [(colour name, value)]). System is checked on Breeze Dark, Plasma's
    /// own dark scheme (its Window BackgroundNormal, 32,35,38), with the
    /// default theme's marks it takes.
    fn mark_colours() -> Vec<(&'static str, String, Vec<(&'static str, String)>)> {
        let text = std::fs::read_to_string(root().join("Theme.qml")).unwrap();
        let block = |start: &str, end: &str| -> String {
            let from = text.find(start).unwrap_or_else(|| panic!("{start}"));
            let to = from + text[from..].find(end).unwrap_or_else(|| panic!("{end}"));
            text[from..to].to_string()
        };
        let hex = |block: &str, name: &str| -> String {
            let key = format!("\"{name}\": \"#");
            let at = block.find(&key).unwrap_or_else(|| panic!("{name}")) + key.len();
            block[at..at + 6].to_string()
        };
        let themes = [
            ("daevalog", block("\"daevalog\": {", "\"classic\": {")),
            ("classic", block("\"classic\": {", "\"nord\": {")),
            ("nord", block("\"nord\": {", "readonly property var base")),
        ];
        let mut marks: Vec<_> = themes
            .into_iter()
            .map(|(theme, b)| (theme, hex(&b, "window"), MARK_COLOURS.iter().map(|&name| (name, hex(&b, name))).collect::<Vec<_>>()))
            .collect();
        let default = marks[0].2.clone();
        marks.push(("system on Breeze Dark", "202326".to_string(), default));
        marks
    }

    const MARK_COLOURS: [&str; 4] = ["abyssBoss", "rift", "gartua", "commander"];

    /// WCAG contrast of two "rrggbb" colours.
    fn contrast(a: &str, b: &str) -> f64 {
        let luminance = |hex: &str| {
            let channel = |at: usize| {
                let c = u8::from_str_radix(&hex[at..at + 2], 16).unwrap() as f64 / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4)
        };
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn each_boss_tier_and_the_rift_has_its_own_mark_colour() {
        // Abyss Bosses red; the rift purple, as its map icon; field bosses
        // by tier: Gartua emerald, the commanders yellow; the rest keep the
        // ink. Only Gartua and the commanders get a countdown, so no other
        // tier has a colour.
        let icon = std::fs::read_to_string(root().join("NoticeIcon.qml")).unwrap();
        for (kind, colour) in [
            ("abyss", "Theme.abyssBoss"),
            ("gartua", "Theme.gartua"),
            ("commander", "Theme.commander"),
            ("rift", "Theme.rift"),
            ("warning", "Theme.warning"),
        ] {
            assert!(icon.contains(&format!("\"{kind}\": {colour}")), "{kind}");
        }
        assert!(icon.contains("readonly property color ink: inks[kind] || Theme.hudSub"));
        assert!(icon.contains(r#"readonly property string shape: ["abyss", "gartua", "commander"].includes(kind) ? "boss" : kind"#));
        let theme = std::fs::read_to_string(root().join("Theme.qml")).unwrap();
        for name in MARK_COLOURS {
            assert!(theme.contains(&format!("readonly property color {name}: base.{name}\n")), "{name}");
            assert!(theme.contains(&format!("\"{name}\": themes.daevalog.{name},")), "System takes the default's {name}");
        }
        for (theme, _, colours) in mark_colours() {
            let values: std::collections::BTreeSet<&String> = colours.iter().map(|(_, v)| v).collect();
            assert_eq!(values.len(), MARK_COLOURS.len(), "{theme}: each mark its own colour");
        }
    }

    #[test]
    fn the_notice_marks_read_on_the_panel_over_a_mid_grey_scene() {
        // A mark needs 3:1 (WCAG's figure for icons) on the panel: the
        // window colour at the default opacity over a mid grey game scene.
        let opacity = crate::settings_store::Values::default().window_opacity as f64 / 100.0;
        let mut weak = Vec::new();
        for (theme, window, colours) in mark_colours() {
            let panel: String = (0..3)
                .map(|i| {
                    let w = u8::from_str_radix(&window[2 * i..2 * i + 2], 16).unwrap() as f64;
                    format!("{:02x}", (w * opacity + 128.0 * (1.0 - opacity)).round() as u8)
                })
                .collect();
            for (name, value) in colours {
                let ratio = contrast(&value, &panel);
                if ratio < 3.0 {
                    weak.push(format!("{theme} {name} #{value}: {ratio:.2}:1"));
                }
            }
        }
        assert!(weak.is_empty(), "{}", weak.join("\n"));
        assert!((contrast("FF5C5C", "161925") - 5.78).abs() < 0.01, "the ratio as Theme.qml's notes give it");
    }

    #[test]
    fn the_class_filter_shows_each_class_icon() {
        let history = std::fs::read_to_string(root().join("HistoryWindow.qml")).unwrap();
        assert!(history.contains("\"icon\": Theme.classIcon(j.icon)"));
        let dropdown = std::fs::read_to_string(root().join("UiDropdown.qml")).unwrap();
        assert!(dropdown.contains("\"icon\": option.icon || \"\""));
        // Only a menu with an icon makes room for one.
        let menu = std::fs::read_to_string(root().join("UiMenu.qml")).unwrap();
        assert!(menu.contains("readonly property int iconGutter: hasIcons ? 22 : 0"));
        assert!(menu.contains("x: 10 + menu.gutter + menu.iconGutter"));
    }

    #[test]
    fn the_character_window_only_shows_and_opens_beside_history() {
        // Read only: the tabs and the character list are its only controls.
        for path in files(&root(), "qml").into_iter().filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("Character")) {
            let chars = code_only(&std::fs::read_to_string(&path).unwrap());
            for control in ["UiButton", "UiSwitch", "UiSlider", "UiField", "UiMenuItem"] {
                assert!(blocks_of(&chars, control).is_empty(), "{} has a {control}", path.display());
            }
        }
        let main = std::fs::read_to_string(root().join("Main.qml")).unwrap();
        let history = main.find("text: I18n.tr(\"tray.history\")").expect("History in the meter's menu");
        let character = main.find("text: I18n.tr(\"character.title\")").expect("Character in the meter's menu");
        let settings = main.find("text: I18n.tr(\"tray.settings\")").unwrap();
        assert!(history < character && character < settings, "right after History");
        assert!(main.contains("onTriggered: app.openCharacter()"));
    }

    #[test]
    fn every_character_page_says_when_the_game_has_not_sent_it() {
        for page in ["CharacterGear.qml", "CharacterStats.qml", "CharacterSkills.qml", "CharacterBoards.qml"] {
            let text = std::fs::read_to_string(root().join(page)).unwrap();
            assert!(text.contains("I18n.tr(\"character.notRead\")"), "{page}");
        }
    }

    #[test]
    fn the_character_window_follows_new_reads_and_the_language() {
        let text = std::fs::read_to_string(root().join("CharacterWindow.qml")).unwrap();
        assert!(text.contains("running: window.visible\n        onTriggered: Character.refresh()"), "looks again while open");
        assert!(text.contains("function onLanguageChanged() {\n            Character.redraw()"));
        assert!(text.contains("function onHour24Changed() {\n            Character.redraw()"));
        // Several characters: a list to pick from; one: no list.
        assert!(text.contains("visible: window.files.length > 1"));
    }

    /// A page taller than the window shows that it scrolls: every page is
    /// a Flickable the window loads, and the window's upright scroll bar
    /// follows whichever page is open.
    #[test]
    fn every_character_page_shows_that_it_scrolls() {
        for page in ["CharacterGear.qml", "CharacterStats.qml", "CharacterSkills.qml", "CharacterBoards.qml"] {
            let chars = code_only(&std::fs::read_to_string(root().join(page)).unwrap());
            let text: String = chars.iter().collect();
            assert!(text.trim_start().starts_with("pragma") && text.contains("\nFlickable {"), "{page} is a Flickable");
            let name = page.trim_end_matches(".qml");
            let window = std::fs::read_to_string(root().join("CharacterWindow.qml")).unwrap();
            assert!(window.contains(&format!("        {name} {{\n            view: window.view")), "{page} is one of the window's pages");
        }
        let chars = code_only(&std::fs::read_to_string(root().join("CharacterWindow.qml")).unwrap());
        let loader = blocks_of(&chars, "Loader");
        assert!(loader.len() == 1 && loader[0].own.contains("id: pages"), "the pages' loader");
        let bars = blocks_of(&chars, "UiScrollBar");
        assert_eq!(bars.len(), 1, "one scroll bar for the pages");
        let bar: String = chars[bars[0].open..bars[0].close].iter().collect();
        for line in [
            "page: pages.item as Flickable",
            "vertical: true",
            "anchors.top: pages.top",
            "anchors.bottom: pages.bottom",
            "anchors.right: parent.right",
            "position: page ? page.contentY : 0",
            "range: page ? Math.max(0, page.contentHeight - page.height) : 0",
            "view: page ? page.height : 1",
            "onMoved: p => page.contentY = p",
        ] {
            assert!(bar.contains(line), "the scroll bar: {line}");
        }
        let scroll_bar = std::fs::read_to_string(root().join("UiScrollBar.qml")).unwrap();
        assert!(scroll_bar.contains("property bool vertical: false") && scroll_bar.contains("visible: range > 0"), "upright, only when there is something to scroll");
    }

    /// Every scroll bar's thumb (the Character pages, the skill tables, the
    /// timeline) is at least 3:1 on the window, the surface and its own
    /// track, in every theme; under System, on Breeze and Adwaita, light
    /// and dark, and on Qt's own palette.
    #[test]
    fn every_scroll_thumb_reads_at_3_to_1() {
        let theme = std::fs::read_to_string(root().join("Theme.qml")).unwrap();
        let over = |fg: [f64; 3], alpha: f64, bg: [f64; 3]| -> [f64; 3] { [0, 1, 2].map(|i| fg[i] * alpha + bg[i] * (1.0 - alpha)) };
        let lum = |c: [f64; 3]| -> f64 {
            let [r, g, b] = c.map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) });
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        let ratio = |a: [f64; 3], b: [f64; 3]| -> f64 {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        let alpha_of = |line: &str| -> f64 {
            let start = theme.find(line).unwrap_or_else(|| panic!("Theme.qml: {line}")) + line.len();
            theme[start..].split(')').next().unwrap().trim().parse().unwrap()
        };
        let track = alpha_of("readonly property color rowEmpty: tint(base.ink,");
        assert!(theme.contains("readonly property color scrollThumb: tint(base.ink, base.scrollThumb)"));
        // A theme's value: `"key": ...,` inside its block.
        let value = |block: &str, key: &str| -> String {
            let at = block.find(&format!("\"{key}\": ")).unwrap_or_else(|| panic!("{key}")) + key.len() + 4;
            block[at..].split([',', '\n']).next().unwrap().trim().to_string()
        };
        let check = |name: &str, window: [f64; 3], surface: [f64; 3], ink: [f64; 3], thumb: f64| {
            for (place, bg) in [("window", window), ("surface", surface)] {
                let under = over(ink, track, bg);
                let drawn = over(ink, thumb, under);
                for (against, colour) in [(place.to_string(), bg), (format!("track on the {place}"), under)] {
                    let r = ratio(drawn, colour);
                    assert!(r >= 3.0, "{name}: the scroll thumb is {r:.2}:1 on the {against}");
                }
            }
        };
        for name in ["daevalog", "classic", "nord"] {
            let start = theme.find(&format!("\"{name}\": {{")).unwrap_or_else(|| panic!("{name}"));
            let block = &theme[start..start + theme[start..].find("\n        }").unwrap()];
            let thumb: f64 = value(block, "scrollThumb").parse().unwrap();
            check(name, rgb(&value(block, "window")), rgb(&value(block, "surface")), rgb(&value(block, "ink")), thumb);
        }
        let system = &theme[theme.find("theme === \"system\" ? ({").unwrap()..];
        let thumb: f64 = value(&system[..system.find("})").unwrap()], "scrollThumb").parse().unwrap();
        for (name, window, view, text) in [
            ("System, Qt's palette", "#efefef", "#ffffff", "#000000"),
            ("System, Breeze", "#eff0f1", "#ffffff", "#232629"),
            ("System, Breeze Dark", "#202326", "#141618", "#fcfcfc"),
            ("System, Adwaita", "#fafafa", "#ffffff", "#2e3436"),
            ("System, Adwaita dark", "#242424", "#1e1e1e", "#ffffff"),
        ] {
            check(name, rgb(window), rgb(view), rgb(text), thumb);
        }
        // Every scroll bar draws its thumb in that colour.
        let bar = std::fs::read_to_string(root().join("UiScrollBar.qml")).unwrap();
        assert!(bar.contains("? Theme.sub : Theme.scrollThumb"), "the thumb at rest is Theme.scrollThumb");
        for file in ["CharacterWindow.qml", "UiTable.qml", "Timeline.qml"] {
            assert!(std::fs::read_to_string(root().join(file)).unwrap().contains("UiScrollBar {"), "{file}");
        }
    }

    /// Each of the game's item grades has a colour in both grade maps, in
    /// the hue the game gives that grade's text: Common grey, Rare green,
    /// Legend blue, Unique gold, Epic orange, Mythic red, Special cyan.
    #[test]
    fn item_grades_take_the_games_hues() {
        let theme = std::fs::read_to_string(root().join("Theme.qml")).unwrap();
        let map = |name: &str| -> HashMap<String, (f64, f64)> {
            let start = theme.find(&format!("property var {name}: ({{")).unwrap_or_else(|| panic!("{name}"));
            let block = &theme[start..start + theme[start..].find("})").unwrap()];
            block.lines().filter_map(|line| {
                let (key, value) = line.trim().trim_end_matches(',').split_once(": ")?;
                let [r, g, b] = value.trim_matches('"').starts_with('#').then(|| rgb(value))?;
                let (max, min) = (r.max(g).max(b), r.min(g).min(b));
                let d = max - min;
                let hue = if d == 0.0 { 0.0 } else if max == r { 60.0 * ((g - b) / d).rem_euclid(6.0) }
                    else if max == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) };
                Some((key.trim_matches('"').to_string(), (hue, d / max)))
            }).collect()
        };
        let games = [("Rare", 137.0), ("Legend", 211.0), ("Unique", 47.0), ("Epic", 25.0), ("Mythic", 355.0), ("Special", 171.0)];
        for name in ["gradeColors", "gradeColorsOnLight"] {
            let colours = map(name);
            assert_eq!(colours.len(), 7, "{name}: {colours:?}");
            assert!(colours["Common"].1 < 0.25, "{name}: Common is grey");
            for (grade, hue) in games {
                let (h, saturation) = colours[grade];
                let off = (h - hue).abs().min(360.0 - (h - hue).abs());
                assert!(off <= 20.0 && saturation > 0.4, "{name}: {grade} at hue {h:.0}, the game's {hue}");
            }
        }
    }

    #[test]
    fn the_checks_catch_what_they_are_for() {
        let dir = std::env::temp_dir().join(format!("qml-checks-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("T.qml"), r#"Item {
    UiMenuItem { text: "A {n}"; checked: true }
    UiMenuItem { text: "B"; onTriggered: Thing.go(1) }
    UiSwitch { checked: true; UiTip { onToggled: x() } }
    UiButton { text: "C" // no action: a label
    }
    function f() { Thing.go(1, 2); Thing.gone() }
    MeterRow { onClicked: open() }
    UiTable { barFill: r => r.fill; barStrength: 0.4 }
    UiTable { barFill: r => r.fill
        barStrength: Theme.tableBarStrength }
    UiTable { barStrength: 0.4 }
}
"#).unwrap();
        std::fs::write(dir.join("UiTable.qml"), "Column {\n    property real barStrength: Theme.barStrength\n}\n").unwrap();
        std::fs::write(dir.join("src/thing.rs"), "#[qinvokable]\nfn go(self: Pin<&mut Thing>, n: i32);\n").unwrap();
        let controls = controls_without_action(&dir);
        assert_eq!(controls, vec!["T.qml:2 UiMenuItem", "T.qml:4 UiSwitch"], "a nested handler is not the switch's");
        let calls = calls_with_wrong_arguments(&dir, &dir.join("src"));
        assert_eq!(calls, vec!["T.qml:7 Thing.go takes 1, given 2", "T.qml:7 Thing.gone is no function of Thing"]);
        assert_eq!(meter_rows_missing_an_action(&dir), vec!["T.qml:8 MeterRow onPointerOver"]);
        assert_eq!(tables_with_strong_bars(&dir), vec!["T.qml:9 UiTable barStrength", "UiTable.qml barStrength default"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
