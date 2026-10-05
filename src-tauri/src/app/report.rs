//! "Report a problem": the report info a bug report starts with, and the
//! bug-report copy of a packet log. Nothing here sends anything: the player
//! opens the issue form, and attaches what they choose.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::capture::report_log;
use crate::platform;

/// The few facts every issue form asks for, one per line. Nothing that names
/// the player or the computer: no user or character names, no paths, no
/// host name.
pub(crate) fn report_info() -> String {
    let env = |name: &str| std::env::var(name).ok().map(|v| clean(&v)).filter(|v| !v.is_empty());
    let unknown = || "-".to_string();
    [
        format!("Daevalog {}", crate::version::DISPLAY),
        format!("System: {}", clean(&platform::process::system_name())),
        format!("Desktop: {}", env("XDG_CURRENT_DESKTOP").unwrap_or_else(unknown)),
        format!("Session: {}", env("XDG_SESSION_TYPE").unwrap_or_else(unknown)),
        format!("Display backend: {}", platform::process::display_backend().map(|v| clean(&v)).unwrap_or_else(unknown)),
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
pub(crate) struct PacketLog {
    pub name: String,
    pub bytes: u64,
}

fn is_packet_log(name: &str) -> bool {
    name.starts_with("packets_") && name.ends_with(".txt") && !name.contains(['/', '\\'])
}

/// The packet logs in the data folder, newest first. Names are time stamps,
/// so name order is time order.
pub(crate) fn packet_logs(data_dir: &Path) -> Vec<PacketLog> {
    let Ok(entries) = std::fs::read_dir(data_dir) else { return Vec::new() };
    let mut logs: Vec<PacketLog> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let bytes = e.metadata().ok()?.len();
            (is_packet_log(&name) && bytes > 0).then_some(PacketLog { name, bytes })
        })
        .collect();
    logs.sort_by(|a, b| b.name.cmp(&a.name));
    logs
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedLog {
    /// The copy's file name, in the data folder.
    pub name: String,
    pub names_known: usize,
}

/// Write `report_<name>`: the packet log `name` (the newest when `None`) with
/// every character name blinded.
pub(crate) fn prepare_log(data_dir: &Path, name: Option<&str>) -> Result<PreparedLog, String> {
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
    let out_name = format!("report_{}", log.name);
    let out = data_dir.join(&out_name);
    let mut file = platform::files::private_options()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&out)
        .map_err(|e| format!("Could not write {out_name}: {e}"))?;
    std::io::Write::write_all(&mut file, prepared.text.as_bytes()).map_err(|e| format!("Could not write {out_name}: {e}"))?;
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
        let info = report_info();
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
        let record = "3336ed745e91c12837064e616963686118051e000000011c0000007f0100007f0100001c000000d002040000000000";
        let packet = format!("{:02X}{}", record.len() / 2 + 4, record.to_uppercase());
        let log = format!("# Packet capture started at x\n\n2026-10-04T00:42:10.000000000-07:00|Client:5000:13328|{packet}\n");
        std::fs::write(dir.join("packets_20261004_004203.txt"), &log).unwrap();
        std::fs::write(dir.join("packets_20261005_101010.txt"), &log).unwrap();
        std::fs::write(dir.join("packets_empty.txt"), "").unwrap();
        std::fs::write(dir.join("debug.log"), "x").unwrap();
        std::fs::write(dir.join("report_packets_20261001_000000.txt"), "x").unwrap();

        let names: Vec<String> = packet_logs(&dir).into_iter().map(|l| l.name).collect();
        assert_eq!(names, ["packets_20261005_101010.txt", "packets_20261004_004203.txt"]);

        let newest = prepare_log(&dir, None).unwrap();
        assert_eq!(newest.name, "report_packets_20261005_101010.txt");
        let copy = std::fs::read_to_string(dir.join(&newest.name)).unwrap();
        assert!(copy.contains(report_log::MARK));
        assert!(!copy.contains("4E6169636861"), "the name is blinded");
        assert!(prepare_log(&dir, Some("debug.log")).is_err());
        assert!(prepare_log(&dir, Some("../packets_20261004_004203.txt")).is_err());
        assert_eq!(prepare_log(&dir, Some("packets_20261004_004203.txt")).unwrap().name, "report_packets_20261004_004203.txt");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
