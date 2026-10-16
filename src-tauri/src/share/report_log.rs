//! A copy of a packet log for a bug report, with every character name blinded.
//!
//! Issues are public, and a packet log holds the names of everyone the player
//! met. The copy blinds names the same way an Evidence Slice does (the
//! slice's `Blinder`: the names the capture itself resolves, then anything
//! shaped like a name), and keeps everything else, so the replay tools read
//! the copy as they read the original and count the same damage.
//!
//! The copy is framed again per line: each line holds the packets that the
//! original line completed, so the parser reads the same packets at the same
//! time stamps. A packet keeps its bytes but for the names in it. A bundle
//! whose contents had a name is compressed again, so its bytes change but
//! not what it holds. Damage, damage over time and HP records hold no names
//! and are copied as they are.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use crate::capture::fight_tape::{carries_event, wrap_record, pack_bundle, Blinder, NameMap};
use crate::capture::framing::{self, ChunkForm, NEST_LIMIT};
use crate::capture::byte_queue::ByteQueue;
use crate::capture::packet_log::LogLine;
use crate::capture::reassembly::Reassembler;
use crate::capture::decoder::PacketDecoder;
use crate::capture::varint::varint_at;
use crate::combat::data_storage::DataStorage;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// The comment line a prepared copy starts its data with.
pub const MARK: &str = "# Character names blinded for a bug report";

/// One packet line of a log: its time stamp and stream as written, the time
/// in Unix ms, and the bytes.
pub struct Line<'a> {
    pub ts: &'a str,
    pub key: &'a str,
    pub at_ms: i64,
    pub bytes: Vec<u8>,
}

/// A line of a packet log that is worth keeping.
enum LogItem<'a> {
    Note(&'a str),
    Packet(Line<'a>),
}

/// The comments and packet lines of a log, in order. Blank lines and lines
/// that do not read as `TS|KEY|HEX` are passed over.
fn read_log(text: &str) -> impl Iterator<Item = LogItem<'_>> {
    text.lines().filter(|raw| !raw.is_empty()).filter_map(|raw| {
        if raw.starts_with('#') {
            Some(LogItem::Note(raw))
        } else {
            packet_line(raw).map(LogItem::Packet)
        }
    })
}

fn packet_line(raw: &str) -> Option<Line<'_>> {
    let LogLine { stamp, stream, at_ms, bytes } = LogLine::parse(raw)?;
    Some(Line { ts: stamp, key: stream, at_ms, bytes })
}

impl<'a> LogItem<'a> {
    fn line(&self) -> Option<&Line<'a>> {
        match self {
            LogItem::Packet(line) => Some(line),
            LogItem::Note(_) => None,
        }
    }
}

/// The packet lines of a packet log.
pub fn lines(text: &str) -> Vec<Line<'_>> {
    read_log(text)
        .filter_map(|item| match item {
            LogItem::Packet(line) => Some(line),
            LogItem::Note(_) => None,
        })
        .collect()
}

/// Runs a stream's waiting bytes through the framing, handing `take` each
/// stretch a walk consumed with its frames, until a walk consumes nothing.
fn drain(pending: &mut ByteQueue, mut take: impl FnMut(&[u8], &[framing::Frame])) {
    while pending.size() > 0 {
        let held = pending.snapshot();
        let walked = framing::walk(held);
        if walked.consumed == 0 {
            break;
        }
        take(&held[..walked.consumed], &walked.frames);
        pending.drop_front(walked.consumed);
    }
}

/// Every character name the log resolves, parsed the way the meter parses
/// it (one parser per stream, one shared storage, each line at its own
/// time). A zone change clears names from the storage, so they are gathered
/// every 200 lines as well as at the end. Not through `PacketLogReplay` on
/// purpose: its `RepeatFilter` drops repeated payloads, and a name may be in one.
pub fn name_table(lines: &[&Line]) -> NameMap {
    const GATHER_EVERY: usize = 200;
    let storage = Arc::new(DataStorage::new());
    // No skill or NPC names: only character names matter here.
    let tables = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
    let mut parsers: HashMap<&str, (Reassembler, PacketDecoder)> = HashMap::new();
    let mut found = NameMap::new();
    for (n, line) in lines.iter().enumerate() {
        let (assembler, processor) = parsers.entry(line.key).or_insert_with(|| {
            let parser = PacketDecoder::new(Arc::clone(&storage), Arc::clone(&tables.0), Arc::clone(&tables.1));
            (Reassembler::new(), parser)
        });
        processor.pin_clock(Some(line.at_ms));
        assembler.push_bytes(&line.bytes, processor);
        if (n + 1) % GATHER_EVERY == 0 {
            gather(&storage, &mut found);
        }
    }
    gather(&storage, &mut found);
    found.retain(|name, _| !name.is_empty());
    found
}

/// Adds what the storage knows now. A roster id beats a 0, never the reverse.
fn gather(storage: &DataStorage, found: &mut NameMap) {
    for (name, member) in storage.roster_entries() {
        let id = found.entry(name).or_insert(0);
        if member.dbid != 0 {
            *id = member.dbid;
        }
    }
    for name in storage.all_names().into_values().chain(storage.local_character_name()) {
        found.entry(name).or_insert(0);
    }
}

/// Why no copy was made.
#[derive(Debug, PartialEq, Eq)]
pub enum PrepareError {
    /// The log holds no packet line.
    Empty,
    /// The finished copy still held a resolved name.
    NameLeft,
}

/// Shown to the player after "Could not prepare <log>: ".
impl std::fmt::Display for PrepareError {
    fn fmt(&self, out: &mut std::fmt::Formatter) -> std::fmt::Result {
        let text = match self {
            Self::Empty => "the packet log has no packets",
            Self::NameLeft => "a character name was still in the copy, so no copy was written",
        };
        out.write_str(text)
    }
}

/// A bug-report copy, ready to write.
#[derive(Debug)]
pub struct Prepared {
    pub text: String,
    /// Character names the original resolves.
    pub names_known: usize,
    /// Places in the copy where a name was blinded.
    pub names_blinded: usize,
}

/// The bug-report copy of a packet log: the same lines, with every name the
/// log resolves (and anything shaped like one) blinded. Refused when the log
/// has no packets, or when a name is still in the copy afterwards.
pub fn prepare(text: &str) -> Result<Prepared, PrepareError> {
    let items: Vec<LogItem> = read_log(text).collect();
    let packets: Vec<&Line> = items.iter().filter_map(LogItem::line).collect();
    if packets.is_empty() {
        return Err(PrepareError::Empty);
    }
    let names = name_table(&packets);
    let mut copier = Copier { blinder: Blinder::for_report(&names), blinded: 0 };
    let mut streams: HashMap<&str, ByteQueue> = HashMap::new();
    let mut text_out = String::with_capacity(text.len());
    let mut marked = false;
    for item in &items {
        let Some(line) = item.line() else {
            if let LogItem::Note(note) = item {
                text_out.push_str(note);
                text_out.push('\n');
            }
            continue;
        };
        if !marked {
            text_out.push_str(MARK);
            text_out.push('\n');
            marked = true;
        }
        let pending = streams.entry(line.key).or_default();
        pending.append(&line.bytes);
        let mut done = Vec::new();
        drain(pending, |region, frames| copier.region(region, frames, &mut done));
        if done.is_empty() {
            continue;
        }
        text_out.push_str(line.ts);
        text_out.push('|');
        text_out.push_str(line.key);
        text_out.push('|');
        for byte in &done {
            let _ = write!(text_out, "{byte:02X}");
        }
        text_out.push('\n');
    }
    if names_left(&text_out, &names) {
        return Err(PrepareError::NameLeft);
    }
    Ok(Prepared { text: text_out, names_known: names.len(), names_blinded: copier.blinded })
}

/// The bytes of a log as a reader would look for names in them: each packet,
/// each bundle's contents opened up, and the bytes between frames, as
/// separate pieces. Damage, DoT and HP records are left out unless `events`.
pub fn readable(text: &str, events: bool) -> Vec<Vec<u8>> {
    let mut pieces = Vec::new();
    let mut streams: HashMap<&str, ByteQueue> = HashMap::new();
    for line in lines(text) {
        let pending = streams.entry(line.key).or_default();
        pending.append(&line.bytes);
        drain(pending, |region, frames| {
            let mut walker = Readable { events, out: &mut pieces };
            let mut at = 0;
            for frame in frames {
                walker.out.push(region[at..frame.start].to_vec());
                walker.frame(region, frame, 1);
                at = frame.end;
            }
            walker.out.push(region[at..].to_vec());
        });
    }
    pieces.retain(|piece| !piece.is_empty());
    pieces
}

struct Copier {
    blinder: Blinder,
    blinded: usize,
}

impl Copier {
    fn unframed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        let mut copy = bytes.to_vec();
        self.blinded += self.blinder.blind_unframed(&mut copy);
        out.extend_from_slice(&copy);
    }

    /// One packet, at any depth. A bundle inside it is blinded on its own and
    /// compressed again only when a name was in it; the packet's length is
    /// written again when the bundle's changed. Past `NEST_LIMIT` its bundles
    /// stay shut and the packet is blinded as one.
    fn packet(&mut self, packet: &[u8], depth: usize, out: &mut Vec<u8>) {
        if carries_event(packet) {
            out.extend_from_slice(packet);
            return;
        }
        let mut copy = packet.to_vec();
        self.blinded += self.blinder.blind(&mut copy);
        let embedded = if depth <= NEST_LIMIT { framing::embedded_bundles(packet) } else { Vec::new() };
        if embedded.is_empty() {
            out.extend_from_slice(&copy);
            return;
        }
        let mut rebuilt = Vec::with_capacity(copy.len());
        let mut at = 0;
        for bundle in embedded {
            rebuilt.extend_from_slice(&copy[at..bundle.start]);
            let inner = self.inner(&bundle.data, depth + 1);
            match (inner != bundle.data).then(|| pack_bundle(&inner)).flatten() {
                Some(frame) => rebuilt.extend_from_slice(&frame),
                None => rebuilt.extend_from_slice(&packet[bundle.start..bundle.end]),
            }
            at = bundle.end;
        }
        rebuilt.extend_from_slice(&copy[at..]);
        if rebuilt.len() == copy.len() {
            out.extend_from_slice(&rebuilt);
            return;
        }
        let header = varint_at(&rebuilt, 0);
        match wrap_record(&rebuilt[header.length.max(0) as usize..]) {
            Some(framed) => out.extend_from_slice(&framed),
            None => out.extend_from_slice(&rebuilt),
        }
    }

    /// A bundle's contents: packets, nested bundles, and whatever does not
    /// frame, all blinded.
    fn inner(&mut self, data: &[u8], depth: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let walk = framing::split_frames(data);
        let mut at = 0;
        for frame in &walk.frames {
            self.unframed(&data[at..frame.start], &mut out);
            self.frame(data, frame, depth, &mut out);
            at = frame.end;
        }
        self.unframed(&data[at..], &mut out);
        out
    }

    fn frame(&mut self, buffer: &[u8], frame: &framing::Frame, depth: usize, out: &mut Vec<u8>) {
        let bytes = frame.bytes(buffer);
        match frame.kind {
            ChunkForm::Packet => self.packet(bytes, depth, out),
            ChunkForm::Bundle => match framing::inflate_block(frame.payload(buffer)).filter(|_| depth <= NEST_LIMIT) {
                Some(data) => {
                    let inner = self.inner(&data, depth + 1);
                    match (inner != data).then(|| pack_bundle(&inner)).flatten() {
                        Some(framed) => out.extend_from_slice(&framed),
                        None => out.extend_from_slice(bytes),
                    }
                }
                None => self.unframed(bytes, out),
            },
        }
    }

    /// What one walk of a stream consumed: its frames, and the bytes between
    /// them that the walk skipped.
    fn region(&mut self, region: &[u8], frames: &[framing::Frame], out: &mut Vec<u8>) {
        let mut at = 0;
        for frame in frames {
            self.unframed(&region[at..frame.start], out);
            self.frame(region, frame, 1, out);
            at = frame.end;
        }
        self.unframed(&region[at..], out);
    }
}

/// Shortest name the check looks for. A one- or two-byte name turns up in
/// any long run of binary by chance; the blinder still blinds those.
pub const MIN_CHECKED_NAME: usize = 3;

struct Readable<'a> {
    events: bool,
    out: &'a mut Vec<Vec<u8>>,
}

impl Readable<'_> {
    fn packet(&mut self, bytes: &[u8], depth: usize) {
        if !self.events && carries_event(bytes) {
            return;
        }
        self.out.push(bytes.to_vec());
        if depth <= NEST_LIMIT {
            for bundle in framing::embedded_bundles(bytes) {
                self.inner(&bundle.data, depth + 1);
            }
        }
    }

    fn inner(&mut self, data: &[u8], depth: usize) {
        let walk = framing::split_frames(data);
        let mut at = 0;
        for frame in &walk.frames {
            self.out.push(data[at..frame.start].to_vec());
            self.frame(data, frame, depth);
            at = frame.end;
        }
        self.out.push(data[at..].to_vec());
    }

    fn frame(&mut self, buffer: &[u8], frame: &framing::Frame, depth: usize) {
        match frame.kind {
            ChunkForm::Packet => self.packet(frame.bytes(buffer), depth),
            ChunkForm::Bundle => match framing::inflate_block(frame.payload(buffer)).filter(|_| depth <= NEST_LIMIT) {
                Some(data) => self.inner(&data, depth + 1),
                None => self.out.push(frame.bytes(buffer).to_vec()),
            },
        }
    }
}

/// Whether any resolved name of `MIN_CHECKED_NAME` bytes or more is still in
/// the readable bytes of `text`.
fn names_left(text: &str, names: &NameMap) -> bool {
    let needles: Vec<&[u8]> = names.keys().map(|n| n.as_bytes()).filter(|n| n.len() >= MIN_CHECKED_NAME).collect();
    readable(text, false)
        .iter()
        .any(|buf| needles.iter().any(|n| buf.len() >= n.len() && buf.windows(n.len()).any(|w| w == *n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// A self record (`33 36`) naming "Tester".
    fn self_record() -> Vec<u8> {
        crate::built::self_record(3001, "Tester", 1202, 30, 30)
    }

    fn log(lines: &[Vec<u8>]) -> String {
        let mut text = String::from("# Packet capture started at 2026-01-02T12:00:00+00:00\n# Format: TIMESTAMP|STREAMKEY|HEX_DATA\n\n");
        for (i, bytes) in lines.iter().enumerate() {
            text.push_str(&format!("2026-01-02T12:00:{:02}.000000000+00:00|Client:5000:13328|{}\n", 10 + i, hex(bytes)));
        }
        text
    }

    #[test]
    fn a_name_in_a_packet_is_blinded_and_the_rest_kept() {
        let record = self_record();
        let prepared = prepare(&log(&[record.clone()])).unwrap();
        assert!(prepared.text.contains(MARK));
        assert!(!prepared.text.contains(&hex(b"Tester")));
        let copy = lines(&prepared.text);
        assert_eq!(copy.len(), 1);
        assert_eq!(copy[0].bytes.len(), record.len());
        let at = record.windows(6).position(|w| w == b"Tester").unwrap();
        for (i, (a, b)) in record.iter().zip(&copy[0].bytes).enumerate() {
            if !(at..at + 6).contains(&i) {
                assert_eq!(a, b, "byte {i} outside the name changed");
            }
        }
    }

    #[test]
    fn a_name_inside_a_bundle_is_blinded() {
        let bundle = pack_bundle(&self_record()).unwrap();
        let prepared = prepare(&log(&[bundle])).unwrap();
        let readable = readable(&prepared.text, false);
        assert!(!readable.iter().any(|b| b.windows(6).any(|w| w == b"Tester")));
        assert!(readable.iter().any(|b| b.len() == self_record().len()), "the record is still there, blinded");
    }

    #[test]
    fn a_name_inside_a_bundle_inside_a_packet_is_blinded() {
        let mut host = vec![0x00, 0x36, 0x01, 0x02, 0x03];
        host.extend(pack_bundle(&self_record()).unwrap());
        host.extend([0x04, 0x05]);
        let packet = wrap_record(&host).unwrap();
        let original = readable(&log(&[packet.clone()]), false);
        assert!(original.iter().any(|b| b.windows(6).any(|w| w == b"Tester")), "the test finds the name in the original");
        let prepared = prepare(&log(&[packet])).unwrap();
        assert!(!readable(&prepared.text, false).iter().any(|b| b.windows(6).any(|w| w == b"Tester")));
    }

    /// A packet holding a bundle of a named record and the next packet in,
    /// `levels` deep.
    fn chain(levels: usize) -> Vec<u8> {
        use crate::built::{bundle, packet, F};
        let mut packet_now = self_record();
        for _ in 0..levels {
            let held = bundle(&[self_record(), packet_now].concat());
            packet_now = packet([0x00, 0x36], &[F::Raw(&[1, 2, 3]), F::Raw(&held), F::Raw(&[4, 5])]);
        }
        packet_now
    }

    #[test]
    fn bundles_inside_packets_stop_at_the_nest_limit() {
        let deep = chain(12);
        let original = readable(&log(&[deep.clone()]), false);
        assert!(original.iter().any(|b| b.windows(6).any(|w| w == b"Tester")), "the test finds a name in the original");
        let prepared = prepare(&log(&[deep.clone()])).unwrap();
        let copy = readable(&prepared.text, false);
        assert_eq!(copy.len(), original.len(), "the same bundles are opened");
        assert!(!copy.iter().any(|b| b.windows(6).any(|w| w == b"Tester")));

        let mut copier = Copier { blinder: Blinder::for_report(&NameMap::new()), blinded: 0 };
        let mut past = Vec::new();
        copier.packet(&deep, NEST_LIMIT + 1, &mut past);
        let mut blinded = deep.clone();
        copier.blinder.blind(&mut blinded);
        assert_eq!(past, blinded, "past the limit the packet is copied as one");
    }

    #[test]
    fn a_packet_split_over_two_lines_is_written_where_it_completes() {
        let record = self_record();
        let (a, b) = record.split_at(10);
        let prepared = prepare(&log(&[a.to_vec(), b.to_vec()])).unwrap();
        let copy = lines(&prepared.text);
        assert_eq!(copy.len(), 1);
        assert!(copy[0].ts.starts_with("2026-01-02T12:00:11"));
        assert_eq!(copy[0].bytes.len(), record.len());
    }

    #[test]
    fn damage_records_are_copied_as_they_are() {
        let damage = wrap_record(&[&[0x04, 0x38][..], b"Tester", &[0x00]].concat()).unwrap();
        let prepared = prepare(&log(&[self_record(), damage.clone()])).unwrap();
        assert!(prepared.text.contains(&hex(&damage)));
    }

    #[test]
    fn a_log_without_packets_is_refused() {
        assert_eq!(prepare("# Packet capture started\n").unwrap_err(), PrepareError::Empty);
    }

    #[test]
    fn refusals_read_as_the_player_sees_them() {
        assert_eq!(PrepareError::Empty.to_string(), "the packet log has no packets");
        assert_eq!(PrepareError::NameLeft.to_string(), "a character name was still in the copy, so no copy was written");
    }

    #[test]
    fn the_line_reader_keeps_good_lines_and_passes_over_bad_ones() {
        let at = "2026-01-02T12:00:10.000000000+00:00";
        let text = format!(
            "# a note\r\n\r\n{at}|Client:5000:13328|0A0b\r\n{at}|Client:5000:13328|0A0\n{at}|Client:5000:13328|0G\n\
             {at}|Client:5000:13328\nyesterday|Client:5000:13328|0A\n{at}|Client:1:2| \n{at}|K|00|11\n{at}|K|+A\n"
        );
        let got = lines(&text);
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].ts, got[0].key, got[0].at_ms), (at, "Client:5000:13328", 1_767_355_210_000));
        assert_eq!(got[0].bytes, [0x0A, 0x0B]);
        assert_eq!(got[1].key, "Client:1:2");
        assert!(got[1].bytes.is_empty());
        let items: Vec<LogItem> = read_log(&text).collect();
        assert!(matches!(items[0], LogItem::Note("# a note")));
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn one_plain_packet_reads_as_one_piece() {
        let record = self_record();
        assert_eq!(readable(&log(std::slice::from_ref(&record)), false), [record]);
    }

    #[test]
    fn the_names_come_from_parsing_the_log() {
        let text = log(&[self_record()]);
        let packets = lines(&text);
        let names = name_table(&packets.iter().collect::<Vec<_>>());
        assert!(names.contains_key("Tester"), "{names:?}");
        let prepared = prepare(&text).unwrap();
        assert_eq!(prepared.names_known, names.len());
        assert!(prepared.names_blinded >= 1);
    }

    #[test]
    fn the_copy_keeps_notes_in_place_and_marks_the_data_once() {
        let record = hex(&self_record());
        let text = format!(
            "# first\n\n2026-01-02T12:00:10.000000000+00:00|Client:5000:13328|{record}\n# middle\n\
             2026-01-02T12:00:11.000000000+00:00|Client:5000:13328|{record}\n"
        );
        let copy = prepare(&text).unwrap().text;
        let shape: Vec<&str> = copy.lines().map(|l| if l.starts_with("2026") { "data" } else { l }).collect();
        assert_eq!(shape, ["# first", MARK, "data", "# middle", "data"]);
        assert!(copy.ends_with('\n'));
    }

    /// Length-prefixed text in whole frames: a byte of 6 to 40, then that many
    /// bytes of UTF-8 without control characters, at least half letters and
    /// digits (all of them when 3 characters or fewer).
    fn spelled_out(pieces: &[Vec<u8>]) -> std::collections::HashSet<String> {
        fn wordlike(text: &str) -> bool {
            let (mut all, mut plain) = (0, 0);
            for c in text.chars() {
                if c.is_control() {
                    return false;
                }
                all += 1;
                plain += usize::from(c.is_alphanumeric());
            }
            plain * 2 >= all && (all > 3 || plain == all)
        }
        /// A whole frame's bytes past its length and opcode.
        fn whole(piece: &[u8]) -> Option<&[u8]> {
            let head = varint_at(piece, 0);
            (head.length > 0 && framing::frame_size(head.value, head.length) == Some(piece.len()))
                .then(|| piece.get(head.length as usize + 2..))
                .flatten()
        }
        let mut found = std::collections::HashSet::new();
        for mut rest in pieces.iter().filter_map(|p| whole(p)) {
            while let Some((&n, tail)) = rest.split_first() {
                rest = tail;
                let n = usize::from(n);
                let Some(body) = tail.get(..n).filter(|_| (6..=40).contains(&n)) else {
                    continue;
                };
                if let Some(text) = std::str::from_utf8(body).ok().filter(|t| wordlike(t)) {
                    found.insert(text.to_owned());
                }
            }
        }
        found
    }

    /// Run on a real capture: `A2_REPLAY_CAPTURE=<packets_*.txt> cargo test
    /// --lib report_log -- --ignored`.
    #[test]
    #[ignore]
    fn a_real_log_copies_without_names_and_replays_the_same() {
        let Ok(path) = std::env::var("A2_REPLAY_CAPTURE") else {
            return;
        };
        let original = std::fs::read_to_string(&path).unwrap();
        let copy = prepare(&original).unwrap().text;

        let report = |text: &str| {
            let mut out = Vec::new();
            crate::checks::replay_report::run(text, Default::default(), &mut |line| out.push(line));
            out.into_iter()
                .filter(|line| !line.starts_with("payloads ") && !line.starts_with("window "))
                .map(|line| match line.strip_prefix("state: ").and(line.rfind(", ")) {
                    // The spawn scan can find a mob or two more in blinded bytes.
                    Some(cut) => line[..cut].to_string(),
                    None => line,
                })
                .collect::<Vec<_>>()
        };
        let before = report(&original);
        assert!(before.iter().any(|line| line.starts_with("\n== target")), "the capture has a target");
        assert_eq!(before, report(&copy));

        let copy_bytes = readable(&copy, false);
        let packets = lines(&original);
        for name in name_table(&packets.iter().collect::<Vec<_>>()).keys().filter(|n| n.len() >= MIN_CHECKED_NAME) {
            let needle = name.as_bytes();
            assert!(!copy_bytes.iter().any(|b| b.windows(needle.len()).any(|w| w == needle)), "a resolved name is left");
        }

        let spelled = spelled_out(&readable(&original, true));
        assert!(spelled.len() > 20, "only {} strings found", spelled.len());
        let left = spelled_out(&copy_bytes);
        assert_eq!(spelled.intersection(&left).count(), 0, "a spelled-out string is left");
    }
}
