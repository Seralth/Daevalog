// SPDX-License-Identifier: AGPL-3.0-only

//! Daevalog meter, Qt Quick front end: the overlay window, its rows fed by
//! the backend from live capture, or from a packet log with --replay.

// A release build on Windows opens no console window. Debug builds keep it
// for the log.
#![cfg_attr(all(windows, not(test), not(debug_assertions)), windows_subsystem = "windows")]

mod account;
mod capture;
mod details;
mod display;
mod enrage;
mod history;
mod hotkey;
mod hyprland;
mod i18n;
mod instance;
mod layer_drag;
mod live;
mod menus;
mod meter;
mod notices;
mod overlay;
mod party;
mod pet_souls;
mod power_shards;
mod world_spawns;
#[cfg(test)]
mod qml_checks;
#[cfg(test)]
mod quick_tests;
mod replay;
mod settings;
mod settings_store;
mod shell;
mod skill_icons;
mod snapshot;
mod system;
mod windows;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cxx_qt::ConnectionType;
use cxx_qt_lib::{QQmlApplicationEngine, QUrl};

use crate::settings_store::{MODES, Overrides, Store};

const MAIN_QML: &str = "qrc:/qt/qml/Daevalog/Main.qml";

/// The capture helper this program starts, from its own folder. The
/// developer build's has its own name, so it installs beside the release.
#[cfg(all(not(windows), not(feature = "dev")))]
pub const CAPTURE_HELPER: &str = "daevalog-capture";
#[cfg(all(not(windows), feature = "dev"))]
pub const CAPTURE_HELPER: &str = "daevalog-capture-dev";

/// The help text: the top and the capture helper differ by system.
#[cfg(not(windows))]
const USAGE_TOP: &str = "\
Usage: daevalog-dps-meter [--layer-shell] [--locked] [--screenshot FILE.png]
                          [--open VIEW] [--width N] [--icon-size N] [--font-size N]
                          [--meter-size N]
                          [--mode MODE] [--you] [--players N] [--pin-you]
                          [--hide-names] [--language LANG] [--theme THEME] [--columns LIST]
                          [--capture-helper FILE]
                          [--replay FILE [--fast] [--until TIME] [--print-fights]
                           [--ping MS] [--save-fights]]

Without --replay the meter captures live, through its capture helper
daevalog-capture in the program's own folder. Its settings are in
settings.json in its data folder (~/.local/share/daevalog-dps-meter); on the
first start it takes over the 1.x meter's settings and saved fights. A second
start shows the running meter and quits. The options below change a setting
for this run only.

  --layer-shell       place the meter with the Wayland layer shell
                      (org.kde.layershell), the default on KDE, Hyprland and
                      Sway; a normal window where the compositor or Qt
                      has none
";
#[cfg(windows)]
const USAGE_TOP: &str = "\
Usage: daevalog-dps-meter [--locked] [--screenshot FILE.png]
                          [--open VIEW] [--width N] [--icon-size N] [--font-size N]
                          [--meter-size N]
                          [--mode MODE] [--you] [--players N] [--pin-you]
                          [--hide-names] [--language LANG] [--theme THEME] [--columns LIST]
                          [--replay FILE [--fast] [--until TIME] [--print-fights]
                           [--ping MS] [--save-fights]]

Without --replay the meter captures live through Npcap, in its own process.
Its settings are in settings.json in its data folder
(%APPDATA%\\daevalog-dps-meter). A second start shows the running meter and
quits. The options below change a setting for this run only.

";
const USAGE_OPTIONS: &str = "  --locked            start locked: clicks go through the meter to the game
  --screenshot FILE   draw the meter, save it as a PNG and quit; with
                      --replay, once the replay has ended
  --width N           the window --open opens, N px wide (not below its
                      smallest width), for a screenshot of a narrow one
  --icon-size N       Details' skill icons N px (20 to 56)
  --font-size N       Details' text N px (11 to 20)
  --meter-size N      the meter's text and rows at N% (100, 115, 130 or 150)
  --open VIEW         open a window at the start: meter/heal, meter/tank,
                      meter/split, meter/slim, meter/health-split,
                      meter/low-share-number, meter/damage-line,
                      meter/notice, meter/card, meter/tray and
                      meter/tray-closed (the notice tray, with sample
                      notices),
                      meter/paused (suspended), details, details/dmg,
                      details/heal, details/taken,
                      details/mechanics, details/timeline, details/options,
                      details/sorted (by hits, each skill's damage over
                      time open),
                      details/monitor (the monitor badge, held),
                      details/every-target (you on every target),
                      details/row (as a click on the meter's second row),
                      details/row-timeline (the same, on its timeline),
                      details/saved, details/saved-timeline (the newest
                      fight in History),
                      history, history/heal, history/tank,
                      history/record, history/player, settings, settings/PAGE
                      (connection, appearance, meter, meter-heal, account,
                      keybinds, diagnostics, report, advanced), and
                      settings/PAGE-full (the window as tall as the page);
                      with --screenshot, that window is saved
  --mode MODE         boss, target, all, train or enc (default: the Default
                      mode setting; boss with --replay)
  --you               show YOU on your own row instead of your name
  --players N         rows shown, your own row besides: 5, 8, 10 or 12, N
                      rounded up, 12 at most (default: the setting, 10
                      unless changed)
  --pin-you           your own row first
  --hide-names        show other players as their class and a number
  --language LANG     en, de, es, fr, ja, ko, pt or ru
                      (default: the setting, else the system's language)
  --theme THEME       daevalog, classic, nord or system (default: the setting)
  --columns LIST      the DPS tab's columns, comma-separated: total, encDps,
                      dps, pct, crit, last10, last30, last60, maxHit, hits
";
#[cfg(not(windows))]
const USAGE_HELPER: &str = "  --capture-helper FILE
                      start this capture helper instead (for a build that is
                      not installed; the file needs cap_net_raw)
";
#[cfg(windows)]
const USAGE_HELPER: &str = "";
const USAGE_REPLAY: &str = "  --replay FILE       feed a packet log (packets_*.txt) to the meter instead
                      of a live capture, in real time, with the default
                      settings: no settings file is read or written
  --fast              replay as fast as possible
  --until TIME        stop the replay at this time of day, e.g. 15:00:12
  --print-fights      print each fight as the meter last showed it, and the
                      meter at the end of the replay, by entity id
  --ping MS           show this ping in the footer (a replay has none), for
                      a screenshot of its colour
  --save-fights       save the replay's boss fights to the data folder, as
                      the live meter does: every 30 s of capture time, and
                      once more a minute after the last packet
";

/// The windows --open knows; Main.qml reads the page after the slash.
const WINDOWS: [&str; 4] = ["meter", "details", "history", "settings"];

/// --mode's names for the backend's modes, in `MODES`' order.
const MODE_NAMES: [&str; 5] = ["boss", "target", "all", "train", "enc"];

#[derive(Debug, Default, PartialEq)]
pub struct Options {
    pub screenshot: Option<String>,
    /// The window, and its page, to open at the start ("history/record").
    pub open: Option<String>,
    /// --width: the opened window's width, for a screenshot of a narrow one.
    pub width: Option<i32>,
    /// Settings for this run only.
    pub overrides: Overrides,
    pub capture_helper: Option<PathBuf>,
    pub replay: Option<replay::Replay>,
    pub print_fights: bool,
    /// --ping: the ping a replay shows, for a screenshot.
    pub ping: Option<i32>,
}

static OPTIONS: OnceLock<Options> = OnceLock::new();

/// The command-line options; read by `Overlay` when QML first uses it.
pub fn options() -> &'static Options {
    OPTIONS.get_or_init(Options::default)
}

fn usage() -> String {
    [USAGE_TOP, USAGE_OPTIONS, USAGE_HELPER, USAGE_REPLAY].concat()
}

/// Where the installed game data is, from the program's folder: a share
/// folder beside bin on Linux, a data folder beside the .exe on Windows.
#[cfg(not(windows))]
const INSTALLED_DATA: &str = concat!("../share/", env!("DAEVALOG_APP_ID"), "/data");
#[cfg(windows)]
const INSTALLED_DATA: &str = "data";

/// The game data (skill and NPC names, the boss and dummy tables): installed
/// next to the program, or the source tree's when run from a build.
pub fn data_dir() -> Option<PathBuf> {
    let installed = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(INSTALLED_DATA)));
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
    installed
        .into_iter()
        .chain([source])
        .find(|dir| dir.join("i18n/skills").is_dir())
}

/// A JSON file of the game data or the data folder; `None` when it is
/// missing or not JSON.
pub fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The capture helper to start: --capture-helper, else the one in this
/// program's folder, never one from PATH.
#[cfg(not(windows))]
pub fn capture_helper() -> Option<PathBuf> {
    options()
        .capture_helper
        .clone()
        .or_else(|| Some(std::env::current_exe().ok()?.with_file_name(CAPTURE_HELPER)))
}

/// None on Windows: the meter captures in its own process.
#[cfg(windows)]
pub fn capture_helper() -> Option<PathBuf> {
    None
}

#[derive(Debug, PartialEq)]
enum Command {
    Run(Options),
    Help,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut options = Options::default();
    let (mut file, mut fast, mut until, mut save) = (None, false, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let overrides = &mut options.overrides;
        match arg.as_str() {
            "--layer-shell" if cfg!(windows) => {
                return Err("--layer-shell is for Wayland on Linux; on Windows the meter is a normal window".into());
            }
            "--layer-shell" => overrides.layer_shell = true,
            "--locked" => overrides.locked = true,
            "--screenshot" => {
                let file = args.next().ok_or("--screenshot needs a file name")?;
                options.screenshot = Some(file.clone());
            }
            "--open" => {
                let view = args.next().ok_or("--open needs a window")?;
                let window = view.split('/').next().unwrap_or_default();
                if !WINDOWS.contains(&window) {
                    return Err(format!("unknown window {view}"));
                }
                // Layouts that are settings, shown for this run only.
                overrides.slim_mode |= view == "meter/slim";
                overrides.split_view |= view == "meter/split";
                overrides.health_split |= view == "meter/health-split";
                overrides.low_share_number |= view == "meter/low-share-number";
                overrides.damage_line |= view == "meter/damage-line";
                options.open = Some(view.clone());
            }
            "--width" => {
                let width = args.next().ok_or("--width needs a number")?;
                options.width = Some(width.parse().ok().filter(|w| *w > 0).ok_or_else(|| format!("bad width {width}"))?);
            }
            "--icon-size" | "--font-size" => {
                let px = args.next().ok_or_else(|| format!("{arg} needs a number"))?;
                let px = Some(px.parse().map_err(|_| format!("bad size {px}"))?);
                if arg == "--icon-size" {
                    overrides.details_icon_size = px;
                } else {
                    overrides.details_font_size = px;
                }
            }
            "--meter-size" => {
                let size = args.next().ok_or("--meter-size needs a number")?;
                let size = size.parse().ok().filter(|n| settings_store::METER_SIZES.contains(n));
                overrides.meter_size = Some(size.ok_or("--meter-size is 100, 115, 130 or 150")?);
            }
            "--mode" => {
                let name = args.next().ok_or("--mode needs a mode")?;
                let at = MODE_NAMES.iter().position(|n| n == name).ok_or_else(|| format!("unknown mode {name}"))?;
                overrides.default_mode = Some(MODES[at].to_string());
            }
            "--you" => overrides.show_you = true,
            "--players" => {
                let n = args.next().ok_or("--players needs a number")?;
                let n = n.parse().ok().filter(|&n| n > 0).ok_or_else(|| format!("bad number of players {n}"))?;
                overrides.player_limit = Some(n);
            }
            "--pin-you" => overrides.pin_you = true,
            "--hide-names" => overrides.hide_other_names = true,
            "--columns" => {
                let list = args.next().ok_or("--columns needs a list")?;
                let mut columns = Vec::new();
                for key in list.split(',').filter(|k| !k.is_empty()) {
                    let known = settings_store::DPS_COLUMNS.iter().find(|&&k| k == key);
                    columns.push(*known.ok_or_else(|| format!("unknown column {key}"))?);
                }
                overrides.dps_columns = Some(columns);
            }
            "--theme" => {
                let theme = args.next().ok_or("--theme needs a theme")?;
                if !settings_store::COLOR_THEMES.contains(&theme.as_str()) {
                    return Err(format!("unknown theme {theme}"));
                }
                overrides.color_theme = Some(theme.clone());
            }
            "--language" => {
                let language = args.next().ok_or("--language needs a language")?;
                if !i18n::LANGUAGES.contains(&language.as_str()) {
                    return Err(format!("unknown language {language}"));
                }
                overrides.language = Some(language.clone());
            }
            "--capture-helper" if cfg!(windows) => {
                return Err("--capture-helper is for Linux; on Windows the meter captures by itself".into());
            }
            "--capture-helper" => {
                let file = args.next().ok_or("--capture-helper needs a file")?;
                options.capture_helper = Some(PathBuf::from(file));
            }
            "--replay" => file = Some(PathBuf::from(args.next().ok_or("--replay needs a file")?)),
            "--fast" => fast = true,
            "--until" => until = Some(args.next().ok_or("--until needs a time of day")?.clone()),
            "--print-fights" => options.print_fights = true,
            "--save-fights" => save = true,
            "--ping" => {
                let ms = args.next().ok_or("--ping needs a number of ms")?;
                options.ping = Some(ms.parse().ok().filter(|&ms| ms >= 0).ok_or_else(|| format!("bad ping {ms}"))?);
            }
            "-h" | "--help" => return Ok(Command::Help),
            // Qt's own options (-platform, -style ...) have one dash and are Qt's.
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => {}
        }
    }
    match file {
        Some(_) if options.capture_helper.is_some() => {
            return Err("--capture-helper is for live capture, not --replay".into());
        }
        Some(file) => {
            let mode = options.overrides.default_mode.clone().unwrap_or_else(|| MODES[0].to_string());
            options.replay = Some(replay::Replay { file, fast, until, mode, save });
        }
        None if fast || until.is_some() || options.print_fights || options.ping.is_some() || save => {
            return Err("--fast, --until, --print-fights, --ping and --save-fights need --replay".into());
        }
        None => {}
    }
    Ok(Command::Run(options))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let options = match parse_args(args.get(1..).unwrap_or_default()) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            print!("{}", usage());
            return;
        }
        Err(error) => {
            eprint!("daevalog-dps-meter: {error}\n\n{}", usage());
            std::process::exit(2);
        }
    };
    // Before anything reads the version: Settings, reports and uploads say 2.0.
    daevalog::version::set_release(system::RELEASE);
    display::choose_platform(&args);
    let live = options.replay.is_none();
    if live {
        if let Some(done) = settings_store::prepare_folder() {
            eprintln!("daevalog-dps-meter: data folder: {done}");
        }
    }
    let guarded = instance::guarded(&options);
    // Before the settings are read: a second meter writes nothing.
    if let Some(folder) = settings_store::folder().filter(|_| guarded) {
        if !instance::claim(&folder) {
            eprintln!("daevalog-dps-meter: the meter is already running; asked it to show");
            return;
        }
    }
    // A replay is a check: the same with every player's settings.
    let store = match settings_store::folder().filter(|_| live) {
        Some(folder) => Store::open(&folder, settings_store::legacy_folder().as_deref()),
        None => Store::in_memory(),
    };
    store.default_layer_shell(display::layer_shell_by_default());
    store.apply(&options.overrides);
    settings_store::init(store);
    OPTIONS
        .set(options)
        .expect("options are set once, before Qt starts");
    if live {
        // The capture's log lines, on standard error, and in debug.log and
        // the packet log when the settings say so.
        daevalog::logging::logger::init_logging();
        settings_store::store().apply_logging();
    }

    let mut app = shell::ffi::new_application(&args);
    i18n::set_system_languages(&i18n::qobject::system_languages());
    let mut engine = QQmlApplicationEngine::new();
    if let Some(mut engine) = engine.as_mut() {
        // A QML error leaves no window; quit instead of running without one.
        engine
            .as_mut()
            .connect_object_creation_failed(
                |_, _| shell::ffi::exit_application(1),
                ConnectionType::QueuedConnection,
            )
            .release();
        engine.as_mut().load(&QUrl::from(MAIN_QML));
        if guarded {
            instance::ffi::listen_instance(engine.as_mut());
        }
    }
    let code = match app.as_mut() {
        Some(app) => shell::ffi::exec_application(app),
        None => 1,
    };
    // The engine goes before the application it runs in.
    drop(engine);
    drop(app);
    // The capture helper quits with its input closed.
    daevalog::capture::live::stop();
    live::save_before_quit();
    if let Err(error) = settings_store::store().flush() {
        eprintln!("daevalog-dps-meter: the settings were not saved: {error}");
    }
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags_and_qt_options() {
        let parsed = parse_args(&args(&["-platform", "offscreen", "--locked", "--screenshot", "meter.png"]));
        assert_eq!(
            parsed,
            Ok(Command::Run(Options {
                screenshot: Some("meter.png".into()),
                overrides: Overrides { locked: true, ..Overrides::default() },
                ..Options::default()
            }))
        );
    }

    #[test]
    fn a_replay_takes_its_mode_speed_and_end() {
        let parsed = parse_args(&args(&[
            "--mode", "train", "--replay", "p.txt", "--fast", "--until", "15:00:12", "--you",
        ]));
        let Ok(Command::Run(options)) = parsed else { panic!("{parsed:?}") };
        assert_eq!(options.overrides.default_mode.as_deref(), Some("trainTargets"));
        assert!(options.overrides.show_you);
        assert_eq!(
            options.replay,
            Some(replay::Replay {
                file: "p.txt".into(),
                fast: true,
                until: Some("15:00:12".into()),
                mode: "trainTargets".into(),
                save: false,
            })
        );
    }

    #[test]
    fn a_replay_shows_a_ping_only_when_asked() {
        let parsed = parse_args(&args(&["--replay", "p.txt", "--ping", "120"]));
        let Ok(Command::Run(options)) = parsed else { panic!("{parsed:?}") };
        assert_eq!(options.ping, Some(120));
        let Ok(Command::Run(options)) = parse_args(&args(&["--replay", "p.txt"])) else { panic!() };
        assert_eq!(options.ping, None);
        assert!(parse_args(&args(&["--ping", "120"])).is_err(), "--ping without --replay");
        assert!(parse_args(&args(&["--replay", "p.txt", "--ping", "-5"])).is_err());
        assert!(parse_args(&args(&["--replay", "p.txt", "--ping"])).is_err());
    }

    #[test]
    fn a_replay_saves_its_fights_only_when_asked() {
        let Ok(Command::Run(options)) = parse_args(&args(&["--replay", "p.txt", "--save-fights"])) else { panic!() };
        assert!(options.replay.is_some_and(|r| r.save));
        let Ok(Command::Run(options)) = parse_args(&args(&["--replay", "p.txt"])) else { panic!() };
        assert!(options.replay.is_some_and(|r| !r.save));
        assert!(parse_args(&args(&["--save-fights"])).is_err(), "--save-fights without --replay");
    }

    #[cfg(not(windows))]
    #[test]
    fn live_capture_takes_rows_and_a_helper() {
        let parsed = parse_args(&args(&["--players", "8", "--pin-you", "--layer-shell", "--capture-helper", "/tmp/helper"]));
        let Ok(Command::Run(options)) = parsed else { panic!("{parsed:?}") };
        assert_eq!((options.overrides.player_limit, options.overrides.pin_you), (Some(8), true));
        assert!(options.overrides.layer_shell);
        assert_eq!(options.capture_helper, Some(PathBuf::from("/tmp/helper")));
        assert_eq!(options.replay, None);
        assert!(parse_args(&args(&["--players", "0"])).is_err());
        assert!(parse_args(&args(&["--capture-helper", "h", "--replay", "p.txt"])).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_has_no_capture_helper() {
        assert!(parse_args(&args(&["--capture-helper", "h.exe"])).is_err());
        assert!(parse_args(&args(&["--layer-shell"])).is_err());
        assert_eq!(capture_helper(), None);
        assert!(!usage().contains("--capture-helper") && !usage().contains("--layer-shell"));
    }

    #[test]
    fn names_hidden_in_another_language() {
        let parsed = parse_args(&args(&["--hide-names", "--language", "ko"]));
        let Ok(Command::Run(options)) = parsed else { panic!("{parsed:?}") };
        assert!(options.overrides.hide_other_names);
        assert_eq!(options.overrides.language.as_deref(), Some("ko"));
        assert!(parse_args(&args(&["--language", "zh"])).is_err());
        assert!(parse_args(&args(&["--language", "zh-Hans"])).is_err());
        let Ok(Command::Run(themed)) = parse_args(&args(&["--theme", "nord"])) else { panic!("--theme") };
        assert_eq!(themed.overrides.color_theme.as_deref(), Some("nord"));
        assert!(parse_args(&args(&["--theme", "pink"])).is_err());
        assert!(parse_args(&args(&["--language"])).is_err());
    }

    #[test]
    fn a_window_opens_at_the_start() {
        let parsed = parse_args(&args(&["--open", "history/record", "--screenshot", "h.png"]));
        let Ok(Command::Run(options)) = parsed else { panic!("{parsed:?}") };
        assert_eq!(options.open.as_deref(), Some("history/record"));
        let Ok(Command::Run(narrow)) = parse_args(&args(&["--open", "details", "--width", "760"])) else { panic!() };
        assert_eq!(narrow.width, Some(760));
        assert!(parse_args(&args(&["--width", "wide"])).is_err());
        let Ok(Command::Run(sized)) = parse_args(&args(&["--icon-size", "56", "--font-size", "20"])) else { panic!() };
        assert_eq!((sized.overrides.details_icon_size, sized.overrides.details_font_size), (Some(56), Some(20)));
        assert!(parse_args(&args(&["--icon-size", "big"])).is_err());
        let Ok(Command::Run(big)) = parse_args(&args(&["--open", "meter/slim", "--meter-size", "150"])) else { panic!() };
        assert_eq!((big.overrides.meter_size, big.overrides.slim_mode), (Some(150), true));
        for refused in ["120", "200", "big"] {
            assert!(parse_args(&args(&["--meter-size", refused])).is_err(), "{refused}");
        }
        assert!(parse_args(&args(&["--open", "tray"])).is_err());
        assert!(parse_args(&args(&["--open"])).is_err());
    }

    #[test]
    fn columns_for_one_run() {
        let Ok(Command::Run(options)) = parse_args(&args(&["--columns", "hits,total,last60"])) else {
            panic!("--columns refused");
        };
        assert_eq!(options.overrides.dps_columns, Some(vec!["hits", "total", "last60"]));
        let mut values = settings_store::Values::default();
        options.overrides.apply_to(&mut values);
        assert_eq!(values.meter_columns.dps, vec!["total", "last60", "hits"], "in the list's order");
        assert!(parse_args(&args(&["--columns", "total,heal"])).is_err(), "a column of another tab");
        assert!(parse_args(&args(&["--columns"])).is_err());
    }

    #[test]
    fn the_help_names_what_the_options_take() {
        let help = usage();
        let top = help.split("\n\n").next().unwrap_or_default();
        for option in ["--width N", "--icon-size N", "--font-size N", "--meter-size N"] {
            assert!(top.contains(option), "{option} in the usage line");
        }
        for said in [
            "details/dmg",
            "details/sorted",
            "details/row-timeline",
            "settings/PAGE-full",
            "the Default\n                      mode setting",
            "5, 8, 10 or 12, N\n                      rounded up",
        ] {
            assert!(help.contains(said), "{said}");
        }
        // A string continued with a backslash drops the next line's indent.
        assert!(!help.lines().any(|line| line.starts_with("--")), "{help}");
    }

    #[test]
    fn bad_options_are_refused() {
        assert!(parse_args(&args(&["--screenshot"])).is_err());
        assert!(parse_args(&args(&["--layershell"])).is_err());
        assert!(parse_args(&args(&["--mode", "dps"])).is_err());
        assert!(parse_args(&args(&["--fast"])).is_err(), "--fast without --replay");
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
    }
}
