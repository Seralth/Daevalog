//! The name blinder and the `.a2es` file format.
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;

use sha2::{Digest, Sha256};

use super::{carries_event, wrap_record, pack_bundle, SliceCounts, KEPT_OPCODES};
use crate::capture::framing::{self, NEST_LIMIT};
use crate::capture::opcodes::{DEATH, PLAYER_SPAWN, PLAYER_SPAWN_OLD, SELF_IDENTITY, SPAWN, SPAWN_OLD, ZONE_CHANGE};
use crate::capture::decoder::{name_fields, name_fields_unframed, NameField};

/// Character name to roster id (0 when the meter never learned one).
pub type NameMap = HashMap<String, u64>;

/// Why no slice came out of a fight.
#[derive(Debug)]
pub enum SliceFailure {
    /// A name of this many bytes was still readable after blinding.
    NameLeaked(usize),
    /// The fight's window held no packet worth keeping.
    Empty,
}

impl fmt::Display for SliceFailure {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result {
        // The length only: the message may end up in a log or a dialog.
        let line = match self {
            Self::NameLeaked(len) => format!("a {len}-byte name survived blinding, so the slice was thrown away"),
            Self::Empty => "nothing in the fight's time window could go into a slice".to_owned(),
        };
        out.write_str(&line)
    }
}

/// Shorter names are not searched for: a single byte turns up everywhere, so
/// those are replaced only where a name field holds them.
pub(super) const SHORTEST_SEARCHED: usize = 2;
/// The longest stretch after a length byte that is taken for a name.
const LONGEST_RUN: usize = 40;
/// Redraws a minted token gets before it is kept whatever it spells.
const LAST_ATTEMPT: u32 = 15;
/// What one-byte tokens and the bug-report spelling draw their letters from.
const LETTERS: &[u8; 52] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
/// Opcodes that may start a record carried inside another, after a two-byte
/// length ending `02`.
const INNER_OPCODES: [[u8; 2]; 7] = [ZONE_CHANGE, SELF_IDENTITY, SPAWN_OLD, SPAWN, DEATH, PLAYER_SPAWN_OLD, PLAYER_SPAWN];
/// Entity ids are at most this.
const HIGHEST_ENTITY: i32 = 9_999_999;

/// How a token is spelled.
enum Spelling {
    /// Hex digits: what a slice uploads (`hex_token`).
    Hex,
    /// Letters of the same UTF-8 widths, one for each character of the name,
    /// so the parser still reads the token as a name (one to twelve
    /// characters, letters and digits): the bug-report copy of a packet log.
    /// Hex tokens of all digits, or of more than twelve characters for a long
    /// Hangul name, were not names to it, and those players went unnamed.
    #[cfg_attr(not(feature = "backend"), allow(dead_code))]
    Letters {
        /// name -> token, so a name is spelled the same everywhere.
        given: HashMap<String, String>,
        /// Every token given, and every known name, so no two names share one.
        taken: HashSet<String>,
    },
}

/// Replaces names in packets with tokens of the same byte length.
///
/// Three passes over each packet: the name fields the parser reads, every
/// known name wherever it appears, then any length-prefixed stretch that reads
/// like a name. Tokens depend only on the names and the bytes, so the same
/// fight always blinds the same way.
pub(crate) struct Blinder {
    spelling: Spelling,
    /// Each known name and the token that stands for it.
    by_name: HashMap<String, Vec<u8>>,
    /// Known names of `SHORTEST_SEARCHED` bytes or more, with their tokens, in the
    /// order they are searched for.
    searched: Vec<(Vec<u8>, Vec<u8>)>,
    /// Tokens already in place, which the run pass must not mint over.
    placed: HashSet<Vec<u8>>,
    /// One-byte names and one-byte tokens handed out so far (hex spelling).
    single_used: HashSet<u8>,
    /// The first one-byte token each (byte, roster id) got.
    single_first: HashMap<(u8, u64), u8>,
    /// Tokens minted for names nobody resolved, each with roster id 0.
    pub(super) discovered: HashMap<String, u64>,
}

impl Blinder {
    /// The slice blinder, hex spelling. `ordered` comes from `in_search_order`,
    /// so a name is replaced before any shorter name inside it.
    pub(super) fn new(ordered: &[(&String, &u64)]) -> Self {
        Self::over(ordered, Spelling::Hex)
    }

    /// The bug-report blinder: letter spelling, so the copy still parses.
    #[cfg_attr(not(feature = "backend"), allow(dead_code))]
    pub(crate) fn for_report(names: &NameMap) -> Self {
        let taken = names.keys().cloned().collect();
        Self::over(&in_search_order(names), Spelling::Letters { given: HashMap::new(), taken })
    }

    fn over(ordered: &[(&String, &u64)], spelling: Spelling) -> Self {
        let mut blinder = Blinder {
            spelling,
            by_name: HashMap::new(),
            searched: Vec::new(),
            placed: HashSet::new(),
            single_used: ordered.iter().filter(|(name, _)| name.len() == 1).map(|(name, _)| name.as_bytes()[0]).collect(),
            single_first: HashMap::new(),
            discovered: HashMap::new(),
        };
        for &(name, &id) in ordered {
            if name.is_empty() {
                continue;
            }
            let token = blinder.draw(name, id, 0);
            if name.len() >= SHORTEST_SEARCHED {
                blinder.searched.push((name.as_bytes().to_vec(), token.clone()));
            }
            blinder.placed.insert(token.clone());
            blinder.by_name.insert(name.clone(), token);
        }
        blinder
    }

    /// The token a known name gets.
    pub(super) fn token_of(&self, name: &str) -> Option<&[u8]> {
        self.by_name.get(name).map(Vec::as_slice)
    }

    /// Blind one framed packet in place. Returns how many places changed.
    pub(crate) fn blind(&mut self, buf: &mut [u8]) -> usize {
        // Events hold ids and numbers, and some ids read as text.
        // Scanning them blinded skill ids that read as text: `02 | 50 77 f6 00`
        // (Water Spirit: Ice Chain) is "Pw".
        let mut changed = 0;
        if !carries_event(buf) {
            let fields = name_fields(buf);
            changed += self.fill_fields(buf, &fields);
        }
        changed += self.replace_known(buf);
        // Asked again: a known name may have written over the opcode (names
        // can start with a digit), and the baseline slices took what was left
        // for a packet that is no event.
        if !carries_event(buf) {
            // Past the length varint and the two opcode bytes; 0 if the
            // varint does not read.
            let head = super::varint::varint_at(buf, 0);
            let scan_from = if head.length > 0 { head.length as usize + 2 } else { 0 };
            changed += self.mint_runs(buf, scan_from);
        }
        changed
    }

    /// Blind bytes that are not one framed packet (a stretch of the log):
    /// no event test, but record heads inside are passed over as in `blind`.
    #[cfg_attr(not(feature = "backend"), allow(dead_code))]
    pub(crate) fn blind_unframed(&mut self, buf: &mut [u8]) -> usize {
        let fields = name_fields_unframed(buf);
        let mut changed = self.fill_fields(buf, &fields);
        changed += self.replace_known(buf);
        changed + self.mint_runs(buf, 0)
    }

    /// Pass one: every name field, at any length.
    fn fill_fields(&mut self, buf: &mut [u8], fields: &[NameField]) -> usize {
        let mut changed = 0;
        let mut free_from = 0;
        for field in fields {
            let at = field.range.clone();
            if at.is_empty() || at.end > buf.len() || at.start < free_from {
                continue;
            }
            // Known names are text, so bytes that are not cannot be one.
            let Ok(text) = std::str::from_utf8(&buf[at.clone()]) else { continue };
            let text = text.to_owned();
            if let Some(token) = self.by_name.get(&text) {
                buf[at.clone()].copy_from_slice(token);
            } else {
                let token = self.mint(buf, at.clone(), &text);
                self.placed.insert(token);
            }
            free_from = at.end;
            changed += 1;
        }
        changed
    }

    /// Pass two: each searched name, wherever its bytes are.
    fn replace_known(&self, buf: &mut [u8]) -> usize {
        // Short letter tokens look like any two letters, so a short name is
        // only taken where its length byte stands in front of it.
        let needs_length = matches!(self.spelling, Spelling::Letters { .. });
        let mut changed = 0;
        for (name, token) in &self.searched {
            let width = name.len();
            let mut at = 0;
            while at + width <= buf.len() {
                let fronted = !needs_length || width >= 3 || (at > 0 && usize::from(buf[at - 1]) == width);
                if fronted && buf[at..at + width] == name[..] {
                    buf[at..at + width].copy_from_slice(token);
                    changed += 1;
                    at += width;
                } else {
                    at += 1;
                }
            }
        }
        changed
    }

    /// Pass three: a length byte followed by that many bytes of text.
    /// Tokens minted here deliberately stay out of `placed` (rule 12): a later
    /// run spelling one is minted over again, and the baseline slices depend
    /// on that.
    fn mint_runs(&mut self, buf: &mut [u8], from: usize) -> usize {
        let mut changed = 0;
        let mut at = from;
        while at < buf.len() {
            let width = usize::from(buf[at]);
            let end = at + 1 + width;
            let run = (2..=LONGEST_RUN).contains(&width)
                && end <= buf.len()
                && reads_as_text(&buf[at + 1..end])
                && !self.placed.contains(&buf[at + 1..end])
                && !inner_header_at(buf, at);
            if !run {
                at += 1;
                continue;
            }
            let text = String::from_utf8_lossy(&buf[at + 1..end]).into_owned();
            self.mint(buf, at + 1..end, &text);
            changed += 1;
            at = end;
        }
        changed
    }

    /// Write a token for an unresolved name over `at`, redrawing while a
    /// searched name reads across it with the bytes around it. The token goes
    /// into `discovered`.
    fn mint(&mut self, buf: &mut [u8], at: Range<usize>, text: &str) -> Vec<u8> {
        let mut attempt = 0;
        loop {
            let token = self.draw(text, 0, attempt);
            buf[at.clone()].copy_from_slice(&token);
            if attempt == LAST_ATTEMPT || !self.crosses_searched(buf, &at) {
                self.discovered.insert(String::from_utf8_lossy(&token).into_owned(), 0);
                return token;
            }
            attempt += 1;
        }
    }

    /// Does any searched name overlap `at` by at least one byte?
    fn crosses_searched(&self, buf: &[u8], at: &Range<usize>) -> bool {
        self.searched.iter().any(|(name, _)| {
            let first = (at.start + 1).saturating_sub(name.len());
            (first..at.end).any(|p| buf.get(p..p + name.len()) == Some(name.as_slice()))
        })
    }

    /// The token for `text` on its `attempt`th try, in this blinder's spelling.
    fn draw(&mut self, text: &str, id: u64, attempt: u32) -> Vec<u8> {
        if let Spelling::Letters { given, taken } = &mut self.spelling {
            return pick_letters(given, taken, text, attempt).into_bytes();
        }
        match text.as_bytes() {
            &[byte] => vec![self.single_letter(byte, id, attempt)],
            bytes => hex_token(bytes, id, attempt),
        }
    }

    /// A one-byte name's token: a letter that is neither the name nor any
    /// other one-byte name or token.
    fn single_letter(&mut self, byte: u8, id: u64, attempt: u32) -> u8 {
        if let Some(&letter) = self.single_first.get(&(byte, id)).filter(|_| attempt == 0) {
            return letter;
        }
        let id_bytes = id.to_le_bytes();
        let source: &[u8] = if id == 0 { std::slice::from_ref(&byte) } else { &id_bytes };
        let digest = sha256(&[b"a2es-one\0", source, &attempt.to_le_bytes()]);
        let free = |c: &u8| *c != byte && !self.single_used.contains(c);
        let letter = digest
            .iter()
            .map(|d| LETTERS[usize::from(d % 52)])
            .find(free)
            .or_else(|| LETTERS.iter().copied().find(free))
            .unwrap_or(b'q');
        self.single_used.insert(letter);
        if attempt == 0 {
            self.single_first.insert((byte, id), letter);
        }
        letter
    }
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// A hex token as long as `name`. A roster id decides it when there is one,
/// so a player keeps one token under any name; otherwise the name does, with
/// the attempt number added for a redraw. Past 64 digits it is padded with `0`.
fn hex_token(name: &[u8], id: u64, attempt: u32) -> Vec<u8> {
    let digest = if id != 0 {
        sha256(&[b"a2es-dbid\0", &id.to_le_bytes()])
    } else if attempt == 0 {
        sha256(&[b"a2es-name\0", name])
    } else {
        sha256(&[b"a2es-name\0", name, b"\0", attempt.to_string().as_bytes()])
    };
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut token: Vec<u8> = digest.iter().flat_map(|b| [DIGITS[usize::from(b >> 4)], DIGITS[usize::from(b & 15)]]).collect();
    token.resize(name.len(), b'0');
    token
}

/// A letter token for `name` and `seed`: each character becomes one of the
/// same UTF-8 width (ASCII letters, Cyrillic, CJK, CJK extension B).
fn letters_of(name: &str, seed: u32) -> String {
    let mut digest = [0; 32];
    let mut out = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        let slot = i % 16;
        if slot == 0 {
            let block = (i / 16) as u32;
            digest = sha256(&[b"a2rl\0", name.as_bytes(), &seed.to_le_bytes(), &block.to_le_bytes()]);
        }
        let v = u32::from(u16::from_le_bytes([digest[2 * slot], digest[2 * slot + 1]]));
        let (base, span) = match ch.len_utf8() {
            1 => {
                out.push(char::from(LETTERS[(v % 52) as usize]));
                continue;
            }
            2 => (0x0410, 64),
            3 => (0x4E00, 20_900),
            _ => (0x2_0000, 42_000),
        };
        out.push(char::from_u32(base + v % span).expect("inside a valid block"));
    }
    out
}

/// The letter token for `name` on try `attempt`: the first of a thousand
/// seeds whose token nobody holds (the last seed's if all are held).
fn pick_letters(given: &mut HashMap<String, String>, taken: &mut HashSet<String>, name: &str, attempt: u32) -> String {
    if let Some(token) = given.get(name).filter(|_| attempt == 0) {
        return token.clone();
    }
    let seeds = 1000 * attempt..1000 * attempt + 1000;
    let last = seeds.end - 1;
    let token = seeds
        .map(|seed| letters_of(name, seed))
        .find(|token| !taken.contains(token))
        .unwrap_or_else(|| letters_of(name, last));
    taken.insert(token.clone());
    if attempt == 0 {
        given.insert(name.to_owned(), token.clone());
    }
    token
}

/// Text a name could be: printable UTF-8, at least half letters and digits,
/// and all of them when three characters or fewer.
fn reads_as_text(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else { return false };
    if text.chars().any(char::is_control) {
        return false;
    }
    let chars = text.chars().count();
    let alnum = text.chars().filter(|c| c.is_alphanumeric()).count();
    2 * alnum >= chars && (chars > 3 || alnum == chars)
}

/// Is `buf[at]` the `02` ending a two-byte length in front of a record that
/// rides inside this one? `<.. 02> 40 36 <id>` reads like the run "@6".
fn inner_header_at(buf: &[u8], at: usize) -> bool {
    if at == 0 || at + 3 >= buf.len() || buf[at] != 0x02 || buf[at - 1] < 0x80 {
        return false;
    }
    if !INNER_OPCODES.contains(&[buf[at + 1], buf[at + 2]]) {
        return false;
    }
    let id = super::varint::varint_at(buf, at + 3);
    id.length > 0 && (0..=HIGHEST_ENTITY).contains(&id.value)
}

/// Appends the game's varint: seven bits a byte, low bits first.
pub(super) fn write_varint(out: &mut Vec<u8>, value: u32) {
    let mut left = value;
    while left >= 0x80 {
        out.push(left as u8 | 0x80);
        left >>= 7;
    }
    out.push(left as u8);
}

/// The names longest first (ties in byte order), so a name inside a longer
/// one is never replaced first and the order never depends on the map's.
pub(super) fn in_search_order(names: &NameMap) -> Vec<(&String, &u64)> {
    let mut ordered: Vec<(&String, &u64)> = names.iter().collect();
    ordered.sort_by_key(|&(name, _)| (std::cmp::Reverse(name.len()), name.as_str()));
    ordered
}

// The `.a2es` container, little-endian throughout: magic, version, the
// opcode list, the blind map (token, roster id) and the records (ms offset,
// length, bytes).
pub const MAGIC: &[u8; 4] = b"A2ES";

/// 2: lengths follow the corrected framing rule. Version 1 slices are read by
/// re-framing their records (see `upgrade_v1_record`).
pub const VERSION: u16 = 2;
/// What `encode` writes: the log service only reads version 1, so records go
/// out in version 1 lengths (see `downgrade_to_v1`).
const WIRE_VERSION: u16 = 1;

/// One fight, cut from the capture and blinded: what a share uploads.
#[derive(Debug)]
pub struct FightTape {
    /// Framed packets and framed bundles in capture order, each with its time
    /// in ms from the fight start (negative before it).
    pub records: Vec<(i32, Vec<u8>)>,
    /// Token to roster id.
    pub blind_map: HashMap<String, u64>,
    pub stats: SliceCounts,
}

/// The file bytes of a slice. The blind map goes out in token order and the
/// opcode list in `KEPT_OPCODES` order, so equal slices give equal files.
pub fn encode(slice: &FightTape) -> Vec<u8> {
    let mut entries: Vec<(&[u8], u64)> = slice.blind_map.iter().map(|(token, &id)| (token.as_bytes(), id)).collect();
    entries.sort_unstable();
    // Counts and lengths keep their low bits; nothing larger occurs.
    let mut file = Vec::new();
    file.extend(MAGIC);
    file.extend(WIRE_VERSION.to_le_bytes());
    file.extend((KEPT_OPCODES.len() as u16).to_le_bytes());
    file.extend(KEPT_OPCODES.iter().flat_map(|&(pair, _)| *pair));
    file.extend((entries.len() as u16).to_le_bytes());
    for (token, id) in entries {
        file.push(token.len() as u8);
        file.extend(token);
        file.extend(id.to_le_bytes());
    }
    file.extend((slice.records.len() as u32).to_le_bytes());
    for (ms, record) in &slice.records {
        let old_lengths = downgrade_to_v1(record);
        file.extend(ms.to_le_bytes());
        file.extend((old_lengths.len() as u16).to_le_bytes());
        file.extend(old_lengths);
    }
    file
}

/// Re-frame a record for version 1 readers, the reverse of
/// `upgrade_v1_record`: the old walk takes `len - 3` bytes for a frame and one
/// more for a top-level bundle. Each frame keeps its bytes.
fn downgrade_to_v1(record: &[u8]) -> Vec<u8> {
    fn v1_prefix(body: usize, top_bundle: bool) -> Option<Vec<u8>> {
        (1usize..=5).find_map(|n| {
            let mut v = Vec::with_capacity(5);
            write_varint(&mut v, (n + body + 3 - usize::from(top_bundle)) as u32);
            (v.len() == n).then_some(v)
        })
    }
    fn reframe(buf: &[u8], top: bool, depth: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(buf.len() + 8);
        let mut o = 0;
        while o < buf.len() {
            if buf[o] == 0x00 {
                out.push(0x00);
                o += 1;
                continue;
            }
            let len = super::varint::varint_at(buf, o);
            if len.length <= 0 {
                break;
            }
            let n = len.length as usize;
            let Some(size) = framing::frame_size(len.value, len.length) else { break };
            if size < n || o + size > buf.len() {
                break;
            }
            let body = &buf[o + n..o + size];
            let bundle = body.len() > 1 && body[0] == 0xFF && body[1] == 0xFF;
            let inner = (bundle && depth < NEST_LIMIT)
                .then(|| framing::inflate_block(body).and_then(|i| reframe(&i, false, depth + 1)))
                .flatten();
            let body = match inner {
                Some(inner) => {
                    let mut p = vec![0xFF, 0xFF];
                    p.extend_from_slice(&(inner.len() as u32).to_le_bytes());
                    p.extend_from_slice(&lz4_flex::compress(&inner));
                    p
                }
                None => body.to_vec(),
            };
            out.extend_from_slice(&v1_prefix(body.len(), bundle && top)?);
            out.extend_from_slice(&body);
            o += size;
        }
        out.extend_from_slice(&buf[o..]);
        Some(out)
    }
    reframe(record, true, 0).unwrap_or_else(|| record.to_vec())
}

/// Records and blind map of a slice file, version 1 or 2. Version 1 records
/// come back in current lengths. `None` for anything that is not a whole
/// slice; bytes after the last record are ignored.
pub fn decode(data: &[u8]) -> Option<(Vec<(i32, Vec<u8>)>, HashMap<String, u64>)> {
    let mut file = Reader { rest: data };
    if file.take(4)? != MAGIC {
        return None;
    }
    let version = u16::from_le_bytes(file.fixed()?);
    if version != WIRE_VERSION && version != VERSION {
        return None;
    }
    let opcodes = usize::from(u16::from_le_bytes(file.fixed()?));
    file.take(2 * opcodes)?;
    let mut blind_map = HashMap::new();
    for _ in 0..u16::from_le_bytes(file.fixed()?) {
        let [width] = file.fixed()?;
        let token = std::str::from_utf8(file.take(usize::from(width))?).ok()?;
        blind_map.insert(token.to_owned(), u64::from_le_bytes(file.fixed()?));
    }
    let mut records = Vec::new();
    for _ in 0..u32::from_le_bytes(file.fixed()?) {
        let offset = i32::from_le_bytes(file.fixed()?);
        let width = usize::from(u16::from_le_bytes(file.fixed()?));
        let record = file.take(width)?;
        let record = if version == WIRE_VERSION { upgrade_v1_record(record) } else { record.to_vec() };
        records.push((offset, record));
    }
    Some((records, blind_map))
}

/// Reads fields off the front of a file.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let head = self.rest.get(..n)?;
        self.rest = &self.rest[n..];
        Some(head)
    }

    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
}

/// Re-frame a version 1 record for the current walk.
///
/// Version 1 was cut with the old rule: a frame spans `len - 3` bytes, plus one
/// for a top-level bundle. Each frame keeps exactly the bytes it had then and
/// gets a length the current walk reads as that span, so the parser sees what
/// it saw when the slice was made.
fn upgrade_v1_record(record: &[u8]) -> Vec<u8> {
    fn reframe(buf: &[u8], top: bool, depth: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(buf.len() + 8);
        let mut o = 0;
        while o < buf.len() {
            if buf[o] == 0x00 {
                out.push(0x00);
                o += 1;
                continue;
            }
            let len = super::varint::varint_at(buf, o);
            if len.length <= 0 || len.value <= 3 {
                break;
            }
            let n = len.length as usize;
            let bundle = buf.len() > o + n + 1 && buf[o + n] == 0xFF && buf[o + n + 1] == 0xFF;
            let size = len.value as usize - 3 + usize::from(bundle && top);
            if size < n || o + size > buf.len() {
                break;
            }
            let body = &buf[o + n..o + size];
            let rewritten = match bundle {
                true if depth < NEST_LIMIT => framing::inflate_block(body)
                    .and_then(|inner| reframe(&inner, false, depth + 1))
                    .and_then(|inner| pack_bundle(&inner)),
                _ => None,
            };
            match rewritten {
                Some(b) => out.extend_from_slice(&b),
                None => out.extend_from_slice(&wrap_record(body)?),
            }
            o += size;
        }
        out.extend_from_slice(&buf[o..]);
        Some(out)
    }
    reframe(record, true, 0).unwrap_or_else(|| record.to_vec())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{cap, framed};
    use super::super::{build, SliceRecord};
    use super::*;
    use crate::built::{self, F};
    use crate::capture::decoder::NameRecord;
    use crate::capture::varint::index_of_bytes;
    use std::sync::Arc;

    /// Where the blind map's entry count sits in a file: after the magic,
    /// the version, the opcode count and the opcodes.
    const BLIND_COUNT_AT: usize = 4 + 2 + 2 + 2 * KEPT_OPCODES.len();

    fn names(list: &[(&str, u64)]) -> NameMap {
        list.iter().map(|&(name, id)| (name.to_owned(), id)).collect()
    }

    fn blinder_for(list: &[(&str, u64)]) -> Blinder {
        Blinder::new(&in_search_order(&names(list)))
    }

    fn padded(body: &[u8], len: usize) -> Vec<u8> {
        [body, &vec![0; len - body.len()]].concat()
    }

    fn varint_of(value: u32) -> Vec<u8> {
        let mut out = Vec::new();
        write_varint(&mut out, value);
        out
    }

    /// `build` over one packet framed from `payload`, in a fight from 0 ms.
    fn built_from(payload: &[u8], at_ms: i64, fight_ms: i64, list: &[(&str, u64)]) -> FightTape {
        build(&[cap(at_ms, payload)], 0, fight_ms, &names(list)).unwrap()
    }

    fn blinded_by(blinder: &mut Blinder, packet: &[u8]) -> Vec<u8> {
        let mut copy = packet.to_vec();
        blinder.blind(&mut copy);
        copy
    }

    /// `record` against `source`: outside `ranges`, a byte may differ only
    /// where a blinder that knows no names changes it as well.
    fn differs_only_in(record: &[u8], source: &[u8], ranges: &[Range<usize>]) {
        assert_eq!(record.len(), source.len());
        let nameless = blinded_by(&mut Blinder::new(&[]), source);
        for i in 0..source.len() {
            if ranges.iter().any(|r| r.contains(&i)) {
                continue;
            }
            assert!(record[i] == source[i] || nameless[i] != source[i], "byte {i} changed");
        }
    }

    fn replay(records: &[(i32, Vec<u8>)]) -> Arc<crate::combat::data_storage::DataStorage> {
        let storage = Arc::new(crate::combat::data_storage::DataStorage::new());
        let mut processor = crate::capture::decoder::PacketDecoder::new(
            storage.clone(),
            Arc::new(crate::i18n::lookup::SkillLookup::new()),
            Arc::new(crate::i18n::lookup::NpcLookup::new()),
        );
        for (_, record) in records {
            processor.decode_buffer(record);
        }
        storage
    }

    fn one_capture(ms: i64, records: &[Vec<u8>]) -> Vec<SliceRecord> {
        vec![SliceRecord { captured_at_ms: ms, stream: "Client:1".into(), bytes: records.concat() }]
    }

    /// The 73 bytes of the container example.
    fn example_file() -> Vec<u8> {
        let mut out = b"A2ES".to_vec();
        out.extend([0x01, 0x00, 0x0e, 0x00]);
        out.extend([0x04, 0x38, 0x05, 0x38, 0x1b, 0x92, 0x04, 0x8d, 0x23, 0x36, 0x21, 0x36, 0x06, 0x38]);
        out.extend([0x41, 0x36, 0x42, 0x36, 0x40, 0x36, 0x44, 0x36, 0x45, 0x36, 0x33, 0x36, 0x02, 0x97]);
        out.extend([0x02, 0x00]);
        out.extend([0x02, b'2', b'7', 0x07, 0, 0, 0, 0, 0, 0, 0]);
        out.extend([0x01, b'r', 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend([0x01, 0x00, 0x00, 0x00, 0xfb, 0xff, 0xff, 0xff, 0x04, 0x00, 0x07, 0x04, 0x38, 0x01]);
        out
    }

    fn example_slice() -> FightTape {
        FightTape {
            records: vec![(-5, vec![0x07, 0x04, 0x38, 0x01])],
            blind_map: names(&[("r", 0), ("27", 7)]),
            stats: SliceCounts::default(),
        }
    }

    // Tokens.

    #[test]
    fn a_name_hashes_to_hex_of_its_own_length() {
        assert_eq!(hex_token(b"Quorvin", 0, 0), b"ed95a0a");
        assert_eq!(hex_token("雪山小狐狸".as_bytes(), 0, 0), b"6963d229225ce60");
        let long = [b'Q'; 70];
        assert_eq!(hex_token(&long, 0, 0), b"4a1a73115e39f495a7aef2911ace8ab02e6f23305dec6bacbd6c3a6b70c055a1000000");
    }

    #[test]
    fn the_roster_id_decides_the_hex_token() {
        assert_eq!(hex_token(b"Quorvin", 7, 0), b"277b329");
        assert_eq!(hex_token(b"Zelphyx", 7, 0), b"277b329");
        assert_eq!(hex_token(b"Quorvin", 8, 0), b"2950fa8");
        assert_eq!(hex_token(b"ninebytes", 0x0400_0000_0002_1234, 0), b"9731ba71e");
    }

    #[test]
    fn hex_tokens_keep_the_length_and_are_ascii() {
        for name in ["Quorvin", "雪山小狐狸", "a", "Quentaro", "Ли雪x"] {
            for id in [0, 0x03f6_0000_0001_0001] {
                let token = hex_token(name.as_bytes(), id, 0);
                assert_eq!(token.len(), name.len(), "{name}");
                assert!(token.is_ascii(), "{name}");
            }
        }
    }

    #[test]
    fn a_redraw_adds_the_attempt_to_the_name() {
        assert_eq!(hex_token(b"Quorvin", 0, 1), b"c192cac");
        assert_eq!(hex_token(b"Quorvin", 0, 2), b"173a022");
    }

    #[test]
    fn a_one_byte_name_gets_a_free_letter() {
        for (id, attempt, letter) in [(0, 0, b'r'), (42, 0, b'M'), (0, 1, b'B')] {
            let mut solo = Blinder::new(&[]);
            solo.single_used.insert(b'A');
            assert_eq!(solo.single_letter(b'A', id, attempt), letter);
        }

        let mut pair = blinder_for(&[("A", 0)]);
        assert_eq!(pair.token_of("A"), Some(&b"r"[..]));
        assert_eq!(pair.single_letter(b'B', 0, 0), b'p');
        // The same byte asks again: the same letter, though it is now used.
        assert_eq!(pair.single_letter(b'B', 0, 0), b'p');
        assert_eq!(pair.single_letter(b'A', 0, 0), b'r');
    }

    #[test]
    fn one_byte_tokens_fall_back_to_the_first_free_letter_then_q() {
        let mut crowded = Blinder::new(&[]);
        // Every letter but `z` used: no digest byte can land on a free one
        // unless it is `z`, and `z` is the only free letter either way.
        crowded.single_used.extend(LETTERS.iter().copied().filter(|&c| c != b'z'));
        assert_eq!(crowded.single_letter(b'0', 0, 0), b'z');
        assert_eq!(crowded.single_letter(b'1', 0, 0), b'q');
    }

    #[test]
    fn letter_tokens_keep_each_character_width() {
        assert_eq!(letters_of("Quorvin", 0), "GBIOooD");
        assert_eq!(letters_of("Quorvin", 1000), "VIEyeDe");
        assert_eq!(letters_of("Ли", 0), "Цщ");
        assert_eq!(letters_of("雪狐", 0), "\u{5CDF}\u{6B0C}");
        assert_eq!(letters_of("\u{2000B}a", 0), "\u{2485D}N");
        assert_eq!(letters_of(&"a".repeat(17), 0), "SGppMmiDRqMvIxkFX");
        assert_eq!(letters_of("A", 0), "L");
    }

    #[test]
    fn a_letter_token_skips_taken_ones_and_is_kept_for_the_name() {
        let mut given = HashMap::new();
        let mut taken: HashSet<String> = ["Quorvin".to_owned(), "GBIOooD".to_owned()].into();
        let first = pick_letters(&mut given, &mut taken, "Quorvin", 0);
        assert_eq!(first, letters_of("Quorvin", 1));
        assert!(taken.contains(&first));
        assert_eq!(pick_letters(&mut given, &mut taken, "Quorvin", 0), first);
        assert_eq!(pick_letters(&mut given, &mut taken, "Quorvin", 1), "VIEyeDe");
        // A redraw is not remembered: try 0 still answers with the first.
        assert_eq!(pick_letters(&mut given, &mut taken, "Quorvin", 0), first);

        // All thousand seeds of a try held: the last one is used anyway.
        let mut full: HashSet<String> = (2000..3000).map(|seed| letters_of("Ab", seed)).collect();
        let last = pick_letters(&mut HashMap::new(), &mut full, "Ab", 2);
        assert_eq!(last, letters_of("Ab", 2999));
    }

    // Building.

    #[test]
    fn longer_names_come_first_then_byte_order() {
        let map = names(&[("Quorvin", 0), ("Quorvinzu8", 0), ("Ab", 0), ("Aa", 0), ("Z", 0)]);
        let order: Vec<&str> = in_search_order(&map).iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(order, ["Quorvinzu8", "Quorvin", "Aa", "Ab", "Z"]);
    }

    #[test]
    fn new_gives_each_name_its_token() {
        let blinder = blinder_for(&[("A", 0), ("Xy", 7), ("Ef", 0x0519_0000_0000_0001)]);
        assert_eq!(blinder.token_of("A"), Some(&b"r"[..]));
        assert_eq!(blinder.token_of("Ef"), Some(&b"31"[..]));
        assert_eq!(blinder.token_of("Xy"), Some(&b"27"[..]));
        assert_eq!(blinder.token_of("B"), None);
        let searched: Vec<&[u8]> = blinder.searched.iter().map(|(name, _)| name.as_slice()).collect();
        assert_eq!(searched, [&b"Ef"[..], b"Xy"]);
        assert!(blinder.placed.contains(&b"r"[..]) && blinder.placed.contains(&b"27"[..]));
    }

    #[test]
    fn an_empty_name_gets_no_token() {
        let (empty, quorvin) = (String::new(), "Quorvin".to_owned());
        let blinder = Blinder::new(&[(&empty, &0), (&quorvin, &0)]);
        assert_eq!(blinder.token_of(""), None);
        assert_eq!(blinder.token_of("Quorvin"), Some(&b"ed95a0a"[..]));
    }

    #[test]
    fn the_report_blinder_spells_with_letters() {
        let blinder = Blinder::for_report(&names(&[("Quorvin", 5)]));
        assert_eq!(blinder.token_of("Quorvin"), Some(&b"GBIOooD"[..]));
    }

    // Blinding.

    #[test]
    fn a_spawn_frame_head_is_not_a_name() {
        let spawn = wrap_record(&padded(&[0x40, 0x36, 0xe8, 0x84, 0x01, 0x0c], 304)).unwrap();
        assert_eq!(spawn[..2], [0xb4, 0x02]);
        let after = blinded_by(&mut Blinder::new(&[]), &spawn);
        assert_eq!(after[2..7], [0x40, 0x36, 0xe8, 0x84, 0x01]);

        let host = padded(&[0x99, 0x36, 0x00, 0xb3, 0x02, 0x40, 0x36, 0xe8, 0x84, 0x01, 0x0c], 64);
        let after = blinded_by(&mut Blinder::new(&[]), &wrap_record(&host).unwrap());
        assert!(index_of_bytes(&after, 0, &[0x40, 0x36, 0xe8, 0x84, 0x01]).is_some());
    }

    #[test]
    fn a_two_byte_run_is_replaced() {
        let packet = wrap_record(&padded(&[0x45, 0x36, 0x05, 0x00, 0x02, b'K', b'9', 0x00], 32)).unwrap();
        let mut blinder = Blinder::new(&[]);
        let after = blinded_by(&mut blinder, &packet);
        assert!(index_of_bytes(&after, 0, b"K9").is_none());
        assert_eq!(after[6..8], *b"1f");
        assert_eq!(blinder.discovered.get("1f"), Some(&0));
    }

    fn damage_body(actor: u32, tail: &[u8]) -> Vec<u8> {
        let mut body = vec![0x04, 0x38];
        write_varint(&mut body, 33_000);
        body.extend([0x04, 0x00]);
        write_varint(&mut body, actor);
        body.extend_from_slice(tail);
        padded(&body, 40)
    }

    #[test]
    fn a_skill_id_after_02_stays_in_a_damage_record() {
        let packet = wrap_record(&damage_body(17_000, &[0x02, 0xd3, 0x86, 0x01, 0x00])).unwrap();
        assert_eq!(blinded_by(&mut Blinder::new(&[]), &packet), packet);
    }

    #[test]
    fn a_damage_record_is_not_searched_for_runs() {
        let body = damage_body(34_000, &[0x50, 0x77, 0xf6, 0x00]);
        assert_eq!(body[9], 0x02);
        let packet = wrap_record(&body).unwrap();
        assert_eq!(blinded_by(&mut Blinder::new(&[]), &packet), packet);
    }

    #[test]
    fn a_letter_and_a_bracket_stay() {
        let packet = wrap_record(&damage_body(17_000, &[0x02, 0x51, 0x28, 0xf4, 0x00])).unwrap();
        assert_eq!(blinded_by(&mut Blinder::new(&[]), &packet), packet);
    }

    #[test]
    fn a_one_character_name_in_a_player_spawn_is_replaced() {
        let mut body = vec![0x44, 0x36];
        write_varint(&mut body, 17_000);
        body.extend([0x03, 0xe3, 0x81, 0xbb]);
        let packet = wrap_record(&padded(&body, 40)).unwrap();
        let after = blinded_by(&mut Blinder::new(&[]), &packet);
        assert!(index_of_bytes(&after, 0, "ほ".as_bytes()).is_none());
        assert!(index_of_bytes(&after, 0, b"\x03ca6").is_some());
    }

    #[test]
    fn a_token_that_would_spell_a_known_name_is_redrawn() {
        let mut rest = vec![0x0e];
        rest.extend(b"Kp:Made^upRun3");
        rest.extend([0x44, 0xb2, 0x11, 0x22, 0x33]);
        let packet = built::player_record(70, None, &padded(&rest, 66));
        let at = index_of_bytes(&packet, 0, b"Kp:Made^upRun3").unwrap();
        assert_eq!(hex_token(b"Kp:Made^upRun3", 0, 0), b"60447a7d43cfdb");
        let after = blinded_by(&mut blinder_for(&[("bD", 0)]), &packet);
        assert_eq!(after[at..at + 14], *b"cd0ff861130eb4");
        assert!(index_of_bytes(&after, 0, b"bD").is_none());
        assert_eq!(after[..at], packet[..at]);
        assert_eq!(after[at + 14..], packet[at + 14..]);
    }

    #[test]
    fn known_names_are_replaced_in_event_packets_too() {
        let packet = wrap_record(&[&[0x04, 0x38][..], b"Quorvinzu8"].concat()).unwrap();
        let after = blinded_by(&mut blinder_for(&[("Quorvin", 1), ("Quorvinzu8", 2)]), &packet);
        assert_eq!(after[1..], *b"\x04\x38573d574ee1");
    }

    #[test]
    fn short_letter_names_need_their_length_byte() {
        let packet = [0x0b, 0x04, 0x38, 0x02, b'A', b'b', b'A', b'b'];
        let mut report = Blinder::for_report(&names(&[("Ab", 0)]));
        assert_eq!(report.token_of("Ab"), Some(&b"ce"[..]));
        assert_eq!(blinded_by(&mut report, &packet), [0x0b, 0x04, 0x38, 0x02, b'c', b'e', b'A', b'b']);
        let mut slice = blinder_for(&[("Ab", 0)]);
        assert_eq!(blinded_by(&mut slice, &packet), [0x0b, 0x04, 0x38, 0x02, b'd', b'9', b'd', b'9']);
        // Never at the very start, where no byte stands in front.
        let mut front = *b"Ab";
        assert_eq!(report.replace_known(&mut front), 0);
    }

    #[test]
    fn name_fields_skip_empty_overlapping_cut_and_binary_ones() {
        let field = |range: Range<usize>| NameField { range, record: NameRecord::Player };
        let mut buf = *b"..Xy..Kq...\xff\xfe..";
        let fields = [field(0..0), field(2..4), field(3..5), field(6..8), field(11..13), field(14..40)];
        let mut blinder = blinder_for(&[("Xy", 7)]);
        assert_eq!(blinder.fill_fields(&mut buf, &fields), 2);
        assert_eq!(buf[2..4], *b"27");
        assert_eq!(buf[4..6], *b"..");
        let minted = hex_token(b"Kq", 0, 0);
        assert_eq!(buf[6..8], minted[..]);
        assert_eq!(buf[11..13], [0xff, 0xfe]);
        let key = String::from_utf8(minted.clone()).unwrap();
        assert_eq!(blinder.discovered.get(&key), Some(&0));
        assert!(blinder.placed.contains(&minted));
    }

    #[test]
    fn a_placed_token_is_not_minted_again() {
        let mut blinder = blinder_for(&[("Xy", 7)]);
        // `27` is Xy's token: a run spelling it is left alone.
        let packet = wrap_record(&[0x45, 0x36, 0x00, 0x02, b'2', b'7', 0x00]).unwrap();
        assert_eq!(blinded_by(&mut blinder, &packet), packet);
        assert!(blinder.discovered.is_empty());
    }

    #[test]
    fn runs_need_two_to_forty_bytes_that_fit() {
        let mut blinder = Blinder::new(&[]);
        let mut long = vec![0x99, 0x36, 41];
        long.extend([b'a'; 41]);
        let long = wrap_record(&long).unwrap();
        assert_eq!(blinded_by(&mut blinder, &long), long);
        let cut = wrap_record(&[0x99, 0x36, 0x05, b'a', b'b']).unwrap();
        assert_eq!(blinded_by(&mut blinder, &cut), cut);
        let mut forty = vec![0x99, 0x36, 40];
        forty.extend([b'a'; 40]);
        let forty = wrap_record(&forty).unwrap();
        assert!(index_of_bytes(&blinded_by(&mut blinder, &forty), 0, &[b'a'; 40]).is_none());
    }

    #[test]
    fn the_unframed_blind_has_no_event_test() {
        let mut bytes = [0x07, 0x04, 0x38, 0x02, b'K', b'9', 0x00];
        let mut framed_copy = bytes;
        let mut blinder = Blinder::new(&[]);
        assert_eq!(blinder.blind(&mut framed_copy), 0);
        assert_eq!(blinder.blind_unframed(&mut bytes), 1);
        assert_eq!(bytes[4..6], *b"1f");
        let mut lead = *b"\x02K9";
        assert_eq!(Blinder::new(&[]).blind_unframed(&mut lead), 1);
        assert_eq!(lead, *b"\x021f");
    }

    #[test]
    fn the_unframed_blind_passes_over_a_record_head() {
        // A spawn's head, id 17,000: "A6" after a `02` that ends a length.
        let head = [0xb3, 0x02, 0x41, 0x36, 0xe8, 0x84, 0x01];
        let mut bytes = head;
        assert_eq!(Blinder::for_report(&NameMap::new()).blind_unframed(&mut bytes), 0);
        assert_eq!(bytes, head);
        // Without the continuation bit the `02` is a length of its own.
        let mut plain = [0x33, 0x02, 0x41, 0x36, 0xe8, 0x84, 0x01];
        assert_eq!(Blinder::for_report(&NameMap::new()).blind_unframed(&mut plain), 1);
        assert_ne!(plain[2..4], *b"A6");
        assert_eq!(plain[..2], [0x33, 0x02]);
        assert_eq!(plain[4..], [0xe8, 0x84, 0x01]);
    }

    #[test]
    fn text_is_mostly_letters_and_digits() {
        for yes in ["Kp:Made^upRun3", "Pw", "é", "ab!c", "a!!b"] {
            assert!(reads_as_text(yes.as_bytes()), "{yes}");
        }
        for no in ["Q(", "ab!", "a!!!b", "ab\x0ecd"] {
            assert!(!reads_as_text(no.as_bytes()), "{no}");
        }
        assert!(!reads_as_text(&[0xff, 0xfe]));
    }

    #[test]
    fn an_inner_record_header_is_recognised() {
        assert!(inner_header_at(&[0xb3, 0x02, 0x40, 0x36, 0xe8, 0x84, 0x01], 1));
        assert!(!inner_header_at(&[0x33, 0x02, 0x40, 0x36, 0xe8, 0x84, 0x01], 1));
        assert!(!inner_header_at(&[0xb3, 0x02, 0x47, 0x36, 0xe8, 0x84, 0x01], 1));
        assert!(!inner_header_at(&[0xb3, 0x02, 0x40, 0x36, 0xff, 0xff, 0xff, 0x7f], 1));
        assert!(!inner_header_at(&[0xb3, 0x02, 0x40, 0x36], 1));
        assert!(!inner_header_at(&[0x02, 0x40, 0x36, 0x05], 0));
    }

    #[test]
    fn a_header_shaped_run_is_left_alone_but_text_is_not() {
        // `02` then "E6": text, but it is the head of a record inside.
        let header = wrap_record(&[0x99, 0x36, 0xb3, 0x02, 0x45, 0x36, 0x05, 0x00]).unwrap();
        assert_eq!(blinded_by(&mut Blinder::new(&[]), &header), header);
        let plain = wrap_record(&[0x99, 0x36, 0x33, 0x02, 0x45, 0x36, 0x05, 0x00]).unwrap();
        assert_ne!(blinded_by(&mut Blinder::new(&[]), &plain), plain);
    }

    #[test]
    fn varints_carry_seven_bits_a_byte() {
        assert_eq!(varint_of(0), [0x00]);
        assert_eq!(varint_of(127), [0x7f]);
        assert_eq!(varint_of(128), [0x80, 0x01]);
        assert_eq!(varint_of(308), [0xb4, 0x02]);
        assert_eq!(varint_of(u32::MAX), [0xff, 0xff, 0xff, 0xff, 0x0f]);
    }

    #[test]
    fn errors_say_the_length_and_never_the_name() {
        assert!(SliceFailure::NameLeaked(5).to_string().contains('5'));
        assert!(!SliceFailure::Empty.to_string().is_empty());
    }

    // Through `build`.

    #[test]
    fn a_one_letter_player_keeps_a_letter_and_spawns_survive() {
        let player = built::player_record(9303, Some("A"), &[0; 9]);
        let spawn = built::spawn_8f_01(56_001);
        let slice = build(&one_capture(0, &[player.clone(), spawn.clone()]), 0, 10, &names(&[("A", 0)])).unwrap();
        assert_eq!(slice.records.len(), 2);
        let mut expected = player.clone();
        expected[11] = b'r';
        assert_eq!(slice.records[0].1, expected);
        differs_only_in(&slice.records[1].1, &spawn, &[]);
        assert!(slice.blind_map.contains_key("r"));
        let storage = replay(&slice.records);
        assert_eq!(storage.stored_name(9303).as_deref(), Some("r"));
        assert_eq!(storage.enemy_code(56_001), Some(2_920_548));
    }

    #[test]
    fn a_slice_changes_only_name_bytes() {
        let roster = {
            let body = built::from_hex(
                "029701000000012e0600000000000000000000000000000000000101010100000000001905\
                 0245661e0000002d0000006400000019050000000102030000000000000000",
            );
            assert_eq!(body.len(), 68);
            framed(&body)
        };
        let spawn_b = built::Spawn { id: 51_001, kind: 0x1C, name: Some("B"), code: 2_920_548, hp: 100, ..Default::default() };
        let owner = built::packet(
            [0x04, 0x8d],
            &[F::V(50_001), F::U32(0x00E9_2872), F::V(1_600), F::U16(1305), F::Str("Cd"), F::U8(0), F::U32(1)],
        );
        let sources = [
            (built::self_record(3001, "Xy", 1202, 30, 30), Some(11..13), &b"27"[..]),
            (built::player_record(9303, Some("A"), &[0; 9]), Some(11..12), &b"r"[..]),
            (spawn_b.packet(), Some(10..11), &b"p"[..]),
            (owner, Some(15..17), &b"3a"[..]),
            (roster, Some(39..41), &b"31"[..]),
            (built::spawn_8f_01(56_001), None, &b""[..]),
        ];
        let records: Vec<Vec<u8>> = sources.iter().map(|(r, _, _)| r.clone()).collect();
        let map = names(&[("Xy", 7), ("A", 0), ("Ef", 0x0519_0000_0000_0001)]);
        let slice = build(&one_capture(0, &records), 0, 10, &map).unwrap();
        assert_eq!(slice.records.len(), sources.len());
        for ((_, record), (source, range, token)) in slice.records.iter().zip(&sources) {
            let ranges: Vec<Range<usize>> = range.iter().cloned().collect();
            if let Some(range) = range {
                assert_eq!(&record[range.clone()], *token);
            }
            differs_only_in(record, source, &ranges);
        }
    }

    #[test]
    fn a_name_over_the_opcode_lets_the_run_pass_in() {
        // "8x" sits on the second opcode byte of a hit; once it is a token
        // the packet is no event, so "Pw" behind it is blinded too.
        let payload = [0x04, 0x38, 0x78, 0x02, 0x50, 0x77, 0x00];
        let slice = built_from(&payload, 0, 10, &[("8x", 0)]);
        let name_token = hex_token(b"8x", 0, 0);
        let run_token = hex_token(b"Pw", 0, 0);
        assert_eq!(name_token, b"1e");
        let want = [&[0x0b, 0x04][..], &name_token, &[0x02], &run_token, &[0x00]].concat();
        assert_eq!(slice.records, [(0, want)]);
        let tokens = [&name_token, &run_token].map(|t| (String::from_utf8(t.clone()).unwrap(), 0));
        assert_eq!(slice.blind_map, HashMap::from(tokens));
        assert_eq!(slice.stats.names_blinded, 2);
    }

    #[test]
    fn a_known_name_is_replaced_in_place() {
        let payload = [&[0x45, 0x36, 0x05][..], b"Quorvin"].concat();
        let [(_, record)] = &built_from(&payload, 0, 10, &[("Quorvin", 42)]).records[..] else {
            panic!("one record expected");
        };
        assert_eq!(record.len(), framed(&payload).len());
        assert!(index_of_bytes(record, 0, b"Quorvin").is_none());
    }

    #[test]
    fn a_longer_name_goes_before_the_name_inside_it() {
        let payload = [&[0x45, 0x36][..], b"Quorvinzu8"].concat();
        let slice = built_from(&payload, 0, 10, &[("Quorvin", 1), ("Quorvinzu8", 2)]);
        for leftover in [&b"Quorvin"[..], b"zu8"] {
            assert!(index_of_bytes(&slice.records[0].1, 0, leftover).is_none());
        }
    }

    #[test]
    fn a_cjk_name_becomes_ascii_of_the_same_length() {
        let name = "雪山小狐狸";
        let payload = [&[0x02, 0x97][..], name.as_bytes()].concat();
        let slice = built_from(&payload, 0, 10, &[(name, 3)]);
        let (_, record) = &slice.records[0];
        assert_eq!(framed(&payload).len(), record.len());
        assert!(record[3..].is_ascii() && index_of_bytes(record, 0, name.as_bytes()).is_none());
    }

    #[test]
    fn encode_then_decode_gives_the_slice_back() {
        let id = 0x0400_0000_0002_1234;
        let payload = [&[0x02, 0x97, 0x01, 0x02, 0x03][..], b"Quentaro"].concat();
        let slice = built_from(&payload, 500, 1_000, &[("Quentaro", id)]);
        let back = decode(&encode(&slice));
        assert_eq!(back, Some((slice.records, slice.blind_map.clone())));
        assert!(slice.blind_map.values().any(|&v| v == id));
    }

    #[test]
    fn the_same_fight_encodes_to_the_same_bytes() {
        let payload = [&[0x04, 0x38][..], b"Quorvin"].concat();
        let file = || encode(&built_from(&payload, 0, 10, &[("Quorvin", 1), ("Quentaro", 2)]));
        assert_eq!(file(), file());
    }

    // The container.

    #[test]
    fn the_example_slice_encodes_to_its_73_bytes() {
        let bytes = encode(&example_slice());
        assert_eq!(bytes.len(), 73);
        assert_eq!(bytes, example_file());
    }

    #[test]
    fn counts_and_lengths_keep_their_low_bits() {
        let token = "t".repeat(300);
        let slice = FightTape {
            records: Vec::new(),
            blind_map: names(&[(&token, 0)]),
            stats: SliceCounts::default(),
        };
        let bytes = encode(&slice);
        assert_eq!(bytes[BLIND_COUNT_AT..BLIND_COUNT_AT + 3], [0x01, 0x00, 300u16 as u8]);
    }

    #[test]
    fn records_go_out_in_version_1_lengths() {
        let body = padded(&[0x41, 0x36], 200);
        let current = wrap_record(&body).unwrap();
        assert_eq!(current[..2], [0xcc, 0x01]);
        let short = vec![0x07, 0x41, 0x36, 0x05];
        let slice = FightTape {
            records: vec![(0, current), (1, short.clone())],
            blind_map: HashMap::new(),
            stats: SliceCounts::default(),
        };
        let bytes = encode(&slice);
        let first = BLIND_COUNT_AT + 2 + 4;
        assert_eq!(bytes[first..first + 4], [0, 0, 0, 0]);
        assert_eq!(bytes[first + 4..first + 6], [0xca, 0x00]);
        assert_eq!(bytes[first + 6..first + 8], [0xcd, 0x01]);
        assert_eq!(bytes[first + 8..first + 208], body[..]);
        let second = first + 208;
        assert_eq!(bytes[second + 4..second + 6], [0x04, 0x00]);
        assert_eq!(bytes[second + 6..], short[..]);
    }

    #[test]
    fn decode_reads_the_example_and_refuses_broken_files() {
        let file = example_file();
        let (records, map) = decode(&file).unwrap();
        assert_eq!(records, [(-5, vec![0x07, 0x04, 0x38, 0x01])]);
        assert_eq!(map, names(&[("27", 7), ("r", 0)]));

        let mut longer = file.clone();
        longer.push(0xaa);
        assert_eq!(decode(&longer), Some((records.clone(), map.clone())));
        for cut in 0..file.len() {
            assert_eq!(decode(&file[..cut]), None, "cut at {cut}");
        }
        let mut magic = file.clone();
        magic[3] = b'X';
        assert_eq!(decode(&magic), None);
        let mut version = file.clone();
        version[4] = 0x03;
        assert_eq!(decode(&version), None);
        let mut token = file.clone();
        let r = file.iter().position(|&b| b == b'r').unwrap();
        token[r] = 0xff;
        assert_eq!(decode(&token), None);
        let mut v2 = file.clone();
        v2[4] = 0x02;
        assert_eq!(decode(&v2), Some((records, map)));
    }

    #[test]
    fn a_repeated_token_keeps_the_later_id() {
        // The first entry, `27` with id 7, becomes `r` with id 7, ahead of
        // the `r` with id 0.
        let mut file = example_file();
        let at = BLIND_COUNT_AT + 2;
        file.splice(at..at + 3, [0x01, b'r']);
        let (_, map) = decode(&file).unwrap();
        assert_eq!(map, names(&[("r", 0)]));
    }

    #[test]
    fn decode_refuses_other_bytes() {
        assert_eq!(decode(b"not a slice at all"), None);
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[0x08, 0x04, 0x38, 0x01]), None);
    }

    fn long_body() -> Vec<u8> {
        let mut body = vec![0x04, 0x38];
        body.extend((0..200u32).map(|i| i as u8 | 1));
        body
    }

    #[test]
    fn encode_writes_version_1_and_decode_restores_it() {
        let long = wrap_record(&long_body()).unwrap();
        let short = wrap_record(&[0x41, 0x36, 0x05]).unwrap();
        let slice = FightTape {
            records: vec![
                (0, long.clone()),
                (1, pack_bundle(&[short.clone(), long].concat()).unwrap()),
                (2, short),
            ],
            blind_map: HashMap::new(),
            stats: SliceCounts::default(),
        };
        let bytes = encode(&slice);
        assert_eq!(bytes[4..6], [0x01, 0x00]);
        let (records, map) = decode(&bytes).unwrap();
        assert_eq!(records, slice.records);
        assert!(map.is_empty());
    }

    /// A frame by the version 1 rule.
    fn v1_frame(body: &[u8], top_bundle: bool) -> Vec<u8> {
        let mut out = (1..=3usize)
            .map(|n| (n, varint_of((n + body.len() + 3 - usize::from(top_bundle)) as u32)))
            .find(|(n, v)| v.len() == *n)
            .unwrap()
            .1;
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn a_version_1_file_reads_as_current_frames() {
        let long = long_body();
        let short = [0x41, 0x36, 0x05];
        let inner = [v1_frame(&short, false), v1_frame(&long, false)].concat();
        let mut bundle = vec![0xFF, 0xFF];
        bundle.extend((inner.len() as u32).to_le_bytes());
        bundle.extend(lz4_flex::compress(&inner));
        let v1 = [v1_frame(&long, false), v1_frame(&bundle, true), v1_frame(&short, false)];

        // A version 1 file as an old build wrote it.
        let mut file = MAGIC.to_vec();
        file.extend(WIRE_VERSION.to_le_bytes());
        file.extend(0u16.to_le_bytes());
        file.extend(0u16.to_le_bytes());
        file.extend((v1.len() as u32).to_le_bytes());
        for (ms, record) in v1.iter().enumerate() {
            file.extend((ms as i32).to_le_bytes());
            file.extend((record.len() as u16).to_le_bytes());
            file.extend(record);
        }
        let (records, _) = decode(&file).unwrap();
        assert_eq!(records.len(), 3);
        // Each record is one frame by the current walk, and nothing is left.
        let payloads: Vec<&[u8]> = records
            .iter()
            .map(|(_, record)| match framing::walk(record) {
                framing::Framing { consumed, frames } if consumed == record.len() && frames.len() == 1 => {
                    assert_eq!(frames[0].end, record.len());
                    frames[0].payload(record)
                }
                other => panic!("not one whole frame: {other:?}"),
            })
            .collect();
        assert_eq!((payloads[0], payloads[2]), (&long[..], &short[..]));
        let opened = framing::inflate_block(payloads[1]).unwrap();
        let inside: Vec<&[u8]> = framing::split_frames(&opened).frames.iter().map(|f| f.payload(&opened)).collect();
        assert_eq!(inside, [&short[..], &long[..]]);
    }
}
