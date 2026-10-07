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
use std::fmt::Write as _;
use std::sync::Arc;

use crate::capture::evidence_slice::{frame_packet, Blinder, NameMap, EVENT_OPCODES};
use crate::capture::framing::{self, FrameKind, MAX_BUNDLE_DEPTH};
use crate::capture::packet_accumulator::PacketAccumulator;
use crate::capture::stream_assembler::StreamAssembler;
use crate::capture::stream_processor::StreamProcessor;
use crate::capture::varint::read_varint;
use crate::combat::data_storage::DataStorage;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// The comment line a prepared copy starts its data with.
pub const MARK: &str = "# Character names blinded for a bug report";

#[derive(Debug)]
pub struct Prepared {
    pub text: String,
    /// Names the capture resolved, all blinded.
    pub names_known: usize,
    /// Places a name was blinded.
    pub names_blinded: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PrepareError {
    /// No packet lines in the file.
    Empty,
    /// A resolved name was still in the copy. Nothing is written.
    NameLeft,
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrepareError::Empty => write!(f, "the packet log has no packets"),
            PrepareError::NameLeft => write!(f, "a character name was still in the copy, so no copy was written"),
        }
    }
}

/// One `TIMESTAMP|STREAMKEY|HEX` line.
pub struct Line<'a> {
    pub ts: &'a str,
    pub key: &'a str,
    pub at_ms: i64,
    pub bytes: Vec<u8>,
}

enum Entry<'a> {
    Comment(&'a str),
    Data(Line<'a>),
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

/// Lines as `replay_report` reads them: a line whose time stamp or hex does
/// not parse is left out.
fn entries(text: &str) -> Vec<Entry<'_>> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            out.push(Entry::Comment(line));
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(ts), Some(key), Some(hex)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Some(bytes) = decode_hex(hex) else { continue };
        let Ok(when) = chrono::DateTime::parse_from_rfc3339(ts) else { continue };
        out.push(Entry::Data(Line { ts, key, at_ms: when.timestamp_millis(), bytes }));
    }
    out
}

/// The packet lines of a packet log.
pub fn lines(text: &str) -> Vec<Line<'_>> {
    entries(text)
        .into_iter()
        .filter_map(|e| match e {
            Entry::Data(line) => Some(line),
            Entry::Comment(_) => None,
        })
        .collect()
}

/// Every name the capture resolves: party members (with their roster ids),
/// every named entity, and you. Read along the way, since a zone change
/// clears the names the parser holds.
pub fn names_in(lines: &[&Line]) -> NameMap {
    let storage = Arc::new(DataStorage::new());
    let mut streams: HashMap<&str, (StreamAssembler, StreamProcessor)> = HashMap::new();
    let mut names = NameMap::new();
    let collect = |names: &mut NameMap| {
        for (name, member) in storage.get_party_members() {
            if member.dbid != 0 {
                names.insert(name, member.dbid);
            } else {
                names.entry(name).or_insert(0);
            }
        }
        for name in storage.get_nicknames().into_values() {
            names.entry(name).or_insert(0);
        }
        if let Some(name) = storage.local_character_name() {
            names.entry(name).or_insert(0);
        }
    };
    for (n, line) in lines.iter().enumerate() {
        let (assembler, processor) = streams.entry(line.key).or_insert_with(|| {
            let processor = StreamProcessor::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
            (StreamAssembler::new(), processor)
        });
        processor.set_override_timestamp(Some(line.at_ms));
        assembler.process_chunk(&line.bytes, processor);
        if n % 200 == 199 {
            collect(&mut names);
        }
    }
    collect(&mut names);
    names.retain(|name, _| !name.is_empty());
    names
}

fn is_event(packet: &[u8]) -> bool {
    let header = read_varint(packet, 0);
    let o = header.length.max(0) as usize;
    header.length > 0 && packet.len() >= o + 2 && EVENT_OPCODES.contains(&[packet[o], packet[o + 1]])
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
    /// written again when the bundle's changed.
    fn packet(&mut self, packet: &[u8], out: &mut Vec<u8>) {
        if is_event(packet) {
            out.extend_from_slice(packet);
            return;
        }
        let mut copy = packet.to_vec();
        self.blinded += self.blinder.blind(&mut copy);
        let embedded = framing::embedded_bundles(packet);
        if embedded.is_empty() {
            out.extend_from_slice(&copy);
            return;
        }
        let mut rebuilt = Vec::with_capacity(copy.len());
        let mut at = 0;
        for bundle in embedded {
            rebuilt.extend_from_slice(&copy[at..bundle.start]);
            let inner = self.inner(&bundle.data, 2);
            match (inner != bundle.data).then(|| bundle_frame(&inner)).flatten() {
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
        let header = read_varint(&rebuilt, 0);
        match frame_packet(&rebuilt[header.length.max(0) as usize..]) {
            Some(framed) => out.extend_from_slice(&framed),
            None => out.extend_from_slice(&rebuilt),
        }
    }

    /// A bundle's contents: packets, nested bundles, and whatever does not
    /// frame, all blinded.
    fn inner(&mut self, data: &[u8], depth: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let walk = framing::walk_inner(data);
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
            FrameKind::Packet => self.packet(bytes, out),
            FrameKind::Bundle => match framing::decompress_bundle(frame.payload(buffer)).filter(|_| depth <= MAX_BUNDLE_DEPTH) {
                Some(data) => {
                    let inner = self.inner(&data, depth + 1);
                    match (inner != data).then(|| bundle_frame(&inner)).flatten() {
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

/// `<varint len> FF FF <u32 size> <lz4>`.
fn bundle_frame(inner: &[u8]) -> Option<Vec<u8>> {
    let mut payload = vec![0xFF, 0xFF];
    payload.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    payload.extend_from_slice(&lz4_flex::compress(inner));
    frame_packet(&payload)
}

/// The bug-report copy of a packet log.
pub fn prepare(text: &str) -> Result<Prepared, PrepareError> {
    let entries = entries(text);
    let data: Vec<&Line> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Data(line) => Some(line),
            Entry::Comment(_) => None,
        })
        .collect();
    if data.is_empty() {
        return Err(PrepareError::Empty);
    }
    let names = names_in(&data);

    let mut copier = Copier { blinder: Blinder::for_report(&names), blinded: 0 };
    let mut out = String::with_capacity(text.len());
    let mut marked = false;
    let mut streams: HashMap<&str, PacketAccumulator> = HashMap::new();
    for entry in &entries {
        let line = match entry {
            Entry::Comment(comment) => {
                out.push_str(comment);
                out.push('\n');
                continue;
            }
            Entry::Data(line) => line,
        };
        if !marked {
            out.push_str(MARK);
            out.push('\n');
            marked = true;
        }
        // The way `StreamAssembler` reads a line: walk until nothing more frames.
        let acc = streams.entry(line.key).or_insert_with(PacketAccumulator::new);
        acc.append(&line.bytes);
        let mut bytes = Vec::new();
        while acc.size() > 0 {
            let buffer = acc.snapshot().to_vec();
            let walk = framing::walk(&buffer);
            if walk.consumed == 0 {
                break;
            }
            copier.region(&buffer[..walk.consumed], &walk.frames, &mut bytes);
            acc.discard_bytes(walk.consumed);
        }
        if bytes.is_empty() {
            continue;
        }
        let _ = write!(out, "{}|{}|", line.ts, line.key);
        for b in &bytes {
            let _ = write!(out, "{b:02X}");
        }
        out.push('\n');
    }

    if names_left(&out, &names) {
        return Err(PrepareError::NameLeft);
    }
    Ok(Prepared { text: out, names_known: names.len(), names_blinded: copier.blinded })
}

/// Shortest name the check looks for. A one- or two-byte name turns up in
/// any long run of binary by chance; the blinder still blinds those.
pub const MIN_CHECKED_NAME: usize = 3;

/// Every byte of a packet log as the parser can read it: packets, the bytes
/// between them, and every bundle opened, the ones inside packets too.
/// Damage, damage over time and HP records hold no names: `events` false
/// leaves them out.
pub fn readable(text: &str, events: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut walker = Readable { events, out: &mut out };
    let mut streams: HashMap<&str, PacketAccumulator> = HashMap::new();
    for line in lines(text) {
        let acc = streams.entry(line.key).or_insert_with(PacketAccumulator::new);
        acc.append(&line.bytes);
        while acc.size() > 0 {
            let buffer = acc.snapshot().to_vec();
            let walk = framing::walk(&buffer);
            if walk.consumed == 0 {
                break;
            }
            let mut at = 0;
            for frame in &walk.frames {
                walker.out.push(buffer[at..frame.start].to_vec());
                walker.frame(&buffer, frame, 1);
                at = frame.end;
            }
            walker.out.push(buffer[at..walk.consumed].to_vec());
            acc.discard_bytes(walk.consumed);
        }
    }
    out.retain(|b| !b.is_empty());
    out
}

struct Readable<'a> {
    events: bool,
    out: &'a mut Vec<Vec<u8>>,
}

impl Readable<'_> {
    fn packet(&mut self, bytes: &[u8], depth: usize) {
        if !self.events && is_event(bytes) {
            return;
        }
        self.out.push(bytes.to_vec());
        if depth <= MAX_BUNDLE_DEPTH {
            for bundle in framing::embedded_bundles(bytes) {
                self.inner(&bundle.data, depth + 1);
            }
        }
    }

    fn inner(&mut self, data: &[u8], depth: usize) {
        let walk = framing::walk_inner(data);
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
            FrameKind::Packet => self.packet(frame.bytes(buffer), depth),
            FrameKind::Bundle => match framing::decompress_bundle(frame.payload(buffer)).filter(|_| depth <= MAX_BUNDLE_DEPTH) {
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

    /// A self record (`33 36`) naming "Naicha", from a live capture.
    fn self_record() -> Vec<u8> {
        let hex = "3336ed745e91c12837064e616963686118051e000000011c0000007f0100007f0100001c000000d002040000000000";
        let body: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        frame_packet(&body).unwrap()
    }

    fn log(lines: &[Vec<u8>]) -> String {
        let mut text = String::from("# Packet capture started at 2026-10-04T00:42:03-07:00\n# Format: TIMESTAMP|STREAMKEY|HEX_DATA\n\n");
        for (i, bytes) in lines.iter().enumerate() {
            text.push_str(&format!("2026-10-04T00:42:{:02}.000000000-07:00|Client:5000:13328|{}\n", 10 + i, hex(bytes)));
        }
        text
    }

    #[test]
    fn a_name_in_a_packet_is_blinded_and_the_rest_kept() {
        let record = self_record();
        let prepared = prepare(&log(&[record.clone()])).unwrap();
        assert!(prepared.text.contains(MARK));
        assert!(!prepared.text.contains(&hex(b"Naicha")));
        let copy = lines(&prepared.text);
        assert_eq!(copy.len(), 1);
        assert_eq!(copy[0].bytes.len(), record.len());
        let at = record.windows(6).position(|w| w == b"Naicha").unwrap();
        for (i, (a, b)) in record.iter().zip(&copy[0].bytes).enumerate() {
            if !(at..at + 6).contains(&i) {
                assert_eq!(a, b, "byte {i} outside the name changed");
            }
        }
    }

    #[test]
    fn a_name_inside_a_bundle_is_blinded() {
        let bundle = bundle_frame(&self_record()).unwrap();
        let prepared = prepare(&log(&[bundle])).unwrap();
        let readable = readable(&prepared.text, false);
        assert!(!readable.iter().any(|b| b.windows(6).any(|w| w == b"Naicha")));
        assert!(readable.iter().any(|b| b.len() == self_record().len()), "the record is still there, blinded");
    }

    #[test]
    fn a_name_inside_a_bundle_inside_a_packet_is_blinded() {
        let mut host = vec![0x00, 0x36, 0x01, 0x02, 0x03];
        host.extend(bundle_frame(&self_record()).unwrap());
        host.extend([0x04, 0x05]);
        let packet = frame_packet(&host).unwrap();
        let original = readable(&log(&[packet.clone()]), false);
        assert!(original.iter().any(|b| b.windows(6).any(|w| w == b"Naicha")), "the test finds the name in the original");
        let prepared = prepare(&log(&[packet])).unwrap();
        assert!(!readable(&prepared.text, false).iter().any(|b| b.windows(6).any(|w| w == b"Naicha")));
    }

    #[test]
    fn a_packet_split_over_two_lines_is_written_where_it_completes() {
        let record = self_record();
        let (a, b) = record.split_at(10);
        let prepared = prepare(&log(&[a.to_vec(), b.to_vec()])).unwrap();
        let copy = lines(&prepared.text);
        assert_eq!(copy.len(), 1);
        assert!(copy[0].ts.starts_with("2026-10-04T00:42:11"));
        assert_eq!(copy[0].bytes.len(), record.len());
    }

    #[test]
    fn damage_records_are_copied_as_they_are() {
        let damage = frame_packet(&[0x04, 0x38, 0x4e, 0x61, 0x69, 0x63, 0x68, 0x61, 0x00]).unwrap();
        let prepared = prepare(&log(&[self_record(), damage.clone()])).unwrap();
        assert!(prepared.text.contains(&hex(&damage)));
    }

    /// Strings as the protocol writes them, `<u8 len><len bytes of text>`, six
    /// bytes and up: the slice acceptance test's rule
    /// (`tests/evidence_slice_replay.rs`), with the blinder's rule for three
    /// characters or fewer: letters and digits only (a name is nothing else).
    /// "ن=侂" sat in a player record of one capture of 2026-10-04.
    fn length_prefixed_strings(buf: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < buf.len() {
            let len = buf[i] as usize;
            if !(6..=40).contains(&len) || i + 1 + len > buf.len() {
                i += 1;
                continue;
            }
            match std::str::from_utf8(&buf[i + 1..i + 1 + len]) {
                Ok(s) if !s.chars().any(|c| c.is_control()) => {
                    let alnum = s.chars().filter(|c| c.is_alphanumeric()).count();
                    let chars = s.chars().count();
                    if alnum * 2 >= chars.max(1) && (chars > 3 || alnum == chars) {
                        out.push(s.to_string());
                    }
                    i += 1 + len;
                }
                _ => i += 1,
            }
        }
        out
    }

    /// A prepared copy of a real packet log replays to the same report as the
    /// original, and no name the capture resolves, nor any string from it,
    /// is left in the copy.
    ///   A2_REPLAY_CAPTURE=.../packets_x.txt cargo test --lib report_log -- --ignored --nocapture
    #[test]
    #[ignore = "needs a packet log in A2_REPLAY_CAPTURE"]
    fn a_prepared_copy_of_a_real_log_replays_the_same_with_no_name_left() {
        let Ok(path) = std::env::var("A2_REPLAY_CAPTURE") else {
            eprintln!("A2_REPLAY_CAPTURE unset: skipping");
            return;
        };
        let text = std::fs::read_to_string(&path).expect("capture readable");
        let prepared = prepare(&text).expect("the copy is written");
        println!("{} names known, {} places blinded", prepared.names_known, prepared.names_blinded);

        // The same report, but for the counts of lines read and of mobs. The
        // spawn scan reads past some records into the next one; where that is
        // a blinded name, it can find a mob with a made-up code and no damage
        // (two captures of 2026-10-04: one and two more mobs).
        let report = |text: &str| {
            let mut lines = Vec::new();
            let options = crate::checks::replay_report::Options::default();
            crate::checks::replay_report::run(text, options, &mut |line| lines.push(line));
            lines.retain(|l| !l.starts_with("payloads ") && !l.starts_with("window "));
            for line in lines.iter_mut().filter(|l| l.starts_with("state: ")) {
                if let Some(at) = line.rfind(", ") {
                    println!("state ends: {}", &line[at + 2..]);
                    line.truncate(at);
                }
            }
            lines
        };
        let want = report(&text);
        let got = report(&prepared.text);
        assert!(want.iter().any(|l| l.starts_with("\n== target")), "the capture has no damage: this would check nothing");
        let differ: Vec<(&String, &String)> = want.iter().zip(&got).filter(|(a, b)| a != b).collect();
        for (a, b) in differ.iter().take(10) {
            println!("  original: {a}\n  copy:     {b}");
        }
        assert!(differ.is_empty() && want.len() == got.len(), "the copy replays differently");
        println!("report: {} lines, identical", want.len());

        // No resolved name of three bytes or more. Damage, DoT and HP records
        // are ids and numbers only and are copied as they are: a four-byte
        // name turned up in them by chance on one capture of 2026-10-04.
        let all: Vec<Line> = lines(&text);
        let names = names_in(&all.iter().collect::<Vec<_>>());
        let copy = readable(&prepared.text, true);
        let contains = |needle: &[u8]| copy.iter().any(|b| b.len() >= needle.len() && b.windows(needle.len()).any(|w| w == needle));
        let copy_without_events = readable(&prepared.text, false);
        let checked: Vec<&String> = names.keys().filter(|n| n.len() >= MIN_CHECKED_NAME).collect();
        assert!(!checked.is_empty(), "no names resolved: this would check nothing");
        let in_events = checked.iter().filter(|n| contains(n.as_bytes())).count();
        let named = checked
            .iter()
            .filter(|n| copy_without_events.iter().any(|b| b.windows(n.len()).any(|w| w == n.as_bytes())))
            .count();
        println!("{} resolved names checked, {} only in damage, DoT or HP records", checked.len(), in_events - named);
        assert_eq!(named, 0, "a resolved character name is in the copy");

        // No length-prefixed string from the original, damage records included.
        // A packet's own length and opcode are not a string: skip them, as the
        // blinder does.
        let body = |b: &Vec<u8>| -> Vec<u8> {
            let header = read_varint(b, 0);
            let whole = header.length > 0
                && framing::frame_size(header.value, header.length) == Some(b.len());
            if whole { b.get(header.length as usize + 2..).unwrap_or_default().to_vec() } else { b.clone() }
        };
        let mut strings: Vec<String> = readable(&text, true).iter().flat_map(|b| length_prefixed_strings(&body(b))).collect();
        strings.sort();
        strings.dedup();
        assert!(strings.len() > 20, "only {} strings in the capture", strings.len());
        // Where the copy still has one as a string of its own. The same bytes
        // inside binary data elsewhere are not a string: the player records'
        // appearance data repeats from player to player.
        let strings_in = |pieces: &[Vec<u8>]| {
            let mut left: Vec<String> = pieces.iter().flat_map(|b| length_prefixed_strings(&body(b))).collect();
            left.sort();
            left.dedup();
            left
        };
        let left = strings_in(&copy_without_events);
        let left_in_events = strings_in(&copy);
        let survivors: Vec<&String> = strings.iter().filter(|s| left.binary_search(s).is_ok()).collect();
        let in_events = strings.iter().filter(|s| left_in_events.binary_search(s).is_ok()).count() - survivors.len();
        println!(
            "{} strings in the capture, {} in the copy, {} only in damage, DoT or HP records",
            strings.len(),
            survivors.len(),
            in_events
        );
        for s in survivors.iter().take(20) {
            println!("  in the copy: {s:?}");
        }
        // Text seen in an early slice: players never in the party, and public
        // chat (the slice acceptance test's list). Checked where the capture has it.
        for s in ["BaroqueWorks", "SoloLeveling", "TruemansD", "Megalobox", "1-man army", "850K 连刷"] {
            if strings.iter().any(|o| o.contains(s)) {
                assert!(!contains(s.as_bytes()), "third-party text is in the copy");
            }
        }
        assert!(survivors.is_empty(), "{} strings from the capture are in the copy", survivors.len());
    }

    #[test]
    fn a_log_without_packets_is_refused() {
        assert_eq!(prepare("# Packet capture started\n").unwrap_err(), PrepareError::Empty);
    }
}
