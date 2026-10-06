// SPDX-License-Identifier: MIT

//! Daevalog meter, Qt Quick front end. Prototype step 1: the overlay window
//! with mock rows, no backend data yet.

mod overlay;
mod shell;

use std::sync::OnceLock;

use cxx_qt::ConnectionType;
use cxx_qt_lib::{QQmlApplicationEngine, QUrl};

const MAIN_QML: &str = "qrc:/qt/qml/Daevalog/Main.qml";

const USAGE: &str = "\
Usage: daevalog-qml [--layer-shell] [--locked] [--screenshot FILE.png]

  --layer-shell       place the meter with the Wayland layer shell
                      (org.kde.layershell); a normal window if that is missing
  --locked            start locked: clicks go through the meter to the game
  --screenshot FILE   draw the meter, save it as a PNG and quit
";

#[derive(Debug, Default, PartialEq)]
pub struct Options {
    pub layer_shell: bool,
    pub locked: bool,
    pub screenshot: Option<String>,
}

static OPTIONS: OnceLock<Options> = OnceLock::new();

/// The command-line options; read by `Overlay` when QML first uses it.
pub fn options() -> &'static Options {
    OPTIONS.get_or_init(Options::default)
}

#[derive(Debug, PartialEq)]
enum Command {
    Run(Options),
    Help,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--layer-shell" => options.layer_shell = true,
            "--locked" => options.locked = true,
            "--screenshot" => {
                let file = args.next().ok_or("--screenshot needs a file name")?;
                options.screenshot = Some(file.clone());
            }
            "-h" | "--help" => return Ok(Command::Help),
            // Qt's own options (-platform, -style ...) have one dash and are Qt's.
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => {}
        }
    }
    Ok(Command::Run(options))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let options = match parse_args(args.get(1..).unwrap_or_default()) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            print!("{USAGE}");
            return;
        }
        Err(error) => {
            eprint!("daevalog-qml: {error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    OPTIONS
        .set(options)
        .expect("options are set once, before Qt starts");

    let mut app = shell::ffi::new_application(&args);
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
        engine.load(&QUrl::from(MAIN_QML));
    }
    let code = match app.as_mut() {
        Some(app) => shell::ffi::exec_application(app),
        None => 1,
    };
    // The engine goes before the application it runs in.
    drop(engine);
    drop(app);
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
        let parsed = parse_args(&args(&[
            "-platform",
            "offscreen",
            "--layer-shell",
            "--screenshot",
            "meter.png",
        ]));
        assert_eq!(
            parsed,
            Ok(Command::Run(Options {
                layer_shell: true,
                locked: false,
                screenshot: Some("meter.png".into()),
            }))
        );
    }

    #[test]
    fn bad_options_are_refused() {
        assert!(parse_args(&args(&["--screenshot"])).is_err());
        assert!(parse_args(&args(&["--layershell"])).is_err());
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
    }
}
