//! Packet logs (`packets_*.txt`) fed back through the parser.
//!
//! A packet log holds one captured payload per line, `TIMESTAMP|STREAM|HEX`,
//! the timestamp in RFC 3339 local time. Live capture keeps one assembler and
//! one parser per connection, so a replay keeps one per stream, and parses each
//! payload at the time it was captured. The replay report and the Qt
//! meter's `--replay` both go through here.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use super::decoder::PacketDecoder;
use super::reassembly::Reassembler;
use crate::{combat::data_storage::DataStorage, i18n::lookup::{NpcLookup, SkillLookup}};

/// One payload of a packet log.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine<'a> {
    /// The timestamp as written.
    pub stamp: &'a str,
    pub stream: &'a str,
    /// The capture time, unix ms.
    pub at_ms: i64,
    pub bytes: Vec<u8>,
}

impl<'a> LogLine<'a> {
    /// `None` for a blank line, a `#` comment, or a line that does not parse.
    pub fn parse(line: &'a str) -> Option<Self> {
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut parts = line.splitn(3, '|');
        let (stamp, stream, hex) = (parts.next()?, parts.next()?, parts.next()?);
        let bytes = bytes_from_hex(hex)?;
        let at_ms = chrono::DateTime::parse_from_rfc3339(stamp).ok()?.timestamp_millis();
        Some(Self { stamp, stream, at_ms, bytes })
    }

    /// The local time of day as written: `HH:MM:SS`, the fraction and the
    /// offset.
    pub fn time_of_day(&self) -> &'a str {
        self.stamp.get(11..).unwrap_or("")
    }

    /// The time zone the timestamp was written in.
    pub fn offset(&self) -> Option<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339(self.stamp).ok().map(|when| *when.offset())
    }
}

/// The bytes a hex string spells, two digits per byte, either case. Space at
/// the ends is trimmed; anything else that is not a digit pair gives `None`.
pub(crate) fn bytes_from_hex(hex: &str) -> Option<Vec<u8>> {
    let digits = hex.trim().as_bytes();
    if digits.len() % 2 == 1 {
        return None;
    }
    let nibble = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
    let (pairs, _) = digits.as_chunks::<2>();
    pairs.iter().map(|&[high, low]| Some(nibble(high)? << 4 | nibble(low)?)).collect()
}

/// Feeds packet-log lines into a `DataStorage`.
pub struct PacketLogReplay {
    storage: Arc<DataStorage>,
    skills: Arc<SkillLookup>,
    npcs: Arc<NpcLookup>,
    dot_skill_ids: HashSet<i32>,
    streams: HashMap<String, (Reassembler, PacketDecoder)>,
    repeats: RepeatFilter,
}

/// A packet log holds no TCP sequence numbers, and logs written before the
/// meter put the stream in order hold every segment the server sent again.
/// A payload of this many bytes or more, the same as one on its stream a
/// moment before, is taken for such a repeat: real same-stream repeats
/// within minutes are 7 bytes or shorter, while resends are hundreds of
/// bytes and come within a few seconds.
const REPEAT_MIN_BYTES: usize = 32;
const REPEAT_WITHIN_MS: i64 = 5_000;

/// Recent long payloads per stream, to tell a repeat by.
#[derive(Default)]
pub struct RepeatFilter {
    recent: HashMap<String, VecDeque<(i64, Vec<u8>)>>,
}

impl RepeatFilter {
    /// True when this payload is a copy of one just before it on its stream.
    pub fn is_repeat(&mut self, line: &LogLine) -> bool {
        if line.bytes.len() < REPEAT_MIN_BYTES {
            return false;
        }
        let recent = self.recent.entry(line.stream.to_string()).or_default();
        while recent.front().is_some_and(|(at, _)| line.at_ms - at > REPEAT_WITHIN_MS) {
            recent.pop_front();
        }
        if recent.iter().any(|(_, bytes)| *bytes == line.bytes) {
            return true;
        }
        recent.push_back((line.at_ms, line.bytes.clone()));
        false
    }
}

impl PacketLogReplay {
    pub fn new(
        storage: Arc<DataStorage>,
        skills: Arc<SkillLookup>,
        npcs: Arc<NpcLookup>,
        dot_skill_ids: HashSet<i32>,
    ) -> Self {
        Self { storage, skills, npcs, dot_skill_ids, streams: HashMap::new(), repeats: RepeatFilter::default() }
    }

    /// Parses one line on its stream, at its capture time; a repeat is
    /// dropped unparsed. True when the stream holds no partial frame after.
    pub fn feed(&mut self, line: &LogLine) -> bool {
        if self.repeats.is_repeat(line) {
            return self.streams.get(line.stream).is_none_or(|(asm, _)| asm.pending_len() == 0);
        }
        if !self.streams.contains_key(line.stream) {
            let mut parser =
                PacketDecoder::new(self.storage.clone(), self.skills.clone(), self.npcs.clone());
            parser.use_tick_skills(self.dot_skill_ids.clone());
            self.streams.insert(line.stream.to_string(), (Reassembler::new(), parser));
        }
        let (asm, parser) = self.streams.get_mut(line.stream).expect("just added");
        parser.pin_clock(Some(line.at_ms));
        asm.push_bytes(&line.bytes, parser);
        asm.pending_len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_parses_to_its_capture_time_stream_and_bytes() {
        let line = LogLine::parse("2026-01-02T12:00:48.495+09:00|Client:61234:16005|0a0Bff").unwrap();
        assert_eq!(line.at_ms, 1_767_322_848_495);
        assert_eq!(line.stream, "Client:61234:16005");
        assert_eq!(line.bytes, vec![0x0a, 0x0b, 0xff]);
        assert_eq!(line.time_of_day(), "12:00:48.495+09:00");
        assert_eq!(line.offset(), chrono::FixedOffset::east_opt(9 * 3600));
    }

    #[test]
    fn comments_blank_and_broken_lines_are_skipped() {
        assert_eq!(LogLine::parse(""), None);
        assert_eq!(LogLine::parse("# started"), None);
        assert_eq!(LogLine::parse("2026-01-02T12:00:48.495+00:00|s"), None);
        assert_eq!(LogLine::parse("2026-01-02T12:00:48.495+00:00|s|abc"), None);
        assert_eq!(LogLine::parse("yesterday|s|abcd"), None);
    }

    #[test]
    fn a_long_payload_sent_again_is_dropped() {
        use crate::built::{packet, stats, to_hex, F};
        let long = to_hex(&stats(2_800, &[(65, 500), (76, 0), (379, 1000), (380, 0)]));
        assert!(long.len() / 2 >= 32);
        let short = to_hex(&packet([0x6E, 0x56], &[F::U32(3)]));
        let at = |time: &str, stream: &str, hex: &str| format!("2026-01-02T12:00:{time}+00:00|{stream}|{hex}");
        let mut filter = RepeatFilter::default();
        let mut check = |line: String| filter.is_repeat(&LogLine::parse(&line).unwrap());
        assert!(!check(at("37.700", "Client:50000:13328", &long)));
        assert!(check(at("38.150", "Client:50000:13328", &long)), "the resend");
        assert!(!check(at("38.200", "Client:50001:13328", &long)), "another stream");
        assert!(!check(at("43.000", "Client:50000:13328", &long)), "too long after");
        // Short payloads do repeat in the game's own traffic.
        assert!(!check(at("44.000", "Client:50000:13328", &short)));
        assert!(!check(at("44.500", "Client:50000:13328", &short)));
    }

    #[test]
    fn the_clock_stays_at_the_last_payload() {
        let mut replay = PacketLogReplay::new(Arc::new(DataStorage::new()), Arc::new(SkillLookup::new()),
            Arc::new(NpcLookup::new()), HashSet::new());
        let line = LogLine::parse("2026-01-02T12:00:48.495+00:00|s|00").unwrap();
        replay.feed(&line);
        assert_eq!(crate::clock::now_ms(), line.at_ms);
        crate::clock::set_override(None);
    }

    #[test]
    fn hex_takes_digit_pairs_of_either_case_and_nothing_else() {
        assert_eq!(bytes_from_hex("0a0Bff"), Some(vec![0x0A, 0x0B, 0xFF]));
        assert_eq!(bytes_from_hex(" 0A\n"), Some(vec![0x0A]));
        assert_eq!(bytes_from_hex(""), Some(vec![]));
        for bad in ["abc", "0G", "+1", "é0", "éé"] {
            assert_eq!(bytes_from_hex(bad), None, "{bad:?}");
        }
    }

    fn empty_replay() -> PacketLogReplay {
        PacketLogReplay::new(Arc::new(DataStorage::new()), Arc::new(SkillLookup::new()),
            Arc::new(NpcLookup::new()), HashSet::new())
    }

    #[test]
    fn a_frame_split_over_two_lines_is_whole_after_the_second() {
        let frame = crate::built::frame(&[0x99; 26]);
        assert_eq!(frame[0], 30, "the length varint");
        let (head, rest) = frame.split_at(10);
        let mut replay = empty_replay();
        let line = |hex: String| format!("2026-01-02T12:00:48.495+00:00|Server:1:2|{hex}");
        let first = line(crate::built::to_hex(head));
        let second = line(crate::built::to_hex(rest));
        assert!(!replay.feed(&LogLine::parse(&first).unwrap()));
        assert!(replay.feed(&LogLine::parse(&second).unwrap()));
        crate::clock::set_override(None);
    }

    #[test]
    fn a_repeat_on_a_new_stream_counts_as_clean() {
        let long = "ab".repeat(40);
        let mut replay = empty_replay();
        let a = format!("2026-01-02T12:00:48.495+00:00|one|{long}");
        let b = format!("2026-01-02T12:00:48.600+00:00|one|{long}");
        replay.repeats.is_repeat(&LogLine::parse(&a).unwrap());
        let before = crate::clock::now_ms();
        assert!(replay.feed(&LogLine::parse(&b).unwrap()));
        assert!(replay.streams.is_empty(), "nothing parsed");
        assert!(crate::clock::now_ms() >= before, "the clock is not pinned");
    }
}
