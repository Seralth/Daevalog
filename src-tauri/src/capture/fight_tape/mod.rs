//! Evidence slices: out of the stream bytes captured around one fight, the
//! packets a replay of that fight needs and nothing more, names blinded.
//! What stays is a privacy promise (`docs/PRIVACY.md`): a packet kept by
//! mistake can carry chat or a bystander's name, one lost by mistake changes
//! the fight a replay derives. The blinder and the `.a2es` file live in
//! `format.rs`.

use std::collections::HashMap;
use std::ops::Range;

use crate::capture::framing::{self, Frame, ChunkForm, NEST_LIMIT};
use crate::capture::opcodes::{
    DAMAGE, DEATH, DOT, HP_MP, MAP_LOAD, PARTY_ROSTER, PARTY_SCOPE, PLAYER_SPAWN, PLAYER_SPAWN_OLD, SELF_IDENTITY,
    SPAWN, SPAWN_OLD, SUMMON_OWNERSHIP, ZONE_CHANGE,
};
use crate::capture::byte_queue::ByteQueue;
use crate::capture::decoder::{compact_record_in, embedded_spawns, name_fields};

use super::varint;

mod format;

pub(crate) use format::Blinder;
pub use format::{decode, encode, FightTape, NameMap, SliceFailure, MAGIC, VERSION};

/// Events are kept from this long before the fight starts.
pub const SLICE_HEAD_MS: i64 = 60_000;
/// And until this long after it ends, for the death and the last HP records.
pub const TAIL_MS: i64 = 15_000;
/// Before the lead-in, state (no events) is kept for this long more. A boss
/// spawns and the roster arrives when the room is entered, often minutes
/// ahead of the pull; a slice without them files the fight under no boss.
pub const SLICE_LOOKBACK_MS: i64 = 1_800_000;

/// The opcodes a slice may carry, in the order every `.a2es` lists them.
/// The first three are events; the rest is state.
pub const KEPT_OPCODES: &[(&[u8; 2], &str)] = &[
    (&DAMAGE, "damage"),
    (&DOT, "damage or heal over time"),
    (&HP_MP, "hp/mp"),
    (&SUMMON_OWNERSHIP, "summon or loot owner"),
    (&ZONE_CHANGE, "zone change"),
    (&MAP_LOAD, "map load"),
    (&PARTY_SCOPE, "party scope"),
    (&SPAWN, "spawn"),
    (&DEATH, "death"),
    (&SPAWN_OLD, "spawn (before June 2026)"),
    (&PLAYER_SPAWN_OLD, "player spawn (before June 2026)"),
    (&PLAYER_SPAWN, "player spawn"),
    (&SELF_IDENTITY, "self record"),
    (&PARTY_ROSTER, "party roster"),
];

/// The allowlisted opcodes that are events: kept only from the lead-in on.
const EVENT_PAIRS: &[[u8; 2]] = &[DAMAGE, DOT, HP_MP];

/// Whether a framed packet is an event: damage, over-time and HP records hold
/// ids and numbers only, so no blinder pass and no leak check looks in them.
pub(crate) fn carries_event(packet: &[u8]) -> bool {
    let head = varint::varint_at(packet, 0);
    if head.length <= 0 {
        return false;
    }
    let at = head.length as usize;
    packet.get(at..at + 2).is_some_and(|op| EVENT_PAIRS.iter().any(|e| e[..] == *op))
}

/// Bytes one stream delivered at one moment. A frame may be cut across
/// several of these.
#[derive(Debug, Clone)]
pub struct SliceRecord {
    pub captured_at_ms: i64,
    /// Segments with the same stream are joined before they are framed.
    pub stream: String,
    pub bytes: Vec<u8>,
}

/// Counts over every segment handed to `build`, in the window or not.
#[derive(Debug, Default, Clone)]
pub struct SliceCounts {
    /// Packet frames met, at the top level and inside opened bundles.
    pub packets_seen: usize,
    /// Allowlisted packets kept.
    pub packets_kept: usize,
    pub bytes_seen: usize,
    /// Kept packets and lifted pieces; a re-wrapped bundle adds nothing.
    pub bytes_kept: usize,
    pub names_blinded: usize,
    /// Bundles that opened, at any depth.
    pub bundles_expanded: usize,
}

/// Carrier opcode for bytes lifted out of a dropped packet. No reader takes
/// it, so a replay meets a lifted record only through its record scans, as
/// it met it inside the original packet. Fixed: uploaded slices hold it.
pub const LIFT_OPCODE: [u8; 2] = [0xE5, 0xA2];
/// From a hit inside a dropped packet, this many bytes are copied.
const BYTES_AFTER_HIT: usize = 160;
/// How far from its opcode a lifted spawn's own length may end it.
const SPAWN_LEN_FLOOR: usize = 16;
const SPAWN_LEN_CEILING: usize = 4_096;
/// Copied from a spawn whose own length cannot be believed.
const SPAWN_LEN_UNKNOWN: usize = 512;

/// Whether a segment falls before the lead-in or in the fight proper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prelude,
    Fight,
}

impl Phase {
    fn keeps(self, opcode: [u8; 2]) -> bool {
        let listed = KEPT_OPCODES.iter().any(|(op, _)| **op == opcode);
        listed && (self == Phase::Fight || !EVENT_PAIRS.contains(&opcode))
    }
}

fn with_length(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 3);
    format::write_varint(&mut out, framing::length_value(body.len()));
    out.extend_from_slice(body);
    out
}

/// `body` behind its length varint, as the game frames a packet. Never `None`.
pub(crate) fn wrap_record(body: &[u8]) -> Option<Vec<u8>> {
    Some(with_length(body))
}

/// `inner` (frames back to back) packed as one bundle frame: `FF FF`, the
/// inner size as u32 LE, then one raw LZ4 block. `None` only when the size
/// does not fit a u32, which no bundle comes near.
pub fn pack_bundle(inner: &[u8]) -> Option<Vec<u8>> {
    let size = u32::try_from(inner.len()).ok()?;
    let body: Vec<u8> = [0xFF, 0xFF].into_iter().chain(size.to_le_bytes()).chain(lz4_flex::compress(inner)).collect();
    wrap_record(&body)
}

/// A piece: lifted bytes under the carrier opcode, framed.
fn lifted(bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 2);
    body.extend_from_slice(&LIFT_OPCODE);
    body.extend_from_slice(bytes);
    with_length(&body)
}

fn opcode_of(frame: &Frame, buf: &[u8]) -> Option<[u8; 2]> {
    match frame.payload(buf) {
        [a, b, ..] => Some([*a, *b]),
        _ => None,
    }
}

fn hit_at(raw: &[u8], i: usize) -> bool {
    raw.get(i..i + 2) == Some(&DAMAGE[..])
}

/// Where a spawn at `raw[i]` ends, by the length varint in front of it. The
/// widest reading is tried first: a two-byte length's last byte also reads
/// as a one-byte length.
fn spawn_end(raw: &[u8], i: usize) -> usize {
    for width in [3usize, 2, 1] {
        let Some(from) = i.checked_sub(width) else { continue };
        let len = varint::varint_at(raw, from);
        if len.length != width as i32 {
            continue;
        }
        let Some(size) = framing::frame_size(len.value, len.length) else { continue };
        let end = from + size;
        if end >= i + SPAWN_LEN_FLOOR && end <= raw.len() && end <= i + SPAWN_LEN_CEILING {
            return end;
        }
    }
    raw.len().min(i + SPAWN_LEN_UNKNOWN)
}

/// The stretches of a dropped packet's frame worth lifting: each hit (in a
/// fight) and each spawn, merged where they overlap so a record sitting in
/// an earlier one's copy is not lifted twice. The spawns are those the
/// live meter reads in the frame, found by its own scan.
fn lift_spans(raw: &[u8], phase: Phase) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    if phase == Phase::Fight {
        for i in (2..raw.len().saturating_sub(1)).filter(|&i| hit_at(raw, i)) {
            spans.push(i..raw.len().min(i + BYTES_AFTER_HIT));
        }
    }
    for (i, _, _) in embedded_spawns(raw) {
        let end = spawn_end(raw, i);
        let cut = (i + 2..end).find(|&j| hit_at(raw, j)).unwrap_or(end);
        spans.push(i..cut);
    }
    spans.sort_by_key(|s| s.start);
    let mut runs: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        match runs.last_mut() {
            Some(run) if span.start < run.end => run.end = run.end.max(span.end),
            _ => runs.push(span),
        }
    }
    runs
}

/// A cast's skill context out of a dropped packet's payload, found by the
/// walk `decoder/ticks.rs` `read_held_tick_skill`
/// reads it with (`compact_record_in`): the cast mark, the first compact
/// opcode `38` after it, the caster, one uid byte, then a skill code of 3 or
/// 4 bytes. The piece runs from the mark to the end of the skill code.
fn cast_context(payload: &[u8]) -> Option<Vec<u8>> {
    let record = compact_record_in(payload)?;
    Some(lifted(&payload[record.marker..payload.len().min(record.skill_at + 4)]))
}

/// The state of one `build`: the blinder and the counts.
struct Slicer {
    blinder: Blinder,
    stats: SliceCounts,
}

impl Slicer {
    /// Blinds bytes about to go out and counts them as kept.
    fn keep(&mut self, mut bytes: Vec<u8>) -> Vec<u8> {
        self.stats.names_blinded += self.blinder.blind(&mut bytes);
        self.stats.bytes_kept += bytes.len();
        bytes
    }

    /// Frames a stream's held bytes. Gives the outputs in frame order (each
    /// kept packet, piece or re-wrapped bundle on its own) and how many
    /// leading bytes are done with.
    fn read_stream(&mut self, held: &[u8], phase: Phase) -> (Vec<Vec<u8>>, usize) {
        let walked = framing::walk(held);
        let mut outputs = Vec::new();
        for frame in &walked.frames {
            if frame.kind == ChunkForm::Packet {
                outputs.extend(self.packet(held, frame, phase, false));
                continue;
            }
            let mut kept = Vec::new();
            self.open_bundle(frame.payload(held), 1, phase, &mut kept);
            if !kept.is_empty() {
                outputs.extend(pack_bundle(&kept));
            }
        }
        (outputs, walked.consumed)
    }

    /// One packet frame: itself if kept, else whatever lifts out of it.
    fn packet(&mut self, buf: &[u8], frame: &Frame, phase: Phase, in_bundle: bool) -> Vec<Vec<u8>> {
        let raw = frame.bytes(buf);
        self.stats.packets_seen += 1;
        self.stats.bytes_seen += raw.len();
        if opcode_of(frame, buf).is_some_and(|op| phase.keeps(op)) {
            self.stats.packets_kept += 1;
            return vec![self.keep(raw.to_vec())];
        }
        let mut pieces = Vec::new();
        // The parser reads a cast's context only for the hits of its own bundle.
        if in_bundle && phase == Phase::Fight {
            pieces.extend(cast_context(frame.payload(buf)));
        }
        pieces.extend(lift_spans(raw, phase).into_iter().map(|span| lifted(&raw[span])));
        pieces.into_iter().map(|piece| self.keep(piece)).collect()
    }

    /// Opens the bundle `payload` (`depth` 1 for one in the stream) and
    /// appends what it keeps to `kept`; a bundle inside goes inline.
    fn open_bundle(&mut self, payload: &[u8], depth: usize, phase: Phase, kept: &mut Vec<u8>) {
        let Some(content) = framing::inflate_block(payload) else { return };
        self.stats.bundles_expanded += 1;
        if depth > NEST_LIMIT {
            return;
        }
        for frame in framing::split_frames(&content).frames {
            match frame.kind {
                ChunkForm::Packet => {
                    for bytes in self.packet(&content, &frame, phase, true) {
                        kept.extend_from_slice(&bytes);
                    }
                }
                ChunkForm::Bundle => self.open_bundle(frame.payload(&content), depth + 1, phase, kept),
            }
        }
    }
}

/// Each packet frame in `buf`, bundles opened as a replay opens them.
/// `depth` is that of the bundle `buf` came out of, 0 for none.
fn visit_packets(buf: &[u8], depth: usize, visit: &mut dyn FnMut(&[u8])) {
    let walked = if depth == 0 { framing::walk(buf) } else { framing::split_frames(buf) };
    for frame in walked.frames {
        match frame.kind {
            ChunkForm::Packet => visit(frame.bytes(buf)),
            ChunkForm::Bundle if depth < NEST_LIMIT => {
                if let Some(content) = framing::inflate_block(frame.payload(buf)) {
                    visit_packets(&content, depth + 1, visit);
                }
            }
            ChunkForm::Bundle => {}
        }
    }
}

/// A record's packets back to back, bundles opened: what a replay reads.
pub fn expand(record: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(record.len());
    visit_packets(record, 0, &mut |packet| out.extend_from_slice(packet));
    out
}

/// The last look before a slice goes out. First any known name of two bytes
/// or more anywhere in what a replay reads (one record at a time); then any
/// name field, outside the events, that spells a known name of any length.
/// The error carries the byte length of what was found.
fn find_leak(records: &[(i32, Vec<u8>)], ordered: &[(&String, &u64)]) -> Result<(), SliceFailure> {
    let readable: Vec<Vec<u8>> = records.iter().map(|(_, r)| expand(r)).collect();
    let in_some_record = |needle: &[u8]| readable.iter().any(|bytes| varint::index_of_bytes(bytes, 0, needle).is_some());
    let mut searched = ordered.iter().map(|(name, _)| name.as_bytes()).filter(|needle| needle.len() >= format::SHORTEST_SEARCHED);
    if let Some(found) = searched.find(|needle| in_some_record(needle)) {
        return Err(SliceFailure::NameLeaked(found.len()));
    }

    let is_known = |text: &[u8]| ordered.iter().any(|(name, _)| name.as_bytes() == text);
    let mut field_len = None;
    for (_, record) in records {
        visit_packets(record, 0, &mut |packet| {
            if field_len.is_none() && !carries_event(packet) {
                let fields = name_fields(packet);
                field_len = fields.iter().filter_map(|f| packet.get(f.range.clone())).find(|t| is_known(t)).map(<[u8]>::len);
            }
        });
        if let Some(len) = field_len {
            return Err(SliceFailure::NameLeaked(len));
        }
    }
    Ok(())
}

/// Cuts the slice of the fight from `fight_start_ms` to `fight_end_ms` out
/// of `packets`, which are read in the order given. Every segment is framed,
/// filtered and blinded, so the blinder and the counts see the same bytes
/// whatever the window; only output from inside the window is kept. A
/// record's time counts from the fight start.
pub fn build(
    packets: &[SliceRecord],
    fight_start_ms: i64,
    fight_end_ms: i64,
    names: &NameMap,
) -> Result<FightTape, SliceFailure> {
    let ordered = format::in_search_order(names);
    let lead_in_from = fight_start_ms.saturating_sub(SLICE_HEAD_MS);
    let kept_from = lead_in_from.saturating_sub(SLICE_LOOKBACK_MS);
    let kept_until = fight_end_ms + TAIL_MS;
    let mut slicer = Slicer { blinder: Blinder::new(&ordered), stats: SliceCounts::default() };
    let mut held: HashMap<&String, ByteQueue> = HashMap::new();
    let mut records: Vec<(i32, Vec<u8>)> = Vec::new();

    for segment in packets {
        let ms = segment.captured_at_ms;
        let phase = if ms >= lead_in_from { Phase::Fight } else { Phase::Prelude };
        let pending = held.entry(&segment.stream).or_default();
        pending.append(&segment.bytes);
        let (outputs, done) = slicer.read_stream(pending.snapshot(), phase);
        pending.drop_front(done);
        if ms < kept_from || ms > kept_until {
            continue;
        }
        let offset = ms.saturating_sub(fight_start_ms).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        for bytes in outputs {
            records.push((offset, bytes));
        }
    }

    if records.is_empty() {
        return Err(SliceFailure::Empty);
    }
    find_leak(&records, &ordered)?;

    // Names first; a token minted for an unknown name never displaces one.
    let mut blind_map: HashMap<String, u64> = ordered
        .iter()
        .filter_map(|&(name, id)| Some((String::from_utf8_lossy(slicer.blinder.token_of(name)?).into_owned(), *id)))
        .collect();
    for (token, id) in &slicer.blinder.discovered {
        if !blind_map.contains_key(token) {
            blind_map.insert(token.clone(), *id);
        }
    }
    Ok(FightTape { records, blind_map, stats: slicer.stats })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::built::{self, F};
    use crate::capture::decoder::PacketDecoder;
    use crate::combat::data_storage::DataStorage;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};

    /// A packet frame with a one-byte length (payloads up to 123 bytes).
    pub(super) fn framed(payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() <= 123, "one length byte holds at most 123 payload bytes");
        let mut out = vec![payload.len() as u8 + 4];
        out.extend_from_slice(payload);
        out
    }

    /// One framed packet on "Client:1" at `ms`.
    pub(super) fn cap(ms: i64, payload: &[u8]) -> SliceRecord {
        SliceRecord { captured_at_ms: ms, stream: "Client:1".into(), bytes: framed(payload) }
    }

    fn nested(packet: &[u8], depth: usize) -> Vec<u8> {
        (0..depth).fold(packet.to_vec(), |inner, _| built::bundle(&inner))
    }

    fn seg(ms: i64, stream: &str, bytes: Vec<u8>) -> SliceRecord {
        SliceRecord { captured_at_ms: ms, stream: stream.into(), bytes }
    }

    const HIT: [u8; 5] = [0x04, 0x38, 0x01, 0x02, 0x03];
    /// A hit with one byte after its opcode.
    const SHORT_HIT: [u8; 3] = [HIT[0], HIT[1], HIT[2]];
    const MOB: [u8; 5] = [0x41, 0x36, 0x01, 0x02, 0x03];
    const CHAT: [u8; 3] = [0xAB, 0xCD, 0x01];

    /// A dropped packet `99 36 ...` of `body_len` payload bytes, with `04 38`
    /// written at the given payload offsets.
    fn host(body_len: usize, hits: &[usize]) -> Vec<u8> {
        let mut body = vec![0x11; body_len];
        body[..2].copy_from_slice(&[0x99, 0x36]);
        for &at in hits {
            body[at..at + 2].copy_from_slice(&DAMAGE);
        }
        built::frame(&body)
    }

    /// A dropped packet with a spawn of id 1000 at frame index 12, behind a
    /// one-byte length `len_byte`.
    fn spawn_host(len_byte: u8, id: &[u8]) -> Vec<u8> {
        let mut body = vec![0x99, 0x36];
        body.extend([0x11; 8]);
        body.extend([len_byte, 0x45, 0x36]);
        body.extend_from_slice(id);
        body.extend([0x22; 40]);
        built::frame(&body)
    }

    /// A dropped packet holding a 1,037-byte spawn of id 40,001 at frame
    /// index 8, behind a two-byte length `8D 08`.
    fn big_spawn_host() -> Vec<u8> {
        let mut body = built::from_hex("9936 1111 8D08 4136 C1B802");
        body.resize(1_039, 0x22);
        body.extend([0x11; 40]);
        let out = built::frame(&body);
        assert_eq!(out.len(), 1_081);
        assert_eq!(out[..2], [0xBB, 0x08]);
        out
    }

    /// Fight far after `t = 0`, so a segment at 0 is read in prelude mode.
    const LATE_START: i64 = 1_000_000;
    /// The time of a record from that segment.
    const PRELUDE_DT: i32 = -1_000_000;

    fn prelude_records(bytes: Vec<u8>) -> Result<FightTape, SliceFailure> {
        build(&[seg(0, "Client:1", bytes)], LATE_START, LATE_START + 10, &NameMap::new())
    }

    fn fight_records(bytes: Vec<u8>) -> Result<FightTape, SliceFailure> {
        build(&[seg(LATE_START, "Client:1", bytes)], LATE_START, LATE_START + 10, &NameMap::new())
    }

    #[test]
    fn the_allowlist_holds_fourteen_opcodes_in_file_order() {
        let listed: Vec<u8> = KEPT_OPCODES.iter().flat_map(|(op, _)| op.iter().copied()).collect();
        assert_eq!(listed, built::from_hex("0438 0538 1B92 048D 2336 2136 0638 4136 4236 4036 4436 4536 3336 0297"));
        assert!(KEPT_OPCODES.iter().all(|(_, label)| !label.is_empty()));
        let events: Vec<u8> = EVENT_PAIRS.concat();
        assert_eq!(events, listed[..6]);
    }

    #[test]
    fn window_edges_follow_lead_in_tail_and_prelude() {
        let (start, end) = (2_000_000, 2_100_000);
        let mut bytes = built::frame(&SHORT_HIT);
        bytes.extend(built::frame(&MOB));
        let times = [139_999, 140_000, 1_939_999, 1_940_000, 2_115_000, 2_115_001];
        let segments: Vec<_> = times.iter().map(|&t| seg(t, "Client:1", bytes.clone())).collect();
        let slice = build(&segments, start, end, &NameMap::new()).unwrap();
        let hit = built::frame(&SHORT_HIT);
        let mob = built::frame(&MOB);
        assert_eq!(
            slice.records,
            vec![
                (-1_860_000, mob.clone()),
                (-60_001, mob.clone()),
                (-60_000, hit.clone()),
                (-60_000, mob.clone()),
                (115_000, hit),
                (115_000, mob),
            ]
        );
    }

    #[test]
    fn a_segment_outside_the_window_is_still_read_and_blinded() {
        let segments = [
            seg(0, "Client:1", built::frame(&[&[0x23, 0x36, 0x04][..], b"Wxyz"].concat())),
            seg(10_000_000, "Client:1", built::frame(&SHORT_HIT)),
        ];
        let slice = build(&segments, 10_000_000, 10_001_000, &NameMap::new()).unwrap();
        let tokens: Vec<(usize, u64)> = slice.blind_map.iter().map(|(t, id)| (t.len(), *id)).collect();
        assert_eq!((slice.records.len(), slice.stats.packets_seen, tokens), (1, 2, vec![(4, 0)]));
    }

    #[test]
    fn a_frame_cut_across_segments_takes_the_completing_segment_time() {
        let alone = [seg(939_999, "Client:1", vec![0x09, 0x04, 0x38]), seg(940_000, "Client:1", vec![0x01, 0x02, 0x03])];
        let slice = build(&alone, 1_000_000, 1_010_000, &NameMap::new()).unwrap();
        assert_eq!(slice.records, vec![(-60_000, built::frame(&HIT))]);

        let between = [
            seg(939_999, "Client:1", vec![0x09, 0x04, 0x38]),
            seg(939_999, "Client:2", built::frame(&MOB)),
            seg(940_000, "Client:1", vec![0x01, 0x02, 0x03]),
        ];
        let slice = build(&between, 1_000_000, 1_010_000, &NameMap::new()).unwrap();
        assert_eq!(slice.records, vec![(-60_001, built::frame(&MOB)), (-60_000, built::frame(&HIT))]);
    }

    #[test]
    fn a_frame_too_short_for_an_opcode_is_counted_and_dropped() {
        let mut bytes = vec![0x05, 0x04];
        bytes.extend(built::frame(&SHORT_HIT));
        let slice = fight_records(bytes).unwrap();
        assert_eq!(slice.records, vec![(0, built::frame(&SHORT_HIT))]);
        assert_eq!((slice.stats.packets_seen, slice.stats.packets_kept), (2, 1));
    }

    #[test]
    fn a_kept_packet_is_its_captured_frame() {
        let mut body = MOB.to_vec();
        body.resize(150, 0x00);
        let frame = built::frame(&body);
        assert_eq!(frame.len(), 152);
        let slice = prelude_records(frame.clone()).unwrap();
        assert_eq!(slice.records, vec![(PRELUDE_DT, frame)]);
    }

    #[test]
    fn a_bundle_keeps_only_its_listed_packets() {
        let mut inner = built::frame(&MOB);
        inner.extend(built::frame(&CHAT));
        let slice = fight_records(built::bundle(&inner)).unwrap();
        assert_eq!(slice.records, vec![(0, built::bundle(&built::frame(&MOB)))]);
    }

    #[test]
    fn a_bundle_inside_a_bundle_goes_inline() {
        let mut inner = built::frame(&MOB);
        inner.extend(built::bundle(&built::frame(&SHORT_HIT)));
        let slice = fight_records(built::bundle(&inner)).unwrap();
        let mut want = built::frame(&MOB);
        want.extend(built::frame(&SHORT_HIT));
        assert_eq!(slice.records, vec![(0, built::bundle(&want))]);
        assert_eq!(slice.stats.bundles_expanded, 2);
    }

    #[test]
    fn bundles_open_four_deep_and_no_deeper() {
        let hit = built::frame(&SHORT_HIT);
        let slice = fight_records(nested(&hit, 4)).unwrap();
        assert_eq!(slice.records, vec![(0, built::bundle(&hit))]);
        assert_eq!((slice.stats.bundles_expanded, slice.stats.packets_seen), (4, 1));

        let mut deeper = nested(&hit, 5);
        deeper.extend(built::frame(&MOB));
        let slice = fight_records(deeper).unwrap();
        assert_eq!(slice.records, vec![(0, built::frame(&MOB))]);
        assert_eq!((slice.stats.bundles_expanded, slice.stats.packets_seen), (5, 1));
        assert!(matches!(fight_records(nested(&hit, 5)), Err(SliceFailure::Empty)));
    }

    #[test]
    fn a_bundle_that_does_not_open_is_dropped_uncounted() {
        let mut bytes = built::frame(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x01]);
        bytes.extend(built::frame(&MOB));
        let slice = fight_records(bytes).unwrap();
        assert_eq!(slice.records, vec![(0, built::frame(&MOB))]);
        assert_eq!((slice.stats.bundles_expanded, slice.stats.packets_seen), (0, 1));
    }

    #[test]
    fn a_bundle_keeping_nothing_gives_no_record() {
        let mut bytes = built::bundle(&built::frame(&CHAT));
        bytes.extend(built::frame(&MOB));
        let slice = fight_records(bytes).unwrap();
        assert_eq!(slice.records, vec![(0, built::frame(&MOB))]);
        assert_eq!(slice.stats.bundles_expanded, 1);
    }

    #[test]
    fn hits_close_together_lift_as_a_single_piece() {
        let frame = host(400, &[20, 60]);
        assert_eq!((frame.len(), &frame[..2]), (402, &[0x94, 0x03][..]));
        let slice = fight_records(frame.clone()).unwrap();
        assert_eq!(slice.records.len(), 1);
        let piece = &slice.records[0].1;
        assert_eq!(piece.len(), 204);
        assert_eq!(*piece, lifted(&frame[22..222]));
        assert_eq!(piece.windows(2).filter(|w| *w == DAMAGE).count(), 2);
        assert!(matches!(prelude_records(frame), Err(SliceFailure::Empty)));
    }

    #[test]
    fn hits_far_apart_lift_as_separate_bounded_pieces() {
        let frame = host(600, &[20, 300]);
        assert_eq!(frame.len(), 602);
        let slice = fight_records(frame.clone()).unwrap();
        assert_eq!(slice.records, vec![(0, lifted(&frame[22..182])), (0, lifted(&frame[302..462]))]);
        for (_, piece) in &slice.records {
            assert_eq!(piece.len(), 164);
            assert_eq!(piece.windows(2).filter(|w| *w == DAMAGE).count(), 1);
        }
        assert!(matches!(prelude_records(frame), Err(SliceFailure::Empty)));
    }

    #[test]
    fn a_lifted_piece_walks_as_one_packet_nobody_reads() {
        let frame = host(100, &[20]);
        assert_eq!(frame.len(), 101);
        let slice = fight_records(frame.clone()).unwrap();
        let piece = &slice.records[0].1;
        assert_eq!((slice.records.len(), piece.len()), (1, 83));
        assert_eq!(*piece, lifted(&frame[21..101]));
        let walked = framing::walk(piece);
        assert_eq!(walked.frames.len(), 1);
        let only = walked.frames[0];
        assert_eq!((only.kind, only.len()), (ChunkForm::Packet, 83));
        assert_eq!(only.payload(piece)[..2], LIFT_OPCODE);
        assert!(!Phase::Fight.keeps(LIFT_OPCODE));
        assert!(matches!(prelude_records(frame), Err(SliceFailure::Empty)));
    }

    #[test]
    fn touching_spans_stay_apart_and_overlapping_ones_merge() {
        let mut raw = vec![0x11; 400];
        raw[10..12].copy_from_slice(&DAMAGE);
        raw[170..172].copy_from_slice(&DAMAGE);
        raw[329..331].copy_from_slice(&DAMAGE);
        // 10..170 and 170..330 touch; 329..400 overlaps the second.
        assert_eq!(lift_spans(&raw, Phase::Fight), vec![10..170, 170..400]);
        assert!(lift_spans(&raw, Phase::Prelude).is_empty());
    }

    #[test]
    fn a_spawn_inside_a_packet_lifts_to_its_one_byte_length() {
        let frame = spawn_host(0x18, &[0xE8, 0x07]);
        assert_eq!(frame.len(), 56);
        for slice in [prelude_records(frame.clone()).unwrap(), fight_records(frame.clone()).unwrap()] {
            assert_eq!(slice.records.len(), 1);
            assert_eq!(slice.records[0].1, lifted(&frame[12..32]));
            assert_eq!(slice.records[0].1.len(), 23);
        }
    }

    #[test]
    fn a_spawn_length_too_short_copies_to_the_packet_end() {
        let frame = spawn_host(0x13, &[0xE8, 0x07]);
        let slice = prelude_records(frame.clone()).unwrap();
        assert_eq!(slice.records, vec![(PRELUDE_DT, lifted(&frame[12..56]))]);
    }

    #[test]
    fn a_spawn_behind_a_zero_byte_or_with_a_small_id_is_not_lifted() {
        for in_mode in [prelude_records, fight_records] {
            assert!(matches!(in_mode(spawn_host(0x00, &[0xE8, 0x07])), Err(SliceFailure::Empty)));
            assert!(matches!(in_mode(spawn_host(0x18, &[0x05, 0x22])), Err(SliceFailure::Empty)));
        }
    }

    #[test]
    fn a_spawn_is_lifted_only_where_the_decoder_reads_one() {
        // The first `41 36` fails on id 65 and the decoder steps past its
        // id byte, so the `41 36 E8 07` there is never read.
        let mut skipped = built::from_hex("05 4136 4136 E807");
        skipped.resize(40, 0x22);
        assert!(embedded_spawns(&skipped).is_empty());
        assert!(lift_spans(&skipped, Phase::Prelude).is_empty());

        let mut read = built::from_hex("05 1111 4136 E807");
        read.resize(40, 0x22);
        assert_eq!(embedded_spawns(&read), vec![(3, SPAWN, 1_000)]);
        assert_eq!(lift_spans(&read, Phase::Prelude), vec![3..40]);
    }

    #[test]
    fn a_two_byte_spawn_length_wins_over_its_last_byte() {
        let frame = big_spawn_host();
        for slice in [prelude_records(frame.clone()).unwrap(), fight_records(frame.clone()).unwrap()] {
            assert_eq!(slice.records.len(), 1);
            let piece = &slice.records[0].1;
            assert_eq!(piece.len(), 1_037);
            assert_eq!(*piece, lifted(&frame[8..1_041]));
            assert_eq!(framing::walk(piece).frames[0].payload(piece)[2..7], [0x41, 0x36, 0xC1, 0xB8, 0x02]);
        }
    }

    #[test]
    fn a_spawn_piece_ends_where_a_hit_starts() {
        let mut frame = big_spawn_host();
        frame[206] = 0x04;
        frame[207] = 0x38;
        let slice = prelude_records(frame.clone()).unwrap();
        assert_eq!(slice.records, vec![(PRELUDE_DT, lifted(&frame[8..206]))]);
        assert_eq!(slice.records[0].1.len(), 202);
        assert!(!slice.records[0].1.windows(2).any(|w| w == DAMAGE));

        let slice = fight_records(frame.clone()).unwrap();
        assert_eq!(slice.records, vec![(0, lifted(&frame[8..206])), (0, lifted(&frame[206..366]))]);
        assert_eq!((slice.records[0].1.len(), slice.records[1].1.len()), (202, 164));
    }

    #[test]
    fn a_spawn_with_no_readable_length_copies_a_fixed_stretch() {
        // `8D 88` in front of the spawn reads at no width that ends there.
        let mut body = built::from_hex("9936 1111 8D88 4136 C1B802");
        body.resize(700, 0x22);
        let frame = built::frame(&body);
        let slice = prelude_records(frame.clone()).unwrap();
        assert_eq!(slice.records, vec![(PRELUDE_DT, lifted(&frame[8..520]))]);

        // A length claiming more than the packet holds: copied to its end.
        let mut body = built::from_hex("9936 111111 7E 4136 C1B802");
        body.resize(30, 0x22);
        let frame = built::frame(&body);
        let slice = prelude_records(frame.clone()).unwrap();
        assert_eq!(slice.records, vec![(PRELUDE_DT, lifted(&frame[7..31]))]);
    }

    /// A cast packet's payload: mark `08 3B 38 00 00`, `99 38`, caster 300
    /// (`AC 02`), one byte, skill 11,001,234 (`92 DD A7 00`), then more.
    fn cast(mark_kind: &str, caster: &str, skill_end: &str) -> Vec<u8> {
        built::from_hex(&format!("AA3701 08{mark_kind}380000 9938 {caster} 07 92DD{skill_end}"))
    }

    fn full_cast() -> Vec<u8> {
        cast("3B", "AC02", "A700 5566")
    }

    #[test]
    fn a_cast_context_lifts_inside_a_fight_bundle() {
        let inner = [built::frame(&full_cast()), built::frame(&HIT)].concat();
        let slice = fight_records(built::bundle(&inner)).unwrap();
        let want = [built::from_hex("14 E5A2 083B380000 9938 AC02 07 92DDA700"), built::frame(&HIT)].concat();
        assert_eq!(slice.records, vec![(0, built::bundle(&want))]);

        assert!(matches!(prelude_records(built::bundle(&inner)), Err(SliceFailure::Empty)));
        assert!(matches!(fight_records(built::frame(&full_cast())), Err(SliceFailure::Empty)));
    }

    #[test]
    fn cast_context_needs_every_part() {
        assert_eq!(cast_context(&full_cast()), Some(built::from_hex("14 E5A2 083B380000 9938 AC02 07 92DDA700")));
        assert_eq!(cast_context(&cast("3D", "AC02", "A700 5566")), Some(built::from_hex("14 E5A2 083D380000 9938 AC02 07 92DDA700")));
        assert_eq!(cast_context(&cast("3B", "AC02", "A7")), Some(built::from_hex("13 E5A2 083B380000 9938 AC02 07 92DDA7")));
        assert_eq!(cast_context(&cast("3B", "AC02", "")), None);
        assert_eq!(cast_context(&cast("3B", "63", "A700 5566")), None);
        let mut broken_mark = full_cast();
        broken_mark[7] = 0x01;
        assert_eq!(cast_context(&broken_mark), None);
    }

    #[test]
    fn a_record_time_is_clamped_to_i32() {
        let segments = [cap(2_200_000_000, &SHORT_HIT)];
        let slice = build(&segments, 0, 3_000_000_000, &NameMap::new()).unwrap();
        assert_eq!(slice.records[0].0, i32::MAX);
    }

    #[test]
    fn the_blind_map_holds_the_tokens_of_named_entries() {
        let names: NameMap = [("Abcd".to_string(), 7), (String::new(), 3)].into_iter().collect();
        let slice = build(&[cap(0, &[0x23, 0x36, 0x00])], 0, 10, &names).unwrap();
        let blinder = Blinder::new(&format::in_search_order(&names));
        let token = String::from_utf8_lossy(blinder.token_of("Abcd").unwrap()).into_owned();
        assert_eq!(slice.blind_map, HashMap::from([(token, 7)]));
    }

    #[test]
    fn a_name_spread_over_two_frames_of_a_bundle_is_a_leak() {
        let names: NameMap = [("ZA".to_string(), 0)].into_iter().collect();
        let first = built::frame(&[0x1B, 0x92, 0x00, 0x00, 0x00, b'Z']);
        let mut second_body = vec![0x1B, 0x92];
        second_body.extend([0x00; 59]);
        let second = built::frame(&second_body);
        assert_eq!(second[0], b'A');
        let inner = [first.clone(), second.clone()].concat();
        let at_fight = |bytes: Vec<u8>| build(&[seg(0, "Client:1", bytes)], 0, 10, &names);
        assert!(matches!(at_fight(built::bundle(&inner)), Err(SliceFailure::NameLeaked(2))));
        assert_eq!(at_fight(inner).unwrap().records.len(), 2);
    }

    #[test]
    fn a_name_field_spelling_a_one_byte_name_is_a_leak() {
        let names: NameMap = [("Q".to_string(), 5)].into_iter().collect();
        let ordered = format::in_search_order(&names);
        let record = built::player_record(4_242, Some("Q"), &[0; 12]);
        assert!(matches!(find_leak(&[(0, record.clone())], &ordered), Err(SliceFailure::NameLeaked(1))));
        assert!(matches!(find_leak(&[(0, built::bundle(&record))], &ordered), Err(SliceFailure::NameLeaked(1))));
        assert!(find_leak(&[(0, nested(&record, 5))], &ordered).is_ok());
        let other: NameMap = [("R".to_string(), 5)].into_iter().collect();
        assert!(find_leak(&[(0, record)], &format::in_search_order(&other)).is_ok());
    }

    #[test]
    fn stats_count_every_frame_met() {
        let mut bytes = built::frame(&HIT);
        bytes.extend(host(100, &[20]));
        let mut inner = built::frame(&MOB);
        inner.extend(built::frame(&CHAT));
        bytes.extend(built::bundle(&inner));
        let piece = lifted(&host(100, &[20])[21..101]);

        let slice = fight_records(bytes.clone()).unwrap();
        assert_eq!(slice.records, vec![(0, built::frame(&HIT)), (0, piece), (0, built::bundle(&built::frame(&MOB)))]);
        let s = &slice.stats;
        assert_eq!((s.packets_seen, s.packets_kept, s.bytes_seen, s.bytes_kept, s.bundles_expanded), (4, 2, 117, 95, 1));

        let slice = prelude_records(bytes).unwrap();
        assert_eq!(slice.records, vec![(PRELUDE_DT, built::bundle(&built::frame(&MOB)))]);
        let s = &slice.stats;
        assert_eq!((s.packets_seen, s.packets_kept, s.bytes_seen, s.bytes_kept, s.bundles_expanded), (4, 1, 117, 6, 1));
    }

    #[test]
    fn expand_opens_bundles_four_deep() {
        let a = built::frame(&SHORT_HIT);
        let b = built::frame(&MOB);
        assert_eq!(expand(&a), a);
        assert_eq!(expand(&built::bundle(&[a.clone(), b.clone()].concat())), [a.clone(), b].concat());
        assert_eq!(expand(&[&[0x00, 0x00][..], &a].concat()), a);
        assert_eq!(expand(&nested(&a, 4)), a);
        assert!(expand(&nested(&a, 5)).is_empty());
        assert!(expand(&built::frame(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x01])).is_empty());
    }

    #[test]
    fn framing_helpers_match_the_test_builders() {
        assert_eq!(wrap_record(&HIT), Some(built::frame(&HIT)));
        let long = vec![0x5A; 200];
        let frame = wrap_record(&long).unwrap();
        assert_eq!(frame[..2], [0xCC, 0x01]);
        assert_eq!(frame, built::frame(&long));
        assert_eq!(wrap_record(&[]), Some(vec![0x04]));
    }

    #[test]
    fn a_packed_bundle_opens_to_what_went_in() {
        let inner = [7u8, 8, 9];
        let frame = pack_bundle(&inner).unwrap();
        let walked = framing::walk(&frame);
        assert_eq!(walked.frames.len(), 1);
        let payload = walked.frames[0].payload(&frame);
        assert_eq!(payload[..6], [0xFF, 0xFF, 0x03, 0x00, 0x00, 0x00]);
        assert_eq!(framing::inflate_block(payload).unwrap(), inner);
    }

    #[test]
    fn events_are_known_by_the_two_bytes_after_the_length() {
        assert!(carries_event(&built::frame(&[0x04, 0x38])));
        assert!(carries_event(&built::frame(&[0x05, 0x38, 0x01])));
        assert!(carries_event(&built::frame(&[0x1B, 0x92, 0x01])));
        assert!(!carries_event(&built::frame(&[0x04])));
        assert!(!carries_event(&built::frame(&MOB)));
        assert!(!carries_event(&[]));
        assert!(!carries_event(&[0x80]));
    }

    #[test]
    fn the_same_input_builds_the_same_slice() {
        let names: NameMap = [("Mirelle".to_string(), 11), ("Tovan".to_string(), 12)].into_iter().collect();
        let segments = [
            seg(0, "Client:1", built::self_record(500, "Mirelle", 1001, 2, 40)),
            seg(5, "Client:1", built::frame(&[0x23, 0x36, 0x04, b'W', b'x', b'y', b'z'])),
            seg(100_000, "Client:1", host(100, &[20])),
            seg(100_000, "Client:2", built::bundle(&[built::frame(&MOB), built::frame(&HIT)].concat())),
        ];
        let one = build(&segments, 100_000, 110_000, &names).unwrap();
        let two = build(&segments, 100_000, 110_000, &names).unwrap();
        assert_eq!(one.records, two.records);
        assert_eq!(one.blind_map, two.blind_map);
        assert_eq!(format!("{:?}", one.stats), format!("{:?}", two.stats));
        assert!(one.records.iter().all(|(_, r)| !expand(r).windows(7).any(|w| w == b"Mirelle")));
    }

    #[test]
    fn events_wait_for_the_lead_in_but_state_does_not() {
        assert!(matches!(prelude_records(built::frame(&HIT)), Err(SliceFailure::Empty)));
        assert_eq!(fight_records(built::frame(&HIT)).unwrap().records, vec![(0, built::frame(&HIT))]);
        assert_eq!(prelude_records(built::frame(&MOB)).unwrap().records, vec![(PRELUDE_DT, built::frame(&MOB))]);
    }

    #[test]
    fn scope_and_map_loads_stay_without_their_scene_text() {
        let scope = built::packet([0x06, 0x38], &[F::V(23001), F::U32(0x0012_3456), F::U16(12)]);
        let world = built::map_load(1, 1010, 0, 0x4F);
        let scene = built::packet(
            [0x21, 0x36],
            &[
                F::U32(3),
                F::U32(777_001),
                F::U32(0x0001_2345),
                F::U32(0),
                F::F32(1000.0),
                F::F32(2000.0),
                F::F32(300.0),
                F::F32(45.0),
                F::U32(2),
                F::Raw(&[0; 9]),
                F::U8(0x39),
                F::Str("Scene_Test_0001"),
                F::U8(0),
            ],
        );
        let segments = [
            seg(0, "Client:1", scope.clone()),
            seg(0, "Client:1", world),
            seg(100_000, "Client:1", scene),
            seg(100_000, "Client:1", scope),
        ];
        let slice = build(&segments, 100_000, 110_000, &NameMap::new()).unwrap();
        assert_eq!(slice.records.len(), 4);
        let all: Vec<u8> = slice.records.iter().flat_map(|(_, r)| r.clone()).collect();
        assert!(varint::index_of_bytes(&all, 0, &[0x29, 0xDB, 0x0B, 0x00]).is_some());
        assert!(varint::index_of_bytes(&all, 0, b"Scene_Te").is_none());

        let storage = Arc::new(DataStorage::new());
        storage.enter_dungeon(600021);
        let mut p = PacketDecoder::new(storage.clone(), Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        for (_, record) in &slice.records[..2] {
            p.decode_buffer(record);
        }
        assert_eq!(storage.current_dungeon_id(), 0);
    }

    #[test]
    fn unlisted_packets_are_counted_then_dropped() {
        let segments = [cap(1000, &[0x04, 0x38, 0x01, 0x02]), cap(1000, &[0xAB, 0xCD, 0x63, 0x68, 0x61, 0x74])];
        let slice = build(&segments, 1000, 2000, &NameMap::new()).unwrap();
        assert_eq!(slice.records.len(), 1);
        assert_eq!((slice.stats.packets_seen, slice.stats.packets_kept), (2, 1));
    }

    #[test]
    fn hits_outside_the_lead_in_and_tail_are_dropped() {
        let segments: Vec<_> = [0, 40_001, 100_000, 215_001].iter().map(|&t| cap(t, &SHORT_HIT)).collect();
        let slice = build(&segments, 100_000, 200_000, &NameMap::new()).unwrap();
        let times: Vec<i32> = slice.records.iter().map(|(t, _)| *t).collect();
        assert_eq!(times, [-59_999, 0]);
    }

    #[test]
    fn record_time_counts_from_the_fight_start() {
        let slice =
            build(&[cap(1_700_000_005_000, &SHORT_HIT)], 1_700_000_000_000, 1_700_000_010_000, &NameMap::new()).unwrap();
        assert_eq!(slice.records[0].0, 5_000);
    }

    #[test]
    fn a_slice_with_nothing_kept_is_an_error() {
        let result = build(&[cap(0, &SHORT_HIT)], 10_000_000, 10_001_000, &NameMap::new());
        assert!(matches!(result, Err(SliceFailure::Empty)));
    }

    #[test]
    fn a_fight_at_the_start_of_time_has_its_window_cut_at_i64_min() {
        let start = i64::MIN + 100_000;
        assert!(matches!(build(&[], start, start + 10, &NameMap::new()), Err(SliceFailure::Empty)));
    }
}
