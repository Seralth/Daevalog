// SPDX-License-Identifier: AGPL-3.0-only

use cxx_qt_build::{CxxQtBuilder, QResource, QResources, QmlFile, QmlModule};

fn main() {
    gear_glyphs();
    // The QML files sit next to the module's qmldir. In a subfolder, Qt 6.4
    // takes Theme from the folder's implicit import and loses the singleton.
    let module = QmlModule::new("Daevalog").qml_files([
        QmlFile::from("Main.qml"),
        QmlFile::from("MeterWindow.qml"),
        QmlFile::from("LayerMeterWindow.qml"),
        QmlFile::from("LayerArea.qml"),
        QmlFile::from("MeterRow.qml"),
        QmlFile::from("HudText.qml"),
        QmlFile::from("MeterSizes.qml"),
        QmlFile::from("Theme.qml").singleton(true),
        QmlFile::from("I18n.qml").singleton(true),
        QmlFile::from("Format.qml").singleton(true),
        // The other windows and their parts.
        QmlFile::from("AppWindow.qml"),
        QmlFile::from("SkillCard.qml"),
        QmlFile::from("DetailsWindow.qml"),
        QmlFile::from("DpsChart.qml"),
        QmlFile::from("Timeline.qml"),
        QmlFile::from("TimeAxis.qml"),
        QmlFile::from("HistoryWindow.qml"),
        QmlFile::from("HistoryPages.qml"),
        QmlFile::from("GameRecordView.qml"),
        QmlFile::from("SettingsWindow.qml"),
        QmlFile::from("SettingRow.qml"),
        QmlFile::from("UiText.qml"),
        QmlFile::from("UiButton.qml"),
        QmlFile::from("UiSwitch.qml"),
        QmlFile::from("UiSlider.qml"),
        QmlFile::from("UiSegmented.qml"),
        QmlFile::from("UiDropdown.qml"),
        QmlFile::from("UiMenu.qml"),
        QmlFile::from("UiMenuItem.qml"),
        QmlFile::from("UiMenuSeparator.qml"),
        QmlFile::from("UiField.qml"),
        QmlFile::from("UiTable.qml"),
        QmlFile::from("UiTip.qml"),
        QmlFile::from("UiScrollBar.qml"),
        QmlFile::from("PointerRest.qml"),
        QmlFile::from("Chevron.qml"),
        QmlFile::from("ConfirmStep.qml"),
        QmlFile::from("AutoHide.qml"),
        QmlFile::from("Reopen.qml"),
        QmlFile::from("SkillOrder.qml"),
        QmlFile::from("NoData.qml"),
        QmlFile::from("HotkeyText.qml"),
        QmlFile::from("SkillIcon.qml"),
        QmlFile::from("NoticeTray.qml"),
        QmlFile::from("NoticeIcon.qml"),
        QmlFile::from("GearGlyph.qml"),
        QmlFile::from("UiTipCard.qml"),
        QmlFile::from("CharacterWindow.qml"),
        QmlFile::from("CharacterGear.qml"),
        QmlFile::from("CharacterGearCard.qml"),
        QmlFile::from("CharacterStats.qml"),
        QmlFile::from("CharacterSkills.qml"),
        QmlFile::from("CharacterBoards.qml"),
        QmlFile::from("CharacterBoard.qml"),
    ]);

    let builder = CxxQtBuilder::new_qml_module(module)
        .files(["src/meter.rs", "src/details.rs", "src/history.rs", "src/overlay.rs", "src/settings.rs", "src/i18n.rs", "src/shell.rs", "src/menus.rs", "src/system.rs", "src/capture.rs", "src/account.rs", "src/instance.rs", "src/windows.rs", "src/hotkey.rs", "src/skill_icons.rs", "src/notices.rs", "src/gear_glyphs.rs", "src/character.rs"])
        .cpp_file("cpp/shell.cpp")
        .cpp_file("cpp/menus.cpp")
        .cpp_file("cpp/instance.cpp")
        // Class icons, the app icon and Pretendard live elsewhere in the repo;
        // the .qrc gives them short ASCII names.
        .qrc("resources.qrc")
        .qrc_resources(skill_icons())
        .qt_module("Gui")
        .qt_module("Network")
        .qt_module("Quick")
        .qt_module("Widgets");
    // The app id: the .desktop file's name and the data folder's. The
    // developer build (feature `dev`, package daevalog-dps-meter-dev) has
    // its own, so it runs beside an installed release.
    let app_id = if std::env::var_os("CARGO_FEATURE_DEV").is_some() {
        "daevalog-dps-meter-dev"
    } else {
        "daevalog-dps-meter"
    };
    println!("cargo:rustc-env=DAEVALOG_APP_ID={app_id}");
    // DAEVALOG_QML_DEBUG set at build time turns QML debugging on
    // (`cpp/shell.cpp`), for qmlprofiler. Never in a package: the debug
    // port is open to anything on the machine.
    println!("cargo:rerun-if-env-changed=DAEVALOG_QML_DEBUG");
    let qml_debug = std::env::var_os("DAEVALOG_QML_DEBUG").is_some();
    let dev = std::env::var_os("CARGO_FEATURE_DEV").is_some();
    // SAFETY: plain defines; cxx-qt's own settings stay as they are.
    let builder = unsafe {
        builder.cc_builder(move |cc| {
            cc.define("DAEVALOG_APP_ID", format!("\"{app_id}\"").as_str());
            if qml_debug {
                cc.define("QT_QML_DEBUG", None);
            }
            if dev {
                cc.define("DAEVALOG_DEV", None);
            }
        })
    };
    builder.build();
}

/// Daevalog's own skill icons (skill-icons/): each
/// SVG compiled in under qrc:/daevalog/skill-icons/, and their ids for
/// `src/skill_icons.rs`.
fn skill_icons() -> QResources {
    println!("cargo:rerun-if-changed=skill-icons");
    println!("cargo:rerun-if-changed=skill-icons/generic");
    let svgs = |dir: &str| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".svg").map(str::to_string))
            .collect();
        names.sort();
        names
    };
    let mut ids: Vec<i32> = svgs("skill-icons").iter().map(|n| n.parse().unwrap_or_else(|_| panic!("skill-icons/{n}.svg"))).collect();
    ids.sort_unstable();
    let generic = svgs("skill-icons/generic");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("skill_icons.rs");
    std::fs::write(out, format!("const ICONS: &[i32] = &{ids:?};\nconst GENERIC: &[&str] = &{generic:?};\n")).unwrap();
    // The alias is the path given, so the files sit under the prefix as
    // they sit here.
    let icons = QResource::new()
        .prefix("/daevalog")
        .files(ids.iter().map(|id| format!("skill-icons/{id}.svg")))
        .files(generic.iter().map(|job| format!("skill-icons/generic/{job}.svg")));
    QResources::new().resource(icons)
}

/// The gear slot glyphs (icons/gear/): each SVG's path data, joined, for
/// `src/gear_glyphs.rs`. QML draws it with a Shape in the item's grade
/// colour, so a glyph may hold nothing but <path d="..."/> outlines.
fn gear_glyphs() {
    println!("cargo:rerun-if-changed=icons/gear");
    let mut glyphs: Vec<(String, String)> = std::fs::read_dir("icons/gear")
        .expect("icons/gear")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".svg").map(str::to_string))
        .map(|name| {
            let svg = std::fs::read_to_string(format!("icons/gear/{name}.svg")).unwrap();
            let body = svg.split("-->").last().unwrap();
            let mut paths = Vec::new();
            for element in body.split('<').skip(1).filter(|e| !e.starts_with('/')) {
                if let Some(rest) = element.strip_prefix("path d=\"") {
                    paths.push(rest.split('"').next().unwrap().to_string());
                } else {
                    panic!("icons/gear/{name}.svg: only paths, found <{element}");
                }
            }
            (name, paths.join(" "))
        })
        .collect();
    glyphs.sort();
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("gear_glyphs.rs");
    std::fs::write(out, format!("const GLYPHS: &[(&str, &str)] = &{glyphs:?};\n")).unwrap();
}
