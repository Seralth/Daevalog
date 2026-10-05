//! The dry run: cut a fight's slice from packet captures, and write what an upload would send to disk.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};

use super::envelope::build_envelope;
use crate::capture::evidence_slice::{self, CapturedPacket, NameMap};
use crate::capture::packet_accumulator::PacketAccumulator;
use crate::capture::stream_processor::StreamProcessor;
use crate::combat::data_storage::DataStorage;
use crate::entity::fight_record::FightRecord;
use crate::i18n::lookup::{NpcLookup, SkillLookup};
use crate::platform::files;

/// What the dry run produced, for the UI to show.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResult {
    pub out_dir: String,
    pub slice_path: String,
    pub envelope_path: String,
    pub slice_bytes: usize,
    /// What an upload would actually send — the slice gzipped.
    pub slice_compressed_bytes: usize,
    pub envelope_bytes: usize,
    pub packets_seen: usize,
    pub packets_kept: usize,
    pub bytes_seen: usize,
    pub bytes_kept: usize,
    pub names_blinded: usize,
    pub participants: usize,
    /// Captures that were read to build this.
    pub sources: Vec<String>,
}

/// Parse a `packets_*.txt` capture into the buffers the slice builder wants.
///
/// Whole file, not just the fight window: the builder reassembles TCP streams,
/// and starting halfway through one means framing from the middle of a packet.
pub fn read_capture(path: &Path) -> Result<Vec<CapturedPacket>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(ts), Some(key), Some(hex)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Ok(when) = chrono::DateTime::parse_from_rfc3339(ts.trim()) else {
            continue;
        };
        let Some(bytes) = decode_hex(hex) else {
            continue;
        };
        out.push(CapturedPacket {
            captured_at_ms: when.timestamp_millis(),
            stream: key.to_string(),
            bytes,
        });
    }
    Ok(out)
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    let b = hex.as_bytes();
    if b.len() % 2 != 0 || b.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// Gzip a buffer.
///
/// Worth doing even though a slice already holds LZ4-compressed bundles: the
/// framing, the repeated opcodes and the hex-free binary still compress about
/// 2.2x, measured on the reference run (467 KB -> 212 KB). For a diagnostic
/// capture, which is ASCII hex, it is closer to 3x — the difference between an
/// 8 MB upload and a 2.8 MB one, and the reason a size limit can be generous.
pub fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 2), Compression::best());
    encoder.write_all(data).map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())
}

/// Every capture the packet logger has written, newest first.
pub fn find_captures(app_data_dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = match std::fs::read_dir(app_data_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("packets_") && n.ends_with(".txt"))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out.reverse();
    out
}

/// Does this capture cover the fight?
pub(super) fn covers(packets: &[CapturedPacket], start_ms: i64, end_ms: i64) -> bool {
    let (Some(first), Some(last)) = (packets.first(), packets.last()) else {
        return false;
    };
    first.captured_at_ms <= end_ms && last.captured_at_ms >= start_ms
}

/// Replay a capture to learn the character names in it.
///
/// The saved fight record cannot supply these: `obscure_nickname` masks every
/// party member before the record is written, so it holds `Ta****x`, and the
/// blinder needs the real bytes to find and replace them.
fn resolve_names(packets: &[CapturedPacket]) -> NameMap {
    let storage = std::sync::Arc::new(DataStorage::new());
    let mut processor = StreamProcessor::new(
        storage.clone(),
        std::sync::Arc::new(SkillLookup::new()),
        std::sync::Arc::new(NpcLookup::new()),
    );
    let mut streams: HashMap<&str, PacketAccumulator> = HashMap::new();
    for cap in packets {
        processor.set_override_timestamp(Some(cap.captured_at_ms));
        let acc = streams
            .entry(cap.stream.as_str())
            .or_insert_with(PacketAccumulator::new);
        acc.append(&cap.bytes);
        let consumed = processor.consume_stream(acc.snapshot());
        if consumed > 0 {
            acc.discard_bytes(consumed);
        }
    }
    processor.set_override_timestamp(None);

    let mut names = NameMap::new();
    for (name, member) in storage.get_party_members() {
        names.insert(name, member.dbid);
    }
    for name in storage.get_nicknames().into_values() {
        names.entry(name).or_insert(0);
    }
    names
}

/// Write the two files an upload would send, and return what was written.
///
/// Never touches the network. That is the whole point of it.
/// Cut the Evidence Slice for a saved fight from whichever captures cover it.
///
/// Returns the encoded slice and what building it kept and dropped. This is
/// the one place a slice is made from a capture; the preview writes it to
/// disk, an upload sends it.
pub fn slice_for(
    record: &FightRecord,
    captures: &[PathBuf],
) -> Result<(Vec<u8>, evidence_slice::EvidenceSlice, Vec<String>), String> {
    let start = record.start_time_ms;
    let end = record.start_time_ms + record.duration_ms;

    let mut packets: Vec<CapturedPacket> = Vec::new();
    let mut sources = Vec::new();
    for path in captures {
        let parsed = read_capture(path)?;
        if !covers(&parsed, start - evidence_slice::LEAD_IN_MS, end + evidence_slice::TAIL_MS) {
            continue;
        }
        sources.push(path.display().to_string());
        packets.extend(parsed);
    }
    if packets.is_empty() {
        return Err(
            "No packet capture covers this fight. Packet logging is off by default —              turn it on in Settings → Diagnostics and fight again, then preview."
                .into(),
        );
    }
    packets.sort_by_key(|p| p.captured_at_ms);

    let names = resolve_names(&packets);
    let mut slice = evidence_slice::build(&packets, start, end, &names).map_err(|e| e.to_string())?;
    without_roster_ids(&mut slice);
    let encoded = evidence_slice::encode(&slice);
    Ok((encoded, slice, sources))
}

/// Blank the roster ids in a slice's blind table before it is written or sent.
///
/// The builder records which roster id each blinded name belonged to, so a
/// service could join uploads to accounts. Nothing does, and a roster id is a
/// stable handle on a person: every party member's would leave the machine
/// with every upload, for no purpose. The tokens stay (the parser needs names
/// to be distinct), the ids do not.
pub(super) fn without_roster_ids(slice: &mut evidence_slice::EvidenceSlice) {
    for id in slice.blind_map.values_mut() {
        *id = 0;
    }
}

pub fn preview(
    record: &FightRecord,
    captures: &[PathBuf],
    out_dir: &Path,
) -> Result<PreviewResult, String> {
    let (encoded, slice, sources) = slice_for(record, captures)?;
    let envelope = build_envelope(record, &encoded);
    let json = serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())?;

    let compressed = gzip(&encoded)?;

    files::create_private_dir(out_dir).map_err(|e| e.to_string())?;
    // Both: the `.a2es` is what `a2t-inspect` reads, the `.gz` is byte for byte
    // what an upload would put on the wire. Writing only the compressed one
    // would make the artifact harder to check, which is the opposite of why
    // this exists.
    let slice_path = out_dir.join(format!("{}.a2es", record.id));
    let compressed_path = out_dir.join(format!("{}.a2es.gz", record.id));
    let envelope_path = out_dir.join(format!("{}.upload.json", record.id));
    files::write_private(&slice_path, &encoded).map_err(|e| e.to_string())?;
    files::write_private(&compressed_path, &compressed).map_err(|e| e.to_string())?;
    files::write_private(&envelope_path, json.as_bytes()).map_err(|e| e.to_string())?;

    Ok(PreviewResult {
        out_dir: out_dir.display().to_string(),
        slice_path: slice_path.display().to_string(),
        envelope_path: envelope_path.display().to_string(),
        slice_bytes: encoded.len(),
        slice_compressed_bytes: compressed.len(),
        envelope_bytes: json.len(),
        packets_seen: slice.stats.packets_seen,
        packets_kept: slice.stats.packets_kept,
        bytes_seen: slice.stats.bytes_seen,
        bytes_kept: slice.stats.bytes_kept,
        names_blinded: slice.stats.names_blinded,
        participants: envelope.participants.len(),
        sources,
    })
}
