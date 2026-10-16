//! `a2t-inspect`: what an Evidence Slice or a packet capture holds, read on
//! your own machine (see docs/PRIVACY.md). It also cuts the slice of every
//! fight in a capture, rebuilds the fight from that slice alone and compares
//! it with the fight the whole capture gives. Nothing here opens a socket.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use a2tools_dps_meter_lib::capture::fight_tape::{self, SliceRecord, KEPT_OPCODES, LIFT_OPCODE};
use a2tools_dps_meter_lib::capture::framing::{self, Frame, ChunkForm, NEST_LIMIT};
use a2tools_dps_meter_lib::capture::{byte_queue::ByteQueue, ping_tracker::PingTracker};
use a2tools_dps_meter_lib::capture::decoder::PacketDecoder;
use a2tools_dps_meter_lib::combat::{data_storage::DataStorage, dps_calculator::DpsCalculator};
use a2tools_dps_meter_lib::{entity::fight_record::FightRecord, i18n::lookup::{NpcLookup, SkillLookup}};
use a2tools_dps_meter_lib::slice_rebuild::{self, RebuiltFight};
use a2tools_dps_meter_lib::share;

const USAGE: &str = "\
a2t-inspect: see what a slice or a packet capture holds

  a2t-inspect <slice>                    records, opcodes and blinded names in a slice
  a2t-inspect <slice> --strings          the same, then every readable run of text
  a2t-inspect <slice> --find NAME        the same, searched for NAME; exits 1 when found
  a2t-inspect --capture <packets_*.txt>  opcodes and readable text in a packet capture
  a2t-inspect --derive <app-data-dir> [fight-id ...]   saved fights rebuilt from slices
  a2t-inspect --derive-capture <packets_*.txt>         every fight of one capture, the same

A slice (.a2es or .a2es.gz) is what an upload sends; a packet capture is the
whole game connection.
";

/// Mode 4 reads the parser's tables from the repo's `src/data`, wherever the
/// tool was built.
const TABLE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/data");

/// The live meter saves its boss fights this often.
const SAVE_EVERY_MS: i64 = 30_000;

/// Differing skill rows printed per fight unless `A2_ALL_ROWS` is set.
const ROWS_SHOWN: usize = 12;

/// The message a run stops with; it exits 1.
#[derive(Debug)]
struct Failed(String);

impl From<io::Error> for Failed {
    fn from(why: io::Error) -> Self {
        Failed(format!("cannot write the report: {why}"))
    }
}

/// The exit status, or why the run stopped.
type Run = Result<u8, Failed>;

enum Mode<'a> {
    Slice { path: &'a str, strings: bool, find: Option<&'a str> },
    CaptureStrings(&'a str),
    History { dir: &'a str, ids: &'a [String] },
    CaptureFights(&'a str),
}

/// What the arguments ask for; `None` means print the usage.
fn mode(args: &[String]) -> Option<Mode<'_>> {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        return None;
    }
    let (first, rest) = args.split_first()?;
    Some(match first.as_str() {
        "--capture" => Mode::CaptureStrings(rest.first()?),
        "--derive" => Mode::History { dir: rest.first()?, ids: &rest[1..] },
        "--derive-capture" => Mode::CaptureFights(rest.first()?),
        path => {
            let after_find = rest.iter().position(|a| a == "--find").and_then(|at| rest.get(at + 1));
            Mode::Slice { path, strings: rest.iter().any(|a| a == "--strings"), find: after_find.map(String::as_str) }
        }
    })
}

/// Settings mode 4 takes from the environment.
#[derive(Default)]
struct Knobs {
    /// `A2_WRITE_SLICE`: where each fight's slice and rebuilt fight go.
    write_to: Option<PathBuf>,
    /// `A2_ALL_ROWS`: print every differing skill row.
    all_rows: bool,
}

fn main() -> ExitCode {
    let given: Vec<String> = std::env::args().skip(1).collect();
    let Some(asked) = mode(&given) else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let out = &mut io::stdout().lock();
    let knobs = Knobs { write_to: std::env::var_os("A2_WRITE_SLICE").map(PathBuf::from), all_rows: std::env::var_os("A2_ALL_ROWS").is_some() };
    let ran = match asked {
        Mode::Slice { path, strings, find } => look_into_slice(path, strings, find, out),
        Mode::CaptureStrings(path) => look_into_capture(path, out),
        Mode::History { dir, ids } => Tables::load().and_then(|t| recheck_history(Path::new(dir), ids, &t, &knobs, out)),
        Mode::CaptureFights(path) => Tables::load().and_then(|t| recheck_capture(path, &t, &knobs, out)),
    };
    match ran {
        Ok(status) => ExitCode::from(status),
        Err(Failed(why)) => {
            eprintln!("{why}");
            ExitCode::from(1)
        }
    }
}

// ── shared byte views ──────────────────────────────────────────────────────

/// The two bytes after a frame's length varint, when it has them.
fn opcode_of(frame: &Frame, buf: &[u8]) -> Option<[u8; 2]> {
    frame.payload(buf).get(..2)?.try_into().ok()
}

fn hex_pair(op: [u8; 2]) -> String {
    format!("{:02X} {:02X}", op[0], op[1])
}

/// Most frequent first; equal counts in opcode order.
fn by_count(counts: &HashMap<[u8; 2], usize>) -> Vec<([u8; 2], usize)> {
    let mut rows: Vec<([u8; 2], usize)> = counts.iter().map(|(&op, &n)| (op, n)).collect();
    rows.sort_by_key(|&(op, n)| (Reverse(n), op));
    rows
}

/// Adds every stretch of `bytes` that reads as text: printable ASCII or any
/// byte from 0x80 up, `shortest` bytes or more, valid UTF-8 as a whole. A
/// stretch that is not valid UTF-8 is dropped, not cut down.
fn text_runs_into(bytes: &[u8], shortest: usize, found: &mut BTreeSet<String>) {
    let readable = |b: &u8| (0x20..=0x7E).contains(b) || *b >= 0x80;
    for stretch in bytes.split(|b| !readable(b)).filter(|s| s.len() >= shortest) {
        if let Ok(text) = std::str::from_utf8(stretch) {
            found.insert(text.to_owned());
        }
    }
}

/// Most characters first; the set already holds equal lengths in byte order.
fn longest_first(found: BTreeSet<String>) -> Vec<String> {
    let mut runs: Vec<String> = found.into_iter().collect();
    runs.sort_by_key(|run| Reverse(run.chars().count()));
    runs
}

/// A run's report line: its whole length in characters, then at most
/// `at_most` of them.
fn bracketed(run: &str, at_most: usize) -> String {
    format!("  [{:>3}] {}", run.chars().count(), run.chars().take(at_most).collect::<String>())
}

// ── modes 1 and 2: a slice ─────────────────────────────────────────────────

fn not_a_slice(path: &str) -> Failed {
    Failed(format!(
        "{path} is not an Evidence Slice.\n\n\
         A packet capture (packets_*.txt) is another thing and far more sensitive: it\n\
         holds the whole game connection, chat included. Read docs/PRIVACY.md before\n\
         sharing one; `a2t-inspect --capture <file>` shows what one holds."
    ))
}

/// The slice bytes in a file, gunzipped when the file opens like gzip, and
/// whether it did.
fn slice_in_file<'a>(path: &str, file: &'a [u8]) -> Result<(Cow<'a, [u8]>, bool), Failed> {
    if !file.starts_with(&[0x1F, 0x8B]) {
        return Ok((Cow::Borrowed(file), false));
    }
    let plain = gunzip(file).map_err(|why| Failed(format!("{path} is gzipped but will not decompress: {why}")))?;
    Ok((Cow::Owned(plain), true))
}

/// The first gzip member's contents.
fn gunzip(packed: &[u8]) -> io::Result<Vec<u8>> {
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(packed).read_to_end(&mut plain)?;
    Ok(plain)
}

fn size_line(file_len: usize, slice_len: usize, gzipped: bool) -> String {
    if gzipped {
        let ratio = slice_len as f64 / file_len.max(1) as f64;
        format!("  {:<17}{file_len} bytes gzipped ({slice_len} uncompressed, {ratio:.1}x)", "file size")
    } else {
        format!("  {:<17}{slice_len} bytes", "file size")
    }
}

/// From the first record's offset to the last one's, in file order.
fn span_line(first: i32, last: i32) -> String {
    let span = i64::from(last) - i64::from(first);
    format!("  {:<17}{}.{:03}s  (offsets {first} to {last} ms from fight start)", "time span", span / 1000, (span % 1000).abs())
}

fn look_into_slice(path: &str, strings: bool, find: Option<&str>, out: &mut dyn Write) -> Run {
    let file = fs::read(path).map_err(|why| Failed(format!("cannot read {path}: {why}")))?;
    let (bytes, gzipped) = slice_in_file(path, &file)?;
    let (records, blind_map) = fight_tape::decode(&bytes).ok_or_else(|| not_a_slice(path))?;
    // What a replay reads, which is also what a reader of the file can get at.
    let readable: Vec<Vec<u8>> = records.iter().map(|(_, r)| fight_tape::expand(r)).collect();

    writeln!(out, "Evidence Slice: {path}")?;
    writeln!(out, "{}", size_line(file.len(), bytes.len(), gzipped))?;
    writeln!(out, "  {:<17}{}", "records", records.len())?;
    writeln!(out, "  {:<17}{} bytes (decompressed)", "packet content", readable.iter().map(Vec::len).sum::<usize>())?;
    if let (Some((first, _)), Some((last, _))) = (records.first(), records.last()) {
        writeln!(out, "{}", span_line(*first, *last))?;
    }
    writeln!(out)?;

    writeln!(out, "This build's allowlist: {} opcodes", KEPT_OPCODES.len())?;
    for (op, what) in KEPT_OPCODES {
        writeln!(out, "  {}  {what}", hex_pair(**op))?;
    }
    writeln!(out)?;

    opcode_table(&readable, out)?;
    blinded_names(&blind_map, out)?;

    if let Some(name) = find {
        let hits: usize = readable.iter().map(|r| hits_of(name.as_bytes(), r)).sum();
        writeln!(out, "Search for {name:?}: {hits} occurrence(s) in the decompressed content")?;
        writeln!(out)?;
        if hits > 0 {
            return Ok(1);
        }
    }
    if strings {
        let mut found = BTreeSet::new();
        readable.iter().for_each(|r| text_runs_into(r, 4, &mut found));
        writeln!(out, "Printable runs of 4+ bytes: {}", found.len())?;
        for run in longest_first(found) {
            writeln!(out, "{}", bracketed(&run, usize::MAX))?;
        }
    } else {
        writeln!(out, "Add --strings to list every readable run of text in the file, or")?;
        writeln!(out, "--find <name> to search it for one character name.")?;
    }
    Ok(0)
}

fn opcode_table(readable: &[Vec<u8>], out: &mut dyn Write) -> io::Result<()> {
    let mut counts: HashMap<[u8; 2], usize> = HashMap::new();
    let mut packets = 0;
    for bytes in readable {
        for frame in framing::walk(bytes).frames.iter().filter(|f| f.kind == ChunkForm::Packet) {
            packets += 1;
            if let Some(op) = opcode_of(frame, bytes) {
                *counts.entry(op).or_default() += 1;
            }
        }
    }
    writeln!(out, "Opcodes present: {packets} packets")?;
    let (mut lifted, mut foreign) = (0, 0);
    for (op, n) in by_count(&counts) {
        let what = match KEPT_OPCODES.iter().find(|(listed, _)| **listed == op) {
            Some(&(_, what)) => what,
            None if op == LIFT_OPCODE => {
                lifted += n;
                "lifted piece"
            }
            None => {
                foreign += n;
                "*** NOT ON THE ALLOWLIST ***"
            }
        };
        writeln!(out, "  {}  {n:>7}  {what}", hex_pair(op))?;
    }
    if lifted > 0 {
        writeln!(out)?;
        writeln!(out, "  The {lifted} lifted pieces are this build's own frames: a hit, a spawn or a")?;
        writeln!(out, "  skill's context, copied out of a packet the slice does not keep.")?;
    }
    if foreign > 0 {
        writeln!(out)?;
        writeln!(out, "  {foreign} packets are not on this build's allowlist. This build did not make")?;
        writeln!(out, "  the file as it is now: do not trust it.")?;
    }
    writeln!(out)
}

fn blinded_names(blind_map: &HashMap<String, u64>, out: &mut dyn Write) -> io::Result<()> {
    let mut entries: Vec<(&String, &u64)> = blind_map.iter().collect();
    entries.sort();
    writeln!(out, "Blinded names: {}", entries.len())?;
    writeln!(out, "  Every name in the capture was swapped for a token of the same byte length.")?;
    writeln!(out, "  Readers see the tokens; the names themselves are not in the file.")?;
    for (token, id) in entries {
        writeln!(out, "  {token}  <- roster id 0x{id:016x}")?;
    }
    writeln!(out)
}

/// Places `needle` starts in `hay`, overlapping ones too. An empty needle
/// is found nowhere.
fn hits_of(needle: &[u8], hay: &[u8]) -> usize {
    match needle.len() {
        0 => 0,
        width => hay.windows(width).filter(|w| w.starts_with(needle)).count(),
    }
}

// ── mode 3: a packet capture ───────────────────────────────────────────────

/// The stream and bytes of a capture line; `None` for blank lines, comments
/// and lines that are not `stamp|stream|hex`.
fn capture_line(line: &str) -> Option<(&str, Vec<u8>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields = line.splitn(3, '|');
    let (_stamp, stream, digits) = (fields.next()?, fields.next()?, fields.next()?);
    // Each pair is checked digit by digit, so `+1` is not read as hex.
    let digit = |b: u8| char::from(b).to_digit(16);
    let pairs = digits.as_bytes().chunks(2).map(|two| match *two {
        [high, low] => Some(((digit(high)? << 4) | digit(low)?) as u8),
        _ => None,
    });
    Some((stream, pairs.collect::<Option<Vec<u8>>>()?))
}

#[derive(Default)]
struct CaptureTally {
    packets: usize,
    opcodes: HashMap<[u8; 2], usize>,
    text: BTreeSet<String>,
}

impl CaptureTally {
    /// Counts the packets among `frames`, walked in `buf`, and opens the
    /// bundles. `depth` is how many bundles `buf` came out of.
    fn take(&mut self, buf: &[u8], frames: &[Frame], depth: usize) {
        for frame in frames {
            match frame.kind {
                ChunkForm::Packet => {
                    self.packets += 1;
                    if let Some(op) = opcode_of(frame, buf) {
                        *self.opcodes.entry(op).or_default() += 1;
                    }
                    text_runs_into(frame.bytes(buf), 6, &mut self.text);
                }
                ChunkForm::Bundle if depth < NEST_LIMIT => {
                    if let Some(inner) = framing::inflate_block(frame.payload(buf)) {
                        // Not `split_frames`, which stops at the first bytes that
                        // do not frame: this walk steps past them and reads on.
                        self.take(&inner, &framing::walk(&inner).frames, depth + 1);
                    }
                }
                ChunkForm::Bundle => {}
            }
        }
    }
}

/// A line some of whose bytes still wait in its stream's joined bytes.
struct Waiting {
    /// Where the line starts in its stream.
    at: usize,
    /// Starts of the frames the line's own walk found, in order.
    own: Vec<usize>,
}

/// One stream's lines joined, the bytes not framed yet.
#[derive(Default)]
struct Joined {
    buf: Vec<u8>,
    /// Where `buf` starts in the stream.
    base: usize,
    lines: VecDeque<Waiting>,
}

impl Joined {
    /// Adds a line, then counts the frames now whole that no line's own
    /// walk found already.
    fn take(&mut self, bytes: &[u8], own: Vec<usize>, tally: &mut CaptureTally) {
        self.lines.push_back(Waiting { at: self.base + self.buf.len(), own });
        self.buf.extend_from_slice(bytes);
        let walked = framing::walk(&self.buf);
        let new: Vec<Frame> = walked.frames.into_iter().filter(|f| !self.found_alone(self.base + f.start)).collect();
        tally.take(&self.buf, &new, 0);
        self.buf.drain(..walked.consumed);
        self.base += walked.consumed;
        while self.lines.get(1).is_some_and(|next| next.at <= self.base) {
            self.lines.pop_front();
        }
    }

    /// Whether the frame starting at `at` in the stream is one its line's
    /// own walk found.
    fn found_alone(&self, at: usize) -> bool {
        let after = self.lines.partition_point(|line| line.at <= at);
        let line = &self.lines[after - 1];
        line.own.binary_search(&(at - line.at)).is_ok()
    }
}

fn look_into_capture(path: &str, out: &mut dyn Write) -> Run {
    let text = fs::read_to_string(path).map_err(|why| Failed(format!("cannot read {path}: {why}")))?;
    capture_report(&text, out)?;
    Ok(0)
}

/// Each line is framed on its own, and each stream's lines are also joined
/// and framed as the meter does. A joined walk finds a packet cut over two
/// lines; a line's own walk finds what a joined walk that began out of step
/// swallows. The report holds what either finds; a frame both find (same
/// stream, line and offset) counts once.
fn capture_report(text: &str, out: &mut dyn Write) -> io::Result<()> {
    let mut joined: HashMap<&str, Joined> = HashMap::new();
    let mut tally = CaptureTally::default();
    let mut lines = 0;
    for (stream, bytes) in text.lines().filter_map(capture_line) {
        lines += 1;
        let own = framing::walk(&bytes).frames;
        tally.take(&bytes, &own, 0);
        joined.entry(stream).or_default().take(&bytes, own.iter().map(|f| f.start).collect(), &mut tally);
    }

    writeln!(out, "capture lines: {lines}, flattened packets: {}, streams: {}", tally.packets, joined.len())?;
    writeln!(out)?;
    writeln!(out, "top 25 leading opcodes:")?;
    for (op, n) in by_count(&tally.opcodes).into_iter().take(25) {
        writeln!(out, "  {}  {n:>7}", hex_pair(op))?;
    }
    writeln!(out)?;
    writeln!(out, "distinct printable runs >= 6 bytes: {}", tally.text.len())?;
    writeln!(out, "longest 60:")?;
    for run in longest_first(tally.text).iter().take(60) {
        writeln!(out, "{}", bracketed(run, 120))?;
    }
    Ok(())
}

// ── mode 4: rebuild fights from their slices ───────────────────────────────

struct Tables {
    npcs: String,
    skills: String,
    dots: String,
}

impl Tables {
    fn load() -> Result<Self, Failed> {
        let read = |rel: &str| {
            let path = Path::new(TABLE_DIR).join(rel);
            fs::read_to_string(&path).map_err(|why| Failed(format!("cannot read the table {}: {why}", path.display())))
        };
        Ok(Tables { npcs: read("i18n/npcs/en.json")?, skills: read("i18n/skills/en.json")?, dots: read("dot_skill_ids.json")? })
    }
}

/// The fights a whole capture gives, and the storage they came from.
struct Whole {
    fights: Vec<FightRecord>,
    storage: Arc<DataStorage>,
}

/// Plays `packets` through one parser the way the live meter does, saving
/// boss fights every 30 s and once more at the end with all still open. A
/// later save of a fight replaces the earlier one.
fn whole_replay(packets: &[SliceRecord], tables: &Tables) -> Whole {
    let (npcs, skills) = (Arc::new(NpcLookup::new()), Arc::new(SkillLookup::new()));
    npcs.load_from_json(&tables.npcs);
    skills.load_from_json(&tables.skills);
    let storage = Arc::new(DataStorage::new());
    let mut meter = DpsCalculator::new(Arc::clone(&storage), Arc::clone(&skills), Arc::clone(&npcs), Arc::new(PingTracker::new()));
    let mut parser = PacketDecoder::new(Arc::clone(&storage), skills, npcs);
    // A table that is not a list of ids leaves the parser with none.
    parser.use_tick_skills(serde_json::from_str::<HashSet<i32>>(&tables.dots).unwrap_or_default());

    let mut saves: Vec<FightRecord> = Vec::new();
    let mut streams: HashMap<&str, ByteQueue> = HashMap::new();
    let mut save_at = packets.first().map_or(0, |p| p.captured_at_ms + SAVE_EVERY_MS);
    for p in packets {
        parser.pin_clock(Some(p.captured_at_ms));
        let held = streams.entry(p.stream.as_str()).or_default();
        held.append(&p.bytes);
        let used = parser.decode_buffer(held.snapshot());
        held.drop_front(used);
        if p.captured_at_ms >= save_at {
            saves.extend(meter.close_boss_fights());
            save_at = p.captured_at_ms + SAVE_EVERY_MS;
        }
    }
    if !packets.is_empty() {
        saves.extend(meter.close_boss_fights_now());
    }
    parser.pin_clock(None);
    // In save order, so the last save of a fight is the one left.
    let by_id: HashMap<String, FightRecord> = saves.into_iter().map(|r| (r.id.clone(), r)).collect();
    Whole { fights: by_id.into_values().collect(), storage }
}

type RowKey = (i32, i32, bool);
type RowValue = (i64, i32);

fn skill_rows(record: &FightRecord) -> BTreeMap<RowKey, RowValue> {
    record.details.skills.iter().map(|s| ((s.actor_id, s.code, s.is_dot), (s.dmg, s.time))).collect()
}

/// The result line and the differing rows for the whole capture's fight `w`
/// against `d`, rebuilt from the slice, and whether they count as the same:
/// boss, skill rows and total. Target, dungeon and server are shown only.
fn compare(w: &FightRecord, d: &FightRecord, slice_len: usize, packed_len: usize, all_rows: bool) -> (Vec<String>, bool) {
    let (ours, theirs) = (skill_rows(w), skill_rows(d));
    let keys: BTreeSet<&RowKey> = ours.keys().chain(theirs.keys()).collect();
    let differing: Vec<&RowKey> = keys.into_iter().filter(|k| ours.get(k) != theirs.get(k)).collect();
    let same = w.mob_code == d.mob_code && differing.is_empty() && w.details.total_target_damage == d.details.total_target_damage;
    let both = |a: i64, b: i64| format!("{a}/{b} {}", if a == b { "ok" } else { "MISMATCH" });
    let mut lines = vec![format!(
        "   slice {slice_len} bytes ({packed_len} gzipped): boss {} target {}  dungeon {}  server {}  total {} / {} over {} / {} ms  skill rows {} / {}  -> {}",
        d.mob_code,
        both(w.target_id.into(), d.target_id.into()),
        both(w.dungeon_id.into(), d.dungeon_id.into()),
        both(w.server_id.into(), d.server_id.into()),
        w.details.total_target_damage,
        d.details.total_target_damage,
        w.duration_ms,
        d.duration_ms,
        ours.len(),
        theirs.len(),
        if same { "identical" } else { "DIFFERENT" },
    )];
    let shown = if all_rows { differing.len() } else { ROWS_SHOWN };
    for key in differing.into_iter().take(shown) {
        lines.push(format!("     row {key:?}: whole {:?} slice {:?}", ours.get(key), theirs.get(key)));
    }
    (lines, same)
}

/// Cuts `w`'s slice out of `packets`, rebuilds the fight from the slice
/// alone and prints how it compares. True when it is the same fight.
fn cut_and_compare(w: &FightRecord, packets: &[SliceRecord], storage: &DataStorage, tables: &Tables, knobs: &Knobs, out: &mut dyn Write) -> io::Result<bool> {
    let end = w.start_time_ms + w.duration_ms;
    let mut cut = match fight_tape::build(packets, w.start_time_ms, end, &share::names_from(storage)) {
        Ok(cut) => cut,
        Err(why) => {
            writeln!(out, "   slice failed: {why:?}")?;
            return Ok(false);
        }
    };
    // As in kept and uploaded slices: the tokens stay, the accounts go.
    cut.blind_map.values_mut().for_each(|id| *id = 0);
    let slice = fight_tape::encode(&cut);
    let rebuilt = match slice_rebuild::fight_from_slice(&slice, &tables.npcs, &tables.skills, &tables.dots) {
        Ok(rebuilt) => rebuilt,
        Err(why) => {
            writeln!(out, "   derive failed: {why:?} ({} bytes of slice)", slice.len())?;
            where_the_target_went(w, &slice, out)?;
            return Ok(false);
        }
    };
    let packed = share::gzip(&slice).ok();
    if let Some(dir) = &knobs.write_to {
        write_copies(dir, &w.id, packed.as_deref(), &rebuilt);
    }
    let (lines, same) = compare(w, &rebuilt.record, slice.len(), packed.map_or(0, |p| p.len()), knobs.all_rows);
    for line in lines {
        writeln!(out, "{line}")?;
    }
    Ok(same)
}

/// After a failed rebuild: what the slice holds of `w`'s target, and its
/// four most damaged targets.
fn where_the_target_went(w: &FightRecord, slice: &[u8], out: &mut dyn Write) -> io::Result<()> {
    let Ok(summary) = slice_rebuild::derive(slice) else {
        return Ok(());
    };
    write!(out, "     the fight's target {} (mob {}): ", w.target_id, w.mob_code)?;
    match summary.targets.iter().find(|t| t.target_id == w.target_id) {
        Some(t) => writeln!(out, "in the slice as mob {}, {} damage over {} ms", t.mob_code, t.total_damage, t.duration_ms)?,
        None => writeln!(out, "not in the slice")?,
    }
    for t in summary.targets.iter().take(4) {
        writeln!(out, "     target {} mob {} damage {} over {} ms", t.target_id, t.mob_code, t.total_damage, t.duration_ms)?;
    }
    Ok(())
}

/// `A2_WRITE_SLICE`: the gzipped slice and the rebuilt fight, for testing a
/// log service against this build. Failures are ignored.
fn write_copies(dir: &Path, id: &str, packed: Option<&[u8]>, rebuilt: &RebuiltFight) {
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(dir.join(format!("{id}.a2es.gz")), packed.unwrap_or_default());
    let _ = fs::write(dir.join(format!("{id}.derived.json")), serde_json::to_vec(rebuilt).unwrap_or_default());
}

#[derive(Default)]
struct FightCount {
    tried: usize,
    same: usize,
}

impl FightCount {
    fn add(&mut self, same: bool) {
        self.tried += 1;
        self.same += usize::from(same);
    }

    /// The closing line; exits 0 only when at least one fight was tried and
    /// every one came out the same.
    fn finish(&self, out: &mut dyn Write) -> Run {
        writeln!(out)?;
        writeln!(out, "{} of {} fights: slice re-derived identically to the whole capture", self.same, self.tried)?;
        Ok(u8::from(self.tried == 0 || self.same < self.tried))
    }
}

fn by_start(a: &FightRecord, b: &FightRecord) -> std::cmp::Ordering {
    (a.start_time_ms, &a.id).cmp(&(b.start_time_ms, &b.id))
}

fn recheck_capture(path: &str, tables: &Tables, knobs: &Knobs, out: &mut dyn Write) -> Run {
    // The reader's error starts with the path.
    let packets = share::load_capture_file(Path::new(path)).map_err(|why| Failed(format!("cannot read {why}")))?;
    let Whole { mut fights, storage } = whole_replay(&packets, tables);
    fights.sort_by(by_start);
    let mut count = FightCount::default();
    for w in &fights {
        writeln!(out)?;
        writeln!(out, "== {} {} ({} ms, {} actors, total {})", w.id, w.boss_name, w.duration_ms, w.actors.len(), w.details.total_target_damage)?;
        count.add(cut_and_compare(w, &packets, &storage, tables, knobs, out)?);
    }
    count.finish(out)
}

/// The saved fights in `folder` (files that read as a fight record), only
/// those named in `ids` when there are any, by start.
fn saved_fights(folder: &Path, ids: &[String]) -> Result<Vec<FightRecord>, Failed> {
    let listing = fs::read_dir(folder).map_err(|why| Failed(format!("cannot read {}: {why}", folder.display())))?;
    let mut fights = Vec::new();
    for entry in listing.flatten() {
        // A folder fails to read as text, like any file that is not a fight.
        let Ok(text) = fs::read_to_string(entry.path()) else { continue };
        match serde_json::from_str::<FightRecord>(&text) {
            Ok(fight) if ids.is_empty() || ids.iter().any(|id| *id == fight.id) => fights.push(fight),
            _ => {}
        }
    }
    fights.sort_by(by_start);
    Ok(fights)
}

/// First and last stamp of a capture, in file order.
fn stamps(packets: &[SliceRecord]) -> Option<(i64, i64)> {
    Some((packets.first()?.captured_at_ms, packets.last()?.captured_at_ms))
}

fn holds_start(span: Option<(i64, i64)>, start_ms: i64) -> bool {
    span.is_some_and(|(first, last)| (first..=last).contains(&start_ms))
}

/// The whole capture's fight on the same boss that started nearest the
/// saved one; ties go to the earlier start, then the lower id.
fn closest_same_boss<'a>(fights: &'a [FightRecord], saved: &FightRecord) -> Option<&'a FightRecord> {
    let distance = |f: &FightRecord| (f.start_time_ms - saved.start_time_ms).abs();
    fights.iter().filter(|f| f.mob_code == saved.mob_code).min_by(|a, b| distance(a).cmp(&distance(b)).then_with(|| by_start(a, b)))
}

fn recheck_history(dir: &Path, ids: &[String], tables: &Tables, knobs: &Knobs, out: &mut dyn Write) -> Run {
    let saved = saved_fights(&dir.join("history"), ids)?;
    let captures = share::capture_files(dir);
    let spans: Vec<Option<(i64, i64)>> = captures.iter().map(|path| share::load_capture_file(path).ok().and_then(|p| stamps(&p))).collect();
    // Fights in a row mostly sit in the same captures: keep the last replay.
    let mut last: Option<(Vec<usize>, Vec<SliceRecord>, Whole)> = None;
    let mut fed = HashSet::new();
    let mut count = FightCount::default();
    for s in &saved {
        let covering: Vec<usize> = (0..captures.len()).filter(|&i| holds_start(spans[i], s.start_time_ms)).collect();
        if covering.is_empty() {
            continue;
        }
        if last.as_ref().is_none_or(|(had, ..)| *had != covering) {
            let mut packets: Vec<SliceRecord> = covering.iter().filter_map(|&i| share::load_capture_file(&captures[i]).ok()).flatten().collect();
            packets.sort_by_key(|p| p.captured_at_ms);
            let whole = whole_replay(&packets, tables);
            last = Some((covering, packets, whole));
        }
        let (_, packets, whole) = last.as_ref().expect("replayed above");
        let Some(w) = closest_same_boss(&whole.fights, s) else { continue };
        writeln!(out)?;
        writeln!(
            out,
            "== {} {} (saved record: total {}, whole capture: total {})",
            s.id, s.boss_name, s.details.total_target_damage, w.details.total_target_damage
        )?;
        count.add(cut_and_compare(w, packets, &whole.storage, tables, knobs, out)?);
        from_ring(w, packets, &whole.storage, tables, &mut fed, out)?;
    }
    count.finish(out)
}

/// The meter without a packet log keeps traffic only in its ring: feed the
/// packets there (once per packet list, as the ring is shared), let
/// `store_slice` cut the fight's slice from it into a scratch folder, and
/// rebuild the fight from that. Printed only; it does not count.
fn from_ring(w: &FightRecord, packets: &[SliceRecord], storage: &DataStorage, tables: &Tables, fed: &mut HashSet<i64>, out: &mut dyn Write) -> io::Result<()> {
    if let Some(first) = packets.first()
        && fed.insert(first.captured_at_ms)
    {
        packets.iter().for_each(|p| share::ring::record_at(p.captured_at_ms, p.stream.clone(), &p.bytes));
    }
    let scratch = std::env::temp_dir().join(format!("a2t-ring-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let rebuilt = rebuild_kept_slice(&scratch, w, storage, tables);
    let _ = fs::remove_dir_all(&scratch);
    match rebuilt {
        Ok((packed_len, d)) => {
            let same = d.details.total_target_damage == w.details.total_target_damage
                && d.details.skills.len() == w.details.skills.len()
                && d.mob_code == w.mob_code;
            let verdict = if same { "identical" } else { "DIFFERENT" };
            writeln!(out, "   from memory (no packet logging): {packed_len} bytes gzipped, total {} -> {verdict}", d.details.total_target_damage)
        }
        Err(why) => writeln!(out, "   from memory: failed: {why}"),
    }
}

/// The size `store_slice` gives for `w` kept under `app_dir`, and the fight
/// rebuilt from the file it wrote.
fn rebuild_kept_slice(app_dir: &Path, w: &FightRecord, storage: &DataStorage, tables: &Tables) -> Result<(usize, FightRecord), String> {
    let packed_len = share::store_slice(app_dir, w, storage)?;
    // The kept file's name, as the private `share::slices::slice_bin_path` makes
    // it. Read here rather than through `share::read_slice` so a failure
    // keeps its io error text.
    let kept = share::slice_folder(app_dir).join(format!("{}.a2es.gz", w.id));
    let slice = fs::read(kept).and_then(|packed| gunzip(&packed)).map_err(|why| why.to_string())?;
    match slice_rebuild::fight_from_slice(&slice, &tables.npcs, &tables.skills, &tables.dots) {
        Ok(rebuilt) => Ok((packed_len, rebuilt.record)),
        Err(why) => Err(format!("{why:?}")),
    }
}

// ── test builders ──────────────────────────────────────────────────────────

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
    use crate::built::{self, Scratch, F};
    use a2tools_dps_meter_lib::capture::fight_tape::FightTape;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    fn slice_file(records: Vec<(i32, Vec<u8>)>, names: &[(&str, u64)]) -> Vec<u8> {
        let blind_map = names.iter().map(|&(token, id)| (token.to_string(), id)).collect();
        fight_tape::encode(&FightTape { records, blind_map, stats: Default::default() })
    }

    fn gzipped(bytes: &[u8]) -> Vec<u8> {
        share::gzip(bytes).expect("gzip")
    }

    /// A packet of `opcode` holding `text` after a zero byte.
    fn text_packet(opcode: [u8; 2], text: &str) -> Vec<u8> {
        built::packet(opcode, &[F::U8(0), F::Raw(text.as_bytes()), F::U8(0)])
    }

    fn slice_report(path: &Path, strings: bool, find: Option<&str>) -> (Run, String) {
        let mut out = Vec::new();
        let ran = look_into_slice(path.to_str().expect("utf-8 path"), strings, find, &mut out);
        (ran, String::from_utf8(out).expect("utf-8 report"))
    }

    /// The opcode table of mode 1 for these expanded records.
    fn table_text(readable: &[Vec<u8>]) -> String {
        let mut out = Vec::new();
        opcode_table(readable, &mut out).expect("table");
        String::from_utf8(out).expect("utf-8")
    }

    fn report_lines(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        capture_report(text, &mut out).expect("report");
        String::from_utf8(out).expect("utf-8 report").lines().map(String::from).collect()
    }

    fn capture_of(lines: &[(&str, &[u8])]) -> String {
        lines.iter().enumerate().map(|(i, (stream, bytes))| format!("{}|{stream}|{}\n", 1_000 + i, built::to_hex(bytes))).collect()
    }

    /// A bundle frame around `inner` whose header claims `size` bytes.
    fn bundle_claiming(size: u32, inner: &[u8]) -> Vec<u8> {
        built::frame(&[&[0xFF, 0xFF][..], &size.to_le_bytes(), &lz4_flex::compress(inner)].concat())
    }

    /// The opcode rows of a mode 3 report.
    fn opcode_rows(lines: &[String]) -> Vec<String> {
        let from = lines.iter().position(|l| l == "top 25 leading opcodes:").expect("opcode header") + 1;
        lines[from..].iter().take_while(|l| !l.is_empty()).cloned().collect()
    }

    fn skill(actor: i32, code: i32, is_dot: bool, dmg: i64, time: i32) -> serde_json::Value {
        serde_json::json!({
            "actorId": actor, "code": code, "name": "", "time": time, "dmg": dmg, "multiHitCount": 0,
            "multiHitDamage": 0, "crit": 0, "parry": 0, "back": 0, "perfect": 0, "double": 0, "regen": 0, "isDot": is_dot,
        })
    }

    /// Fight on mob 2300 in dungeon `dungeon` with these skill rows.
    fn fight_with(total: i64, dungeon: i32, skills: Vec<serde_json::Value>) -> FightRecord {
        let details = serde_json::json!({ "targetId": 50_001, "totalTargetDamage": total, "battleTime": 60_000, "skills": skills });
        serde_json::from_value(serde_json::json!({
            "id": "auto_50001_1", "bossName": "B", "targetId": 50_001, "startTimeMs": 1, "durationMs": 60_000,
            "totalDamage": total, "jobs": [], "actors": [], "details": details, "mobCode": 2300, "dungeonId": dungeon, "serverId": 1001,
        }))
        .expect("a fight record")
    }

    // ── command line ──

    #[test]
    fn usage_for_no_args_help_or_a_bare_mode_flag() {
        for list in [&[][..], &["-h"], &["s.a2es", "--help"], &["--capture"], &["--derive"], &["--derive-capture"]] {
            assert!(mode(&args(list)).is_none(), "{list:?}");
        }
    }

    #[test]
    fn usage_lists_all_six_command_lines() {
        let lines = ["<slice>  ", "--strings", "--find NAME", "--capture <", "--derive <app-data-dir> [fight-id ...]", "--derive-capture <"];
        for line in lines {
            assert!(USAGE.contains(line), "{line}");
        }
        assert!(USAGE.contains("exits 1 when found"));
        assert!(USAGE.contains("what an upload sends") && USAGE.contains("whole game connection"));
    }

    #[test]
    fn slice_flags_anywhere_and_find_takes_the_next_argument() {
        let given = args(&["f.a2es", "--find", "Abcd", "--strings"]);
        let Some(Mode::Slice { path, strings, find }) = mode(&given) else { panic!("slice mode") };
        assert_eq!((path, strings, find), ("f.a2es", true, Some("Abcd")));

        let given = args(&["f.a2es", "--find"]);
        let Some(Mode::Slice { strings, find, .. }) = mode(&given) else { panic!("slice mode") };
        assert_eq!((strings, find), (false, None));
    }

    #[test]
    fn mode_flags_pick_their_mode() {
        let given = args(&["--derive", "app", "auto_1", "auto_2"]);
        let Some(Mode::History { dir, ids }) = mode(&given) else { panic!("history mode") };
        assert_eq!((dir, ids), ("app", &given[2..]));
        assert!(matches!(mode(&args(&["--capture", "p.txt"])), Some(Mode::CaptureStrings("p.txt"))));
        assert!(matches!(mode(&args(&["--derive-capture", "p.txt"])), Some(Mode::CaptureFights("p.txt"))));
    }

    // ── mode 1 ──

    #[test]
    fn plain_and_gzipped_slices_report_alike_but_for_the_size() {
        let records = vec![(-1_500, text_packet([0x04, 0x38], "zz")), (0, text_packet([0x06, 0x38], "yy"))];
        let plain = slice_file(records, &[("Tok1", 0)]);
        let dir = Scratch::new("inspect-gzip");
        let (a, a_text) = slice_report(&dir.put("f.a2es", &plain), false, None);
        let (b, b_text) = slice_report(&dir.put("f.a2es.gz", gzipped(&plain)), false, None);
        assert_eq!((a.expect("plain"), b.expect("gzipped")), (0, 0));
        let a_lines: Vec<&str> = a_text.lines().collect();
        let b_lines: Vec<&str> = b_text.lines().collect();
        assert_eq!(a_lines.len(), b_lines.len());
        assert_eq!(a_lines[1], format!("  file size        {} bytes", plain.len()));
        assert!(b_lines[1].contains(" bytes gzipped ("), "{}", b_lines[1]);
        assert_eq!(a_lines[2..], b_lines[2..]);
        assert_eq!(a_lines[2], "  records          2");
        let content = text_packet([0x04, 0x38], "zz").len() * 2;
        assert_eq!(a_lines[3], format!("  packet content   {content} bytes (decompressed)"));
        assert_eq!(a_lines[4], "  time span        1.500s  (offsets -1500 to 0 ms from fight start)");
    }

    #[test]
    fn unreadable_broken_gzip_and_foreign_files_exit_one() {
        let dir = Scratch::new("inspect-bad");
        let missing = dir.0.join("none.a2es");
        let (ran, _) = slice_report(&missing, false, None);
        assert!(ran.expect_err("unreadable").0.starts_with(&format!("cannot read {}: ", missing.display())));

        let broken = dir.put("broken.a2es.gz", [0x1F, 0x8B, 0x08, 0x00, 0x01, 0x02]);
        let (ran, _) = slice_report(&broken, false, None);
        assert!(ran.expect_err("broken").0.starts_with(&format!("{} is gzipped but will not decompress: ", broken.display())));

        let foreign = dir.put("hostname", "box\n");
        let (ran, out) = slice_report(&foreign, false, None);
        let why = ran.expect_err("not a slice").0;
        let mut lines = why.lines();
        assert_eq!(lines.next(), Some(format!("{} is not an Evidence Slice.", foreign.display()).as_str()));
        assert_eq!(lines.next(), Some(""));
        assert!(why.contains("--capture") && why.contains("PRIVACY.md"));
        assert!(out.is_empty());
    }

    #[test]
    fn span_counts_from_first_to_last_record() {
        assert!(span_line(-1_859_900, 33_200).starts_with("  time span        1893.100s  (offsets -1859900 to 33200 ms"));
        assert!(span_line(1_500, 0).contains(" -1.500s "));
        assert!(span_line(500, 0).contains(" 0.500s "));
    }

    #[test]
    fn gzip_ratio_has_one_decimal() {
        assert_eq!(size_line(186_056, 445_332, true), "  file size        186056 bytes gzipped (445332 uncompressed, 2.4x)");
        assert_eq!(size_line(0, 10, true), "  file size        0 bytes gzipped (10 uncompressed, 10.0x)");
    }

    #[test]
    fn no_span_line_without_records() {
        let dir = Scratch::new("inspect-empty");
        let (ran, out) = slice_report(&dir.put("e.a2es", slice_file(vec![], &[])), false, None);
        assert_eq!(ran.expect("empty slice"), 0);
        assert!(!out.contains("time span"));
        assert!(out.contains("  records          0\n  packet content   0 bytes (decompressed)\n\nThis build's allowlist: 14 opcodes\n  04 38  damage\n"));
    }

    #[test]
    fn foreign_opcode_gets_a_row_and_the_warning() {
        let packets = [built::packet([0x7A, 0x01], &[F::U8(1)]), built::packet([0x06, 0x38], &[F::U8(1)]), built::packet([0x06, 0x38], &[F::U8(2)])];
        let text = table_text(&[packets.concat()]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[..3], ["Opcodes present: 3 packets", "  06 38        2  party scope", "  7A 01        1  *** NOT ON THE ALLOWLIST ***"]);
        assert_eq!(lines[3], "");
        assert!(lines[4].starts_with("  1 packets are not on this build's allowlist."));
        assert!(lines[5].contains("do not trust it"));
        assert_eq!((lines.len(), lines[6]), (7, ""));
    }

    #[test]
    fn lifted_pieces_are_not_called_untrusted() {
        let packets = [built::packet(LIFT_OPCODE, &[F::U8(1)]), built::packet([0x04, 0x38], &[F::U8(1)])];
        let text = table_text(&[packets.concat()]);
        assert!(text.contains("  E5 A2        1  lifted piece\n"), "{text}");
        assert!(!text.contains("NOT ON THE ALLOWLIST") && !text.contains("trust"));
    }

    #[test]
    fn a_frame_without_opcode_bytes_counts_only_in_the_header() {
        let text = table_text(&[[built::frame(&[0x04]), built::packet([0x21, 0x36], &[])].concat()]);
        assert_eq!(text,"Opcodes present: 2 packets\n  21 36        1  map load\n\n");
    }

    #[test]
    fn equal_counts_sort_by_opcode() {
        let counts = HashMap::from([([0x42, 0x36], 3), ([0x06, 0x38], 3602), ([0x41, 0x36], 3), ([0x02, 0x97], 9)]);
        let order: Vec<[u8; 2]> = by_count(&counts).into_iter().map(|(op, _)| op).collect();
        assert_eq!(order, [[0x06, 0x38], [0x02, 0x97], [0x41, 0x36], [0x42, 0x36]]);
        let packets: Vec<u8> = (0..3602).flat_map(|i| built::packet([0x06, 0x38], &[F::V(i)])).collect();
        assert!(table_text(&[packets]).contains("\n  06 38     3602  party scope\n"));
    }

    #[test]
    fn blind_map_by_token_with_sixteen_hex_digits() {
        let map = HashMap::from([("Zq".to_string(), 0x1A2B), ("02".to_string(), 0), ("Ab".to_string(), u64::MAX)]);
        let mut out = Vec::new();
        blinded_names(&map, &mut out).expect("blind map");
        let text = String::from_utf8(out).expect("utf-8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Blinded names: 3");
        assert!(lines[1].starts_with("  ") && lines[2].starts_with("  "));
        assert_eq!(
            lines[3..],
            ["  02  <- roster id 0x0000000000000000", "  Ab  <- roster id 0xffffffffffffffff", "  Zq  <- roster id 0x0000000000001a2b", ""]
        );
    }

    // ── mode 2 ──

    #[test]
    fn search_counts_overlaps_and_stays_in_one_record() {
        assert_eq!(hits_of(b"aa", b"aaaa"), 3);
        assert_eq!(hits_of(b"", b"aaaa"), 0);
        assert_eq!(hits_of(b"abc", b"ab"), 0);
        assert_eq!(hits_of(b"ab", b"xa") + hits_of(b"ab", b"bx"), 0);
    }

    #[test]
    fn a_found_name_exits_one_before_any_string_list() {
        let records = vec![(0, text_packet([0x04, 0x38], "Mmmm Mmmm")), (10, text_packet([0x04, 0x38], "Mmmm"))];
        let dir = Scratch::new("inspect-find");
        let path = dir.put("f.a2es", slice_file(records, &[]));
        let (ran, out) = slice_report(&path, true, Some("Mmmm"));
        assert_eq!(ran.expect("report"), 1);
        assert!(out.ends_with("Search for \"Mmmm\": 3 occurrence(s) in the decompressed content\n\n"), "{out}");
        assert!(!out.contains("Printable runs"));
    }

    #[test]
    fn an_absent_name_reports_zero_and_goes_on() {
        let dir = Scratch::new("inspect-absent");
        let path = dir.put("f.a2es", slice_file(vec![(0, text_packet([0x04, 0x38], "Wxyz"))], &[]));
        let (ran, out) = slice_report(&path, false, Some("Abcd"));
        assert_eq!(ran.expect("report"), 0);
        assert!(out.contains("Search for \"Abcd\": 0 occurrence(s) in the decompressed content\n\nAdd --strings"));
        let (ran, out) = slice_report(&path, true, Some("Abcd"));
        assert_eq!(ran.expect("report"), 0);
        assert!(out.ends_with("Printable runs of 4+ bytes: 1\n  [  4] Wxyz\n"), "{out}");
    }

    #[test]
    fn text_runs_keep_whole_valid_stretches() {
        let mut found = BTreeSet::new();
        text_runs_into(&[0x00, 0x41, 0x42, 0x43, 0x44, 0x00, 0x41, 0x42, 0x43, 0x00], 4, &mut found);
        assert_eq!(longest_first(found.clone()).iter().map(|r| bracketed(r, usize::MAX)).collect::<Vec<_>>(), ["  [  4] ABCD"]);

        text_runs_into(&[0x41, 0x42, 0xC3, 0x28, 0x43, 0x44], 4, &mut found);
        text_runs_into(b"\x1fABCD\x7f~~~~", 4, &mut found);
        let hangul = [&"\u{D55C}\u{AE00}".as_bytes()[..], b"AB"].concat();
        text_runs_into(&hangul, 4, &mut found);
        text_runs_into(b"zzzzz\x00aaaaa\x00Bbbbb", 4, &mut found);
        let runs = longest_first(found);
        assert_eq!(runs, ["Bbbbb", "aaaaa", "zzzzz", "ABCD", "~~~~", "\u{D55C}\u{AE00}AB"]);
        assert_eq!(bracketed(&runs[5], usize::MAX), "  [  4] \u{D55C}\u{AE00}AB");
        assert_eq!(bracketed(&runs[0], 2), "  [  5] Bb");
    }

    // ── mode 3 ──

    #[test]
    fn capture_lines_that_do_not_parse_are_skipped() {
        assert_eq!(capture_line("x|y|0A0b"), Some(("y", vec![0x0A, 0x0B])));
        assert_eq!(capture_line("  x|y|  "), Some(("y", vec![])));
        assert_eq!(capture_line("x|y|"), Some(("y", vec![])));
        for skipped in ["", "   ", "# x|y|0A", "x|y|0A0", "x|y|0G", "x|0A", "x|y|+1", "x|y|é"] {
            assert_eq!(capture_line(skipped), None, "{skipped:?}");
        }
        let lines = report_lines("# comment\n\nx|y|0A0\nx|y|\n1|s|0A0b\n");
        assert_eq!(lines[0], "capture lines: 2, flattened packets: 0, streams: 2");
    }

    #[test]
    fn bundles_open_four_deep_and_no_deeper() {
        let inner = text_packet([0x45, 0x36], "deep inside");
        let mut nested = inner.clone();
        for _ in 0..4 {
            nested = built::bundle(&nested);
        }
        let lines = report_lines(&capture_of(&[("s", &nested)]));
        assert_eq!(lines[0], "capture lines: 1, flattened packets: 1, streams: 1");
        assert_eq!(opcode_rows(&lines), ["  45 36        1"]);
        assert!(lines.contains(&"  [ 11] deep inside".to_string()));

        let lines = report_lines(&capture_of(&[("s", &built::bundle(&nested))]));
        assert_eq!(lines[0], "capture lines: 1, flattened packets: 0, streams: 1");
    }

    #[test]
    fn bundles_claiming_nothing_or_too_much_are_skipped() {
        let inner = text_packet([0x45, 0x36], "never read");
        for size in [0, 1_000_001] {
            let lines = report_lines(&capture_of(&[("s", &bundle_claiming(size, &inner))]));
            assert_eq!(lines[0], "capture lines: 1, flattened packets: 0, streams: 1", "{size}");
        }
        assert_eq!(report_lines(&capture_of(&[("s", &built::bundle(&inner))]))[0], "capture lines: 1, flattened packets: 1, streams: 1");
    }

    #[test]
    fn a_packet_cut_over_two_lines_of_one_stream_is_found() {
        let whole = text_packet([0x41, 0x36], "split here");
        let (head, tail) = whole.split_at(5);
        let lines = report_lines(&capture_of(&[("a", head), ("b", &[0x00]), ("a", tail)]));
        assert_eq!(lines[0], "capture lines: 3, flattened packets: 1, streams: 2");
        assert_eq!(opcode_rows(&lines), ["  41 36        1"]);
        assert!(lines.contains(&"  [ 10] split here".to_string()));

        let lines = report_lines(&capture_of(&[("a", head), ("b", tail)]));
        assert_eq!(lines[0], "capture lines: 2, flattened packets: 0, streams: 2");
    }

    #[test]
    fn a_stream_begun_mid_frame_still_shows_the_frames_after() {
        // The joined walk reads `10` as a length that takes in the whole
        // next line and waits for more; the line on its own frames.
        let lines = report_lines("1|s|10\n2|s|0E41364162636465666768\n");
        assert_eq!(lines[0], "capture lines: 2, flattened packets: 1, streams: 1");
        assert_eq!(opcode_rows(&lines), ["  41 36        1"]);
        assert!(lines.contains(&"  [ 10] A6Abcdefgh".to_string()));
    }

    #[test]
    fn a_frame_both_walks_find_counts_once() {
        let first = text_packet([0x04, 0x38], "first one");
        let second = text_packet([0x04, 0x38], "second one");
        let lines = report_lines(&capture_of(&[("s", &first), ("s", &[first.clone(), second].concat())]));
        assert_eq!(lines[0], "capture lines: 2, flattened packets: 3, streams: 1");
        assert_eq!(opcode_rows(&lines), ["  04 38        3"]);
    }

    #[test]
    fn capture_report_caps_rows_runs_and_run_width() {
        let mut bytes = Vec::new();
        for i in 0..30u8 {
            for _ in 0..=i {
                bytes.extend(built::packet([0x10, i], &[F::U8(0)]));
            }
        }
        for i in 0..70 {
            bytes.extend(text_packet([0x11, 0x01], &format!("run{i:03}")));
        }
        let wide = "w".repeat(150);
        bytes.extend(text_packet([0x11, 0x01], &wide));
        let lines = report_lines(&capture_of(&[("s", &bytes)]));
        let rows = opcode_rows(&lines);
        assert_eq!(rows.len(), 25);
        assert_eq!(rows[0], "  11 01       71");
        assert_eq!(rows[1], "  10 1D       30");

        let from = lines.iter().position(|l| l == "longest 60:").expect("run header");
        assert_eq!(lines[from - 1], "distinct printable runs >= 6 bytes: 71");
        let runs = &lines[from + 1..];
        assert_eq!(runs.len(), 60);
        assert_eq!(runs[0], format!("  [150] {}", "w".repeat(120)));
        assert_eq!(runs[1], "  [  6] run000");
    }

    // ── mode 4 ──

    #[test]
    fn a_dungeon_mismatch_alone_is_still_identical() {
        let w = fight_with(2_000, 310_046, vec![skill(1_480, 11_020_000, false, 9_556, 12)]);
        let d = fight_with(2_000, 0, vec![skill(1_480, 11_020_000, false, 9_556, 12)]);
        let (lines, same) = compare(&w, &d, 831_039, 402_218, false);
        assert!(same);
        assert_eq!(
            lines,
            ["   slice 831039 bytes (402218 gzipped): boss 2300 target 50001/50001 ok  dungeon 310046/0 MISMATCH  server 1001/1001 ok  total 2000 / 2000 over 60000 / 60000 ms  skill rows 1 / 1  -> identical"]
        );
    }

    #[test]
    fn one_differing_row_makes_it_different() {
        let w = fight_with(2_000, 0, vec![skill(1_480, 11_020_000, false, 9_556, 12), skill(1_480, 11_020_000, true, 5, 1)]);
        let d = fight_with(2_000, 0, vec![skill(1_480, 11_020_000, false, 9_500, 12), skill(1_480, 11_020_000, true, 5, 1)]);
        let (lines, same) = compare(&w, &d, 10, 5, false);
        assert!(!same);
        assert!(lines[0].ends_with("skill rows 2 / 2  -> DIFFERENT"), "{}", lines[0]);
        assert_eq!(lines[1..], ["     row (1480, 11020000, false): whole Some((9556, 12)) slice Some((9500, 12))"]);

        let d = fight_with(2_000, 0, vec![]);
        let (lines, _) = compare(&w, &d, 10, 5, false);
        assert_eq!(lines[1], "     row (1480, 11020000, false): whole Some((9556, 12)) slice None");
        assert_eq!(lines[2], "     row (1480, 11020000, true): whole Some((5, 1)) slice None");
    }

    #[test]
    fn boss_or_total_alone_makes_it_different() {
        let w = fight_with(2_000, 0, vec![]);
        let mut d = fight_with(2_000, 0, vec![]);
        d.mob_code = 2301;
        let (lines, same) = compare(&w, &d, 1, 1, false);
        assert!(!same && lines[0].contains(": boss 2301 target") && lines.len() == 1);

        let d = fight_with(1_999, 0, vec![]);
        let (lines, same) = compare(&w, &d, 1, 1, false);
        assert!(!same && lines[0].contains("total 2000 / 1999 over"));
    }

    #[test]
    fn twelve_rows_unless_all_are_asked_for() {
        let w = fight_with(1, 0, (0..20).map(|i| skill(1_500, 100 + i, false, 1, 1)).collect());
        let d = fight_with(1, 0, vec![]);
        assert_eq!(compare(&w, &d, 1, 1, false).0.len(), 1 + 12);
        let (all, _) = compare(&w, &d, 1, 1, true);
        assert_eq!(all.len(), 1 + 20);
        assert_eq!(all[20], "     row (1500, 119, false): whole Some((1, 1)) slice None");
    }

    #[test]
    fn the_nearest_start_on_the_same_boss_wins_ties_to_the_earlier() {
        let on = |id: &str, start: i64, mob: i32| {
            let mut f = built::fight(id, start, 1_000);
            f.mob_code = mob;
            f
        };
        let saved = on("saved", 10_000, 2300);
        let fights = [on("far", 1_000, 2300), on("other boss", 10_000, 2400), on("late", 12_000, 2300), on("early", 8_000, 2300)];
        assert_eq!(closest_same_boss(&fights, &saved).map(|f| f.id.as_str()), Some("early"));
        assert_eq!(closest_same_boss(&fights[..1], &saved).map(|f| f.id.as_str()), Some("far"));
        assert!(closest_same_boss(&fights[1..2], &saved).is_none());
    }

    #[test]
    fn only_a_capture_that_spans_the_start_covers_it() {
        let at = |ms: i64| SliceRecord { captured_at_ms: ms, stream: "s".into(), bytes: vec![1] };
        let spanning = stamps(&[at(5_000), at(9_000), at(20_000)]);
        let ended_before = stamps(&[at(1_000), at(9_999)]);
        assert!(holds_start(spanning, 10_000) && holds_start(spanning, 20_000));
        assert!(!holds_start(ended_before, 10_000));
        assert!(!holds_start(stamps(&[]), 10_000));
    }

    #[test]
    fn exit_status_needs_every_fight_identical_and_at_least_one() {
        let ended = |tried: usize, same: usize| {
            let mut out = Vec::new();
            let status = FightCount { tried, same }.finish(&mut out).expect("written");
            (status, String::from_utf8(out).expect("utf-8"))
        };
        assert_eq!(ended(3, 3), (0, "\n3 of 3 fights: slice re-derived identically to the whole capture\n".to_string()));
        assert_eq!(ended(3, 2).0, 1);
        assert_eq!(ended(0, 0), (1, "\n0 of 0 fights: slice re-derived identically to the whole capture\n".to_string()));
    }

    #[test]
    fn saved_fights_skip_other_files_and_filter_by_id() {
        let dir = Scratch::new("inspect-history");
        for (id, start) in [("b", 20), ("a", 20), ("c", 10)] {
            dir.put(&format!("history/{id}.json"), serde_json::to_vec(&built::fight(id, start, 5)).expect("json"));
        }
        dir.put("history/notes.txt", "not a fight");
        let all: Vec<String> = saved_fights(&dir.0.join("history"), &[]).expect("listed").into_iter().map(|f| f.id).collect();
        assert_eq!(all, ["c", "a", "b"]);
        let some = saved_fights(&dir.0.join("history"), &args(&["b", "zz"])).expect("listed");
        assert_eq!(some.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["b"]);
        assert!(saved_fights(&dir.0.join("missing"), &[]).expect_err("no folder").0.starts_with("cannot read "));
    }

    // ── by hand, on a real capture (A2_REPLAY_CAPTURE) ──

    #[test]
    #[ignore]
    fn capture_mode_reads_a_real_capture() {
        let Some(path) = std::env::var_os("A2_REPLAY_CAPTURE") else { return };
        let mut out = Vec::new();
        assert_eq!(look_into_capture(path.to_str().expect("utf-8 path"), &mut out).expect("report"), 0);
        let lines: Vec<String> = String::from_utf8_lossy(&out).lines().map(String::from).collect();
        let count: usize = lines[0].strip_prefix("capture lines: ").and_then(|r| r.split(',').next()).and_then(|n| n.parse().ok()).expect("line count");
        assert!(count > 0);
        assert!(!opcode_rows(&lines).is_empty());
    }

    #[test]
    #[ignore]
    fn derive_capture_matches_on_a_real_capture() {
        let Some(path) = std::env::var_os("A2_REPLAY_CAPTURE") else { return };
        let tables = Tables::load().expect("tables");
        let mut out = Vec::new();
        let status = recheck_capture(path.to_str().expect("utf-8 path"), &tables, &Knobs::default(), &mut out).expect("report");
        let text = String::from_utf8_lossy(&out);
        let last = text.lines().last().expect("a last line");
        let (same, rest) = last.split_once(" of ").expect("N of N");
        assert!(rest.starts_with(&format!("{same} fights: ")), "{last}");
        assert!(same.parse::<usize>().expect("a count") > 0);
        assert_eq!(status, 0);
    }
}
