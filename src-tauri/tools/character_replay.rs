//! `character-replay`: your own character's latest state from packet
//! captures, through the meter's own reader (`capture::character_report`), for
//! a character file: a separate program that reads nothing else from the
//! meter.
//!
//! ```text
//! character-replay [--game-data DIR] <packets_*.txt>...    oldest first
//! ```
//!
//! Writes one JSON object, schema 1: `"schema": 1` and the report's sections,
//! `source` (the files given, the last record's time), `character`,
//! `item_level`, `combat_power`, `gear`, `inventory`, `stats`, `skills`,
//! `specialties`, `daevanion`, `titles`, `skins`, `quickslots` and
//! `collections`. Each section holds its own `as_of`, the capture time it was
//! read at; a section no capture held is null or empty. `--game-data` names
//! the game data folder; without it every name stays an id. A capture that
//! cannot be read stops the run (exit 1). A field that changes meaning or goes
//! away raises the schema; a new field does not.

use std::path::PathBuf;
use std::process::ExitCode;

use a2tools_dps_meter_lib::capture::character_report::{self, CaptureText, Names};
use serde_json::{json, Value};

const USAGE: &str = "usage: character-replay [--game-data DIR] <packets_*.txt>...\n";

/// The output's schema (see the module header).
const SCHEMA: u32 = 1;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let game_data = match args.iter().position(|a| a == "--game-data") {
        Some(i) if i + 1 < args.len() => {
            let dir = args.remove(i + 1);
            args.remove(i);
            Some(PathBuf::from(dir))
        }
        Some(_) => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
        None => None,
    };
    if args.is_empty() || args.iter().any(|a| a.starts_with("--")) {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    let mut texts = Vec::new();
    for path in &args {
        match std::fs::read_to_string(path) {
            Ok(text) => texts.push(text),
            Err(why) => {
                eprintln!("character-replay: cannot read {path}: {why}");
                return ExitCode::FAILURE;
            }
        }
    }
    let logs: Vec<CaptureText> = args.iter().zip(&texts).map(|(name, text)| CaptureText { name, text }).collect();
    let names = game_data.map_or_else(Names::default, |dir| Names::load(&dir));
    let report = with_schema(character_report::report(&logs, &names));
    println!("{}", serde_json::to_string_pretty(&report).expect("a Value serialises"));
    ExitCode::SUCCESS
}

/// The report with the schema beside its sections.
fn with_schema(mut report: Value) -> Value {
    if let Some(sections) = report.as_object_mut() {
        sections.insert("schema".into(), json!(SCHEMA));
    }
    report
}

// The crate's record builders (`src/built.rs`) name the library through
// `crate::`; this leads there.
#[cfg(test)]
use a2tools_dps_meter_lib::{capture, entity};

#[cfg(test)]
#[allow(dead_code)]
#[path = "../src/built.rs"]
mod built;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{self_record, to_hex};

    /// The whole output for one self record (Tester, Cleric, server 1201,
    /// level 45, Item Level 500) without the game's tables: every section the
    /// module header names, each with its `as_of`.
    #[test]
    fn a_self_record_writes_schema_1() {
        let text = format!("2026-10-15T04:20:00.000-07:00|Server:13328:50000|{}", to_hex(&self_record(9001, "Tester", 1201, 30, 45)));
        let out = with_schema(character_report::report(&[CaptureText { name: "a.txt", text: &text }], &Names::default()));
        let want = concat!(
            r#"{"character":{"as_of":"2026-10-15T04:20:00.000-07:00","class":"Cleric","class_name":null,"entity":90"#,
            r#"01,"level":45,"name":"Tester","server":1201},"collections":null,"combat_power":null,"daevanion":{"as"#,
            r#"_of":null,"boards":[]},"gear":{"as_of":null,"items":[]},"inventory":{"as_of":null,"items":[]},"item_"#,
            r#"level":{"as_of":"2026-10-15T04:20:00.000-07:00","current":500,"highest":500},"quickslots":null,"sche"#,
            r#"ma":1,"skills":{"as_of":null,"list":[]},"skins":null,"source":{"files":["a.txt"],"last_record":"2026"#,
            r#"-10-15T04:20:00.000-07:00"},"specialties":{"as_of":null,"list":[]},"stats":{"as_of":null,"values":[]"#,
            r#"},"titles":null}"#,
        );
        assert_eq!(out.to_string(), want);
    }
}
