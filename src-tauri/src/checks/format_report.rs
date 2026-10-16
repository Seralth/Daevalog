//! Reports on what the backend writes: the packet log, `settings.json`, PNG
//! files, and the fights derived from Evidence Slices.
//! JSON objects print with their keys sorted, since some come from hash maps.
//!
//! ```text
//! A2_REPLAY_FILE=packets_x.txt  cargo test --lib packet_log_report -- --ignored --nocapture
//!     every payload of the capture through the packet log writer; then each
//!     file it wrote after its first line (the wall-clock start): size, lines
//!     and SHA-256 (set TZ: times are local)
//! cargo test --lib settings_report -- --ignored --nocapture
//!     a scripted run of Settings edits, with the file after each flush
//! cargo test --lib png_report -- --ignored --nocapture
//!     `encode_png` of a few fixed images, as hex
//! A2_CONFORMANCE=<folder>  cargo test --lib derive_report -- --ignored --nocapture
//!     `slice_rebuild::fight_from_slice` of every slice in the folder and below
//! ```

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::capture::packet_log::LogLine;
use crate::config::settings::{read_file, Settings};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A JSON value with every object's keys in order.
fn sorted(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let ordered: BTreeMap<String, serde_json::Value> = map.into_iter().map(|(k, v)| (k, sorted(v))).collect();
            serde_json::Value::Object(ordered.into_iter().collect())
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn json<T: serde::Serialize>(value: &T) -> String {
    sorted(serde_json::to_value(value).expect("serialise")).to_string()
}

/// An empty folder for one report, named by the report and this process.
fn scratch(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("daevalog-report-{}-{name}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear the report folder");
    }
    std::fs::DirBuilder::new().recursive(true).create(&dir).expect("make the report folder");
    dir
}

#[test]
#[ignore]
fn packet_log_report() {
    let path = env("A2_REPLAY_FILE").expect("A2_REPLAY_FILE");
    let text = std::fs::read_to_string(&path).expect("read the capture");
    let dir = scratch("packet-log");
    crate::logging::logger::set_packet_log_enabled(true, &dir);
    let mut fed = 0usize;
    for line in text.lines() {
        let Some(line) = LogLine::parse(line) else { continue };
        crate::logging::logger::write_segment_line(&super::ping_report::payload(&line), None);
        fed += 1;
    }
    crate::logging::logger::set_packet_log_enabled(false, &dir);
    println!("fed {fed}");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).expect("list").flatten().map(|e| e.path()).collect();
    files.sort();
    for (n, file) in files.iter().enumerate() {
        let bytes = std::fs::read(file).expect("read");
        // The first line says when the writer started, by the wall clock.
        let start = bytes.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
        let head = String::from_utf8_lossy(&bytes[..start]);
        let head = head.split(" at ").next().unwrap_or_default();
        let rest = &bytes[start..];
        let lines = rest.iter().filter(|&&b| b == b'\n').count();
        println!("file {n} \"{head} at <now>\" then {} bytes {lines} lines sha256 {}", rest.len(), hex(&Sha256::digest(rest)));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Flush, then print what is on disk and what `get` answers.
fn settings_step(name: &str, settings: &Settings, file: &Path) {
    let flushed = settings.flush();
    let on_disk: BTreeMap<String, String> = read_file(file).into_iter().collect();
    let size = std::fs::metadata(file).map(|m| m.len() as i64).unwrap_or(-1);
    let answers: BTreeMap<&String, Option<String>> = on_disk.keys().map(|k| (k, settings.get(k))).collect();
    println!("settings {name} flush {flushed:?} file {size} bytes {}", serde_json::to_string(&on_disk).unwrap());
    println!("settings {name} get {}", serde_json::to_string(&answers).unwrap());
}

#[test]
#[ignore]
fn settings_report() {
    let dir = scratch("settings");
    let kotlin = dir.join("appdata/AionDPS");
    std::fs::create_dir_all(&kotlin).unwrap();
    std::fs::write(kotlin.join("settings.properties"), "# old app\nhotkey = Ctrl+F1\nopacity=0.8\n\n!skip\nlanguage=ko\nbroken line\n").unwrap();
    // SAFETY: this test runs alone in its process (`--ignored` with its name).
    unsafe { std::env::set_var("APPDATA", dir.join("appdata")) };
    let app = dir.join("app");
    let file = app.join("settings.json");

    let settings = Settings::new(app.clone());
    settings_step("migrated", &settings, &file);
    println!("settings set same {}", settings.set("opacity", "0.8"));
    println!("settings set new {}", settings.set("opacity", "0.9"));
    for i in 0..50 {
        settings.set("slider", &i.to_string());
    }
    settings.remove("language");
    settings.remove("missing");
    settings.set("名前", "値 \"quoted\" \\ \n second line");
    settings_step("edited", &settings, &file);
    drop(settings);

    let settings = Settings::new(app.clone());
    settings_step("reopened", &settings, &file);
    settings.clear();
    settings_step("cleared", &settings, &file);
    drop(settings);

    std::fs::write(&file, "not json").unwrap();
    let settings = Settings::new(app.clone());
    println!("settings unreadable get {:?}", settings.get("hotkey"));
    settings.set("hotkey", "Alt+Z");
    settings_step("rewritten", &settings, &file);
    drop(settings);
    let mut left: Vec<String> = std::fs::read_dir(&app).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    println!("settings folder {left:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn png_report() {
    use crate::platform::screenshot::encode_png;
    let images: [(u32, u32, Vec<u8>); 3] = [
        (1, 1, vec![255, 0, 0, 255]),
        (3, 2, (0..24u8).map(|i| i.wrapping_mul(37)).collect()),
        (
            16,
            9,
            (0..9u32)
                .flat_map(|y| (0..16u32).flat_map(move |x| [x as u8 * 16, y as u8 * 28, ((x ^ y) * 8) as u8, 255 - x as u8 * 4]))
                .collect(),
        ),
    ];
    for (w, h, rgba) in &images {
        println!("png {w}x{h} {}", hex(&encode_png(*w, *h, rgba)));
    }
}

/// Every slice under the conformance folder, path relative to it, sorted.
fn slices() -> Vec<(String, Vec<u8>)> {
    let root = PathBuf::from(env("A2_CONFORMANCE").expect("A2_CONFORMANCE"));
    let mut found = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("list").flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.to_string_lossy().ends_with(".a2es.gz") || path.to_string_lossy().ends_with(".a2es") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
        .into_iter()
        .map(|path| {
            let raw = std::fs::read(&path).expect("read slice");
            let slice = if path.to_string_lossy().ends_with(".gz") {
                let mut out = Vec::new();
                flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut out).expect("gunzip");
                out
            } else {
                raw
            };
            (path.strip_prefix(&root).unwrap().to_string_lossy().into_owned(), slice)
        })
        .collect()
}

#[test]
#[ignore]
fn derive_report() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/data");
    let read = |p: &str| std::fs::read_to_string(data.join(p)).expect(p);
    let (npcs, skills, dots) = (read("i18n/npcs/en.json"), read("i18n/skills/en.json"), read("dot_skill_ids.json"));
    for (name, slice) in slices() {
        match crate::slice_rebuild::fight_from_slice(&slice, &npcs, &skills, &dots) {
            Ok(fight) => println!("derive {name} {}", json(&fight)),
            Err(e) => println!("derive {name} error {e:?}"),
        }
    }
}

#[test]
fn sorted_json_orders_keys_at_every_depth() {
    let v: serde_json::Value = serde_json::from_str(r#"{"b":1,"a":{"d":[{"z":1,"y":2}],"c":3}}"#).unwrap();
    assert_eq!(sorted(v).to_string(), r#"{"a":{"c":3,"d":[{"y":2,"z":1}]},"b":1}"#);
}
