//! "Report a problem": the report info a bug report starts with, and the
//! bug-report copy of a packet log. Nothing here sends anything: the player
//! opens the issue form, and attaches what they choose.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::platform;
use crate::share::report_log;

/// The few facts every issue form asks for, one per line, for the meter of
/// `version` ("1.1 · r598") on the display `display` (what the front end
/// chose). Nothing that names the player or the computer: no user or
/// character names, no paths, no host name.
pub fn report_info(version: &str, display: Option<String>) -> String {
    let env = |name: &str| std::env::var(name).ok().map(|v| clean(&v)).filter(|v| !v.is_empty());
    let unknown = || "-".to_string();
    [
        format!("Daevalog {version}"),
        format!("System: {}", clean(&platform::process::system_name())),
        format!("Desktop: {}", env("XDG_CURRENT_DESKTOP").unwrap_or_else(unknown)),
        format!("Session: {}", env("XDG_SESSION_TYPE").unwrap_or_else(unknown)),
        format!("Display backend: {}", display.map(|v| clean(&v)).unwrap_or_else(unknown)),
        format!("Capture: {}", clean(&crate::capture::live::state())),
    ]
    .join("\n")
}

/// Letters, digits and a few marks, at most 80 characters: what a system,
/// desktop or backend name needs, and no path.
fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric() || " ._-+:,;()=·".contains(*c))
        .take(80)
        .collect::<String>()
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PacketLog {
    pub name: String,
    pub bytes: u64,
}

fn is_packet_log(name: &str) -> bool {
    name.starts_with("packets_") && name.ends_with(".txt") && !name.contains(['/', '\\'])
}

/// The bug-report copy just written: its file name in the data folder, and
/// how many character names it knew to blind.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedLog {
    pub name: String,
    pub names_known: usize,
}

/// The packet logs in the data folder that hold anything, newest name first.
/// Folders and links are passed over.
pub fn packet_logs(data_dir: &Path) -> Vec<PacketLog> {
    let mut logs = Vec::new();
    let Ok(listing) = std::fs::read_dir(data_dir) else {
        return logs;
    };
    for entry in listing.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // `DirEntry::metadata` does not follow a link, so a link is no file here.
        let size = match entry.metadata() {
            Ok(info) if info.is_file() => info.len(),
            _ => continue,
        };
        if size > 0 && is_packet_log(&name) {
            logs.push(PacketLog { name, bytes: size });
        }
    }
    logs.sort_unstable_by(|a, b| b.name.cmp(&a.name));
    logs
}

/// Write `report_<name>`: the packet log `name` (the newest when `None`) with
/// every character name blinded.
pub fn prepare_log(data_dir: &Path, name: Option<&str>) -> Result<PreparedLog, String> {
    let logs = packet_logs(data_dir);
    let log = match name {
        Some(name) => logs.iter().find(|l| l.name == name),
        None => logs.first(),
    }
    .ok_or_else(|| "There is no packet log. Turn on packet logging, play until the problem happens, then try again.".to_string())?;
    let source: PathBuf = data_dir.join(&log.name);
    let text = std::fs::read(&source).map_err(|e| format!("Could not read {}: {e}", log.name))?;
    let text = String::from_utf8_lossy(&text);
    let prepared = report_log::prepare(&text).map_err(|e| format!("Could not prepare {}: {e}", log.name))?;
    // Compressed: GitHub takes attachments up to 25 MB, and a busy log is more.
    let out_name = format!("report_{}.gz", log.name);
    let out = data_dir.join(&out_name);
    let mut file = platform::files::private_options()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&out)
        .map_err(|e| format!("Could not write {out_name}: {e}"))?;
    let gz = crate::share::gzip(prepared.text.as_bytes()).map_err(|e| format!("Could not write {out_name}: {e}"))?;
    std::io::Write::write_all(&mut file, &gz).map_err(|e| format!("Could not write {out_name}: {e}"))?;
    tracing::info!(
        "Prepared {out_name} for a bug report: {} names known, {} places blinded",
        prepared.names_known,
        prepared.names_blinded
    );
    Ok(PreparedLog { name: out_name, names_known: prepared.names_known })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_info_names_nobody() {
        let info = report_info(crate::version::DISPLAY, platform::process::display_backend());
        assert!(info.starts_with("Daevalog "), "{info}");
        assert_eq!(info.lines().count(), 6);
        for name in ["USER", "LOGNAME", "HOSTNAME"] {
            if let Some(value) = std::env::var(name).ok().filter(|v| v.len() >= 3) {
                assert!(!info.contains(&value), "{name} is in the report info");
            }
        }
        if let Some(home) = std::env::var("HOME").ok().filter(|h| h.len() > 1) {
            assert!(!info.contains(&home));
        }
        assert!(!info.contains('/'));
    }

    #[test]
    fn values_keep_no_path() {
        assert_eq!(clean("KDE"), "KDE");
        assert_eq!(clean("x11 (X11 session)"), "x11 (X11 session)");
        assert_eq!(clean("/home/someone/.config"), "homesomeone.config");
        assert_eq!(clean(&"a".repeat(200)).len(), 80);
    }

    #[test]
    fn only_packet_logs_are_listed_and_prepared() {
        let dir = std::env::temp_dir().join(format!("a2t-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let packet = crate::built::to_hex(&crate::built::self_record(3001, "Tester", 1202, 30, 30)).to_uppercase();
        let log = format!("# Packet capture started at x\n\n2026-01-02T12:00:10.000000000+00:00|Client:5000:13328|{packet}\n");
        std::fs::write(dir.join("packets_20260102_120000.txt"), &log).unwrap();
        std::fs::write(dir.join("packets_20260103_101010.txt"), &log).unwrap();
        std::fs::write(dir.join("packets_empty.txt"), "").unwrap();
        std::fs::write(dir.join("debug.log"), "x").unwrap();
        std::fs::write(dir.join("report_packets_20260101_000000.txt"), "x").unwrap();

        let names: Vec<String> = packet_logs(&dir).into_iter().map(|l| l.name).collect();
        assert_eq!(names, ["packets_20260103_101010.txt", "packets_20260102_120000.txt"]);

        let newest = prepare_log(&dir, None).unwrap();
        assert_eq!(newest.name, "report_packets_20260103_101010.txt.gz");
        let mut copy = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(std::fs::File::open(dir.join(&newest.name)).unwrap()), &mut copy).unwrap();
        assert!(copy.contains(report_log::MARK));
        assert!(!copy.contains("546573746572"), "the name is blinded");
        assert!(prepare_log(&dir, Some("debug.log")).is_err());
        assert!(prepare_log(&dir, Some("../packets_20260102_120000.txt")).is_err());
        assert_eq!(prepare_log(&dir, Some("packets_20260102_120000.txt")).unwrap().name, "report_packets_20260102_120000.txt.gz");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listing_passes_over_folders_links_and_a_missing_folder() {
        let dir = std::env::temp_dir().join(format!("daevalog-log-list.{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        assert!(packet_logs(&dir).is_empty());
        std::fs::create_dir_all(dir.join("packets_folder.txt")).unwrap();
        std::fs::write(dir.join("packets_real.txt"), "abc").unwrap();
        let linked = platform::files::symlink(&dir.join("packets_real.txt"), &dir.join("packets_link.txt")).is_ok();
        let logs = packet_logs(&dir);
        assert_eq!(logs, [PacketLog { name: "packets_real.txt".into(), bytes: 3 }], "link made: {linked}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_prepared_log_is_sent_to_the_front_end_in_camel_case() {
        let log = PreparedLog { name: "report_packets_x.txt.gz".into(), names_known: 3 };
        assert_eq!(serde_json::to_string(&log).unwrap(), r#"{"name":"report_packets_x.txt.gz","namesKnown":3}"#);
    }
}
