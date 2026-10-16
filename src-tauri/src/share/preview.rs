//! Cutting a fight's slice from packet captures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::slices::{covers, names_from, strip_party_ids};
use crate::capture::fight_tape::{self, SliceRecord, NameMap, SLICE_HEAD_MS, TAIL_MS};
use crate::capture::byte_queue::ByteQueue;
use crate::capture::packet_log::LogLine;
use crate::capture::decoder::PacketDecoder;
use crate::combat::data_storage::DataStorage;
use crate::entity::fight_record::FightRecord;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// Every payload of a packet log, in the order the logger wrote them.
/// Repeated payloads stay: the slice builder wants each stream as it came.
pub fn load_capture_file(path: &Path) -> Result<Vec<SliceRecord>, String> {
    std::fs::read_to_string(path)
        .map(|text| text.lines().filter_map(as_packet).collect())
        .map_err(|why| format!("{}: {why}", path.display()))
}

/// `None` for comments, blank lines, lines that do not parse, and lines
/// with no payload (the shared reader takes those).
fn as_packet(line: &str) -> Option<SliceRecord> {
    let parsed = LogLine::parse(line.trim()).filter(|l| !l.bytes.is_empty())?;
    Some(SliceRecord { captured_at_ms: parsed.at_ms, stream: parsed.stream.to_string(), bytes: parsed.bytes })
}

/// The packet logs in the app data folder, newest first: their names start
/// with the date and time. A folder that cannot be read has none.
pub fn capture_files(app_data_dir: &Path) -> Vec<PathBuf> {
    let is_log = |name: &str| name.starts_with("packets_") && name.ends_with(".txt");
    let mut logs: Vec<PathBuf> = std::fs::read_dir(app_data_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str().filter(|n| is_log(n)).map(|n| app_data_dir.join(n)))
        .collect();
    logs.sort_unstable_by(|a, b| b.cmp(a));
    logs
}

/// Plays the packets through one parser to learn the names in them, since
/// the saved record masks all but the player's. Unpins the clock after.
/// One parser for all streams, one `decode_buffer` per packet, repeats kept:
/// so not through `Reassembler` (which loops `decode_buffer`),
/// `PacketLogReplay` (which drops repeats) or `report_log::name_table` (a parser
/// per stream). Any of those can learn other names and so change the slice.
fn names_heard(packets: &[SliceRecord]) -> NameMap {
    let storage = Arc::new(DataStorage::new());
    let (skills, npcs) = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
    let mut parser = PacketDecoder::new(Arc::clone(&storage), skills, npcs);
    let mut unread = HashMap::<&str, ByteQueue>::new();
    for SliceRecord { captured_at_ms, stream, bytes } in packets {
        parser.pin_clock(Some(*captured_at_ms));
        let buffer = unread.entry(stream).or_default();
        buffer.append(bytes);
        let framed = parser.decode_buffer(buffer.snapshot());
        buffer.drop_front(framed);
    }
    parser.pin_clock(None);
    names_from(&storage)
}

/// The packets of every capture that reaches the fight's window with the
/// builder's lead-in and tail, by time. A capture goes in whole: the builder
/// rebuilds each stream from its first byte.
fn traffic_for(record: &FightRecord, captures: &[PathBuf]) -> Result<Vec<SliceRecord>, String> {
    let (from, to) = (record.start_time_ms, record.start_time_ms + record.duration_ms);
    let mut traffic = Vec::new();
    for path in captures {
        let mut packets = load_capture_file(path)?;
        if covers(&packets, from - SLICE_HEAD_MS, to + TAIL_MS) {
            traffic.append(&mut packets);
        }
    }
    // Only a capture with packets covers, so none here means none covered.
    if traffic.is_empty() {
        return Err(format!("no packet capture covers fight {}", record.id));
    }
    // Stable, so equal times keep capture then line order.
    traffic.sort_by_key(|p| p.captured_at_ms);
    Ok(traffic)
}

/// The fight's slice cut from the captures, roster ids zeroed, encoded.
pub(super) fn cut_slice(record: &FightRecord, captures: &[PathBuf]) -> Result<Vec<u8>, String> {
    let traffic = traffic_for(record, captures)?;
    let (from, to) = (record.start_time_ms, record.start_time_ms + record.duration_ms);
    let mut slice = fight_tape::build(&traffic, from, to, &names_heard(&traffic)).map_err(|e| e.to_string())?;
    strip_party_ids(&mut slice);
    Ok(fight_tape::encode(&slice))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{self, Scratch};

    /// Late 2023, years from the wall clock, so a pin left behind shows.
    const FIGHT_AT: i64 = 1_700_000_000_000;
    /// Apart from `TAIL_MS`, so the edges below say which boundary they test.
    const FIGHT_MS: i64 = 10_000;
    const STREAM: &str = "Client:5000:13328";

    /// A packet log of `packets` in `folder`, each `(time, payload)` on `STREAM`.
    fn capture(folder: &Scratch, name: &str, packets: &[(i64, Vec<u8>)]) -> PathBuf {
        let mut text = String::from("# Format: TIMESTAMP|STREAMKEY|HEX_DATA\n\n");
        for (at, bytes) in packets {
            let stamp = chrono::DateTime::from_timestamp_millis(*at).expect("a time").to_rfc3339();
            text += &format!("{stamp}|{STREAM}|{}\n", built::to_hex(bytes));
        }
        folder.put(name, text)
    }

    fn hit(n: u32) -> Vec<u8> {
        built::tick(9001, 0, 9002, n, 1, 50, &[])
    }

    /// Hits on 9001 from before the fight to its middle.
    fn fight_traffic() -> Vec<(i64, Vec<u8>)> {
        vec![(FIGHT_AT - 4_000, hit(1)), (FIGHT_AT + 2_000, hit(2)), (FIGHT_AT + 9_000, hit(3))]
    }

    fn stamps(packets: &[SliceRecord]) -> Vec<i64> {
        packets.iter().map(|p| p.captured_at_ms).collect()
    }

    #[test]
    fn log_lines_read_as_packets_in_file_order() {
        let folder = Scratch::new("preview.lines");
        let path = folder.put(
            "packets_20260102_120000.txt",
            "# Packet capture started at 2026-01-02T12:00:00+00:00\r\n\
             # Format: TIMESTAMP|STREAMKEY|HEX_DATA\r\n\
             \r\n\
             2026-01-02T12:00:05.123456789+00:00|Client:13328|0A0b\r\n\
             \x20\x202026-01-02T12:00:05.123456789+00:00|Client:13328|0A0b\t\n\
             2026-01-02T12:00:01.000999999+00:00|Client:5000:13328|ff\n",
        );
        let packets = load_capture_file(&path).unwrap();
        assert_eq!(stamps(&packets), [1_767_355_205_123, 1_767_355_205_123, 1_767_355_201_000]);
        assert_eq!(packets[0].stream, "Client:13328");
        assert_eq!(packets[0].bytes, [0x0A, 0x0B]);
        assert_eq!(packets[1].bytes, [0x0A, 0x0B]);
        assert_eq!((packets[2].stream.as_str(), packets[2].bytes.as_slice()), ("Client:5000:13328", &[0xFF][..]));
    }

    #[test]
    fn log_lines_that_do_not_parse_are_skipped() {
        let folder = Scratch::new("preview.bad-lines");
        let stamp = "2026-01-02T12:00:05.123456789+00:00";
        let lines = [
            format!("{stamp}|Client:13328|"),
            format!("{stamp}|Client:13328|0A0"),
            format!("{stamp}|Client:13328|0G"),
            format!("{stamp}|Client:13328"),
            "yesterday|Client:13328|0A0B".to_string(),
            "#2026-01-02T12:00:05.123456789+00:00|Client:13328|0A0B".to_string(),
            "   ".to_string(),
            format!("{stamp}|Client:13328|0C"),
        ];
        let path = folder.put("packets_x.txt", lines.join("\n"));
        let packets = load_capture_file(&path).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].bytes, [0x0C]);
    }

    #[test]
    fn an_unreadable_log_is_an_error_naming_it() {
        let err = load_capture_file(Path::new("/tmp/x/packets_none.txt")).unwrap_err();
        assert!(err.starts_with("/tmp/x/packets_none.txt: "), "{err}");
        let folder = Scratch::new("preview.not-text");
        let path = folder.put("packets_bin.txt", [0xC3u8, 0x28, b'\n']);
        let err = load_capture_file(&path).unwrap_err();
        assert!(err.starts_with(&format!("{}: ", path.display())), "{err}");
    }

    #[test]
    fn packet_logs_are_listed_newest_first() {
        let folder = Scratch::new("preview.list");
        for name in ["packets_20260102_120000.txt", "packets_20260103_090000_scarecrow.txt", "report_packets_x.txt", "debug.log", "packets_x.log"] {
            folder.put(name, "");
        }
        assert_eq!(
            capture_files(&folder.0),
            [folder.0.join("packets_20260103_090000_scarecrow.txt"), folder.0.join("packets_20260102_120000.txt")]
        );
        assert!(capture_files(&folder.0.join("absent")).is_empty());
    }

    #[test]
    fn captures_count_when_they_reach_the_lead_in_or_the_tail() {
        let folder = Scratch::new("preview.edges");
        let (lead_in, tail) = (FIGHT_AT - SLICE_HEAD_MS, FIGHT_AT + FIGHT_MS + TAIL_MS);
        let early = capture(&folder, "packets_1.txt", &[(lead_in - 30_000, hit(7)), (lead_in, hit(8))]);
        let too_early = capture(&folder, "packets_2.txt", &[(lead_in - 1, hit(7))]);
        let core = capture(&folder, "packets_3.txt", &fight_traffic());
        let late = capture(&folder, "packets_4.txt", &[(tail, hit(9))]);
        let too_late = capture(&folder, "packets_5.txt", &[(tail + 1, hit(9))]);
        let given = [late, too_early, core, too_late, early];
        // Also the case of captures that do not cover beside one that does:
        // only the covering ones go in, each whole.
        let traffic = traffic_for(&built::fight("auto_1_2", FIGHT_AT, FIGHT_MS), &given).unwrap();
        let core_at = fight_traffic().iter().map(|(at, _)| *at).collect::<Vec<_>>();
        assert_eq!(stamps(&traffic), [&[lead_in - 30_000, lead_in][..], &core_at, &[tail]].concat());
    }

    #[test]
    fn a_capture_that_cannot_be_read_ends_the_cut() {
        let folder = Scratch::new("preview.unreadable");
        let missing = folder.0.join("packets_gone.txt");
        let good = capture(&folder, "packets_b.txt", &fight_traffic());
        let err = cut_slice(&built::fight("auto_1_2", FIGHT_AT, FIGHT_MS), &[missing.clone(), good]).unwrap_err();
        assert!(err.starts_with(&format!("{}: ", missing.display())), "{err}");
    }

    #[test]
    fn no_covering_capture_is_an_error() {
        let folder = Scratch::new("preview.uncovered");
        let far = capture(&folder, "packets_far.txt", &[(FIGHT_AT - 500_000, hit(1)), (FIGHT_AT + 500_000, hit(2))]);
        let empty = folder.put("packets_empty.txt", "");
        assert!(cut_slice(&built::fight("auto_1_2", FIGHT_AT + 600_000, FIGHT_MS), &[far, empty]).is_err());
        assert!(cut_slice(&built::fight("auto_1_2", FIGHT_AT, FIGHT_MS), &[]).is_err());
    }

    #[test]
    fn names_come_from_replaying_the_captures_and_lose_their_ids() {
        use built::{Member, Slot};
        let member = Member { mask: 0x1e, slot: 1, dbid: 77, name: "Abcd", class: 26, level: 45, gear: 800, extra: &[], tag: 4, combat_power: 40_000, tail: &[0x00, 0x01, 0x02] };
        let roster = built::roster(0x4000, "Party 3", 600_031, 77, &[Slot::Member(member)]);
        let other = built::player_record(9003, Some("Efgh"), &[]);
        let mut traffic = vec![(FIGHT_AT - 30_000, roster), (FIGHT_AT - 20_000, other)];
        traffic.extend(fight_traffic());
        let folder = Scratch::new("preview.names");
        let log = capture(&folder, "packets_names.txt", &traffic);

        let encoded = cut_slice(&built::fight("auto_1_2", FIGHT_AT, FIGHT_MS), &[log.clone()]).unwrap();

        // The same cut with the names known up front: Abcd's id reaches the
        // map before it is zeroed, and the bytes match.
        let packets = load_capture_file(&log).unwrap();
        let cut = |names: &NameMap| fight_tape::build(&packets, FIGHT_AT, FIGHT_AT + FIGHT_MS, names).unwrap();
        let mut known = cut(&NameMap::from([("Abcd".to_string(), 77), ("Efgh".to_string(), 0)]));
        assert!(known.blind_map.values().any(|&id| id == 77));
        strip_party_ids(&mut known);
        assert_eq!(encoded, fight_tape::encode(&known));
        assert_ne!(encoded, fight_tape::encode(&cut(&NameMap::new())));
        for name in [&b"Abcd"[..], b"Efgh"] {
            assert!(!encoded.windows(name.len()).any(|w| w == name));
        }
        assert!((crate::clock::now_ms() - crate::clock::system_ms()).abs() < 60_000, "the clock is still pinned");
    }
}
