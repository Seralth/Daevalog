//! Where one packet ends and the next begins.
//!
//! `StreamProcessor::consume_stream` used to walk this itself, inline with the
//! parsing it drives. It is split out here because the Evidence Slice builder
//! needs the *same* walk: it decides what to upload per packet, and a second,
//! subtly different framing implementation is exactly the kind of divergence
//! that turns "the server re-derived different numbers" into a permanent
//! mystery. One walk, two callers.
//!
//! The wire shape, as the parser understands it:
//!
//! ```text
//! 00 ...                      padding, skipped
//! <varint len> <payload>      a packet; physical size is len - 3 (an AION 2 quirk)
//! <varint len> FF FF <u32 size> <lz4>   a compressed bundle of further packets
//! ```

use super::stream_processor::read_varint;

/// The largest packet the parser will believe. Past this it treats the length as
/// garbage and resynchronises a byte at a time.
const MAX_PACKET_BYTES: usize = 65535;
/// A length above this that runs past the buffer is treated as corruption rather
/// than as a TCP fragment worth waiting for.
const MAX_FRAGMENT_WAIT_BYTES: usize = 16384;
/// Refuse to allocate for a bundle claiming to decompress to more than this.
const MAX_DECOMPRESSED_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// A plain packet. `range` covers the whole thing, length prefix included.
    Packet,
    /// An `FF FF` LZ4 bundle holding further packets. `range` covers the whole
    /// frame; `payload_start` is the offset of the first `FF` within it.
    Bundle,
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub kind: FrameKind,
    /// Byte range within the buffer this frame was walked from.
    pub start: usize,
    pub end: usize,
    /// Offset from `start` at which the payload begins (i.e. the length prefix
    /// width). For a bundle this is where the `FF FF` sits.
    pub payload_start: usize,
}

impl Frame {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
    pub fn bytes<'a>(&self, buffer: &'a [u8]) -> &'a [u8] {
        &buffer[self.start..self.end]
    }
    pub fn payload<'a>(&self, buffer: &'a [u8]) -> &'a [u8] {
        &buffer[self.start + self.payload_start..self.end]
    }
}

/// The result of walking a buffer: the frames found, and how many bytes were
/// consumed. Bytes past `consumed` are an incomplete trailing packet and must be
/// kept for the next read.
#[derive(Debug, Default)]
pub struct Framing {
    pub frames: Vec<Frame>,
    pub consumed: usize,
}

/// Walk `buffer` into frames, stopping at the first incomplete one.
///
/// This is a transcription of the walk `consume_stream` performed inline, and it
/// must stay one — including the resync behaviour, which is load-bearing: a
/// capture that starts mid-stream is the normal case, not the exception.
pub fn walk(buffer: &[u8]) -> Framing {
    let mut out = Framing::default();
    let mut offset = 0usize;

    while offset < buffer.len() {
        // 1. Skip zero padding.
        if buffer[offset] == 0x00 {
            offset += 1;
            continue;
        }

        let length_info = read_varint(buffer, offset);
        if length_info.length <= 0 || length_info.value <= 0 {
            if offset + 5 > buffer.len() {
                break;
            }
            offset += 1;
            continue;
        }

        // 2. AION 2 quirk: length - 3 == physical size.
        let total_packet_bytes = (length_info.value - 3) as usize;

        // Resync on invalid sizes.
        if total_packet_bytes == 0 || total_packet_bytes > MAX_PACKET_BYTES {
            offset += 1;
            continue;
        }

        // 3. TCP fragmentation check (anti-stall gate).
        if offset + total_packet_bytes > buffer.len() {
            if total_packet_bytes > MAX_FRAGMENT_WAIT_BYTES {
                offset += 1;
                continue;
            }
            break; // Legitimate fragment — wait for more bytes.
        }

        // 4. Check for an FF FF compressed bundle.
        let payload_start = length_info.length as usize;
        let is_bundle = payload_start + 1 < total_packet_bytes
            && buffer[offset + payload_start] == 0xFF
            && buffer[offset + payload_start + 1] == 0xFF;

        if is_bundle {
            // A bundle is one byte longer than its declared size.
            let bundle_size = total_packet_bytes + 1;
            if offset + bundle_size > buffer.len() {
                break;
            }
            out.frames.push(Frame {
                kind: FrameKind::Bundle,
                start: offset,
                end: offset + bundle_size,
                payload_start,
            });
            offset += bundle_size;
        } else {
            out.frames.push(Frame {
                kind: FrameKind::Packet,
                start: offset,
                end: offset + total_packet_bytes,
                payload_start,
            });
            offset += total_packet_bytes;
        }
    }

    out.consumed = offset;
    out
}

/// Walk the *decompressed* contents of a bundle.
///
/// Same varint framing as [`walk`], but the resync rules differ and the
/// difference is deliberate, not an oversight in either place: the outer walk
/// reads a TCP stream that routinely starts mid-packet, so it resynchronises a
/// byte at a time. This one reads a buffer the game itself framed, so a length
/// that does not parse means the decompression or the framing assumption is
/// wrong, and walking further would invent packets. It stops instead.
///
/// The other asymmetry to preserve: an outer bundle occupies `len - 3 + 1`
/// bytes, a nested one occupies `len - 3`. That extra byte is real and dropping
/// it desynchronises the rest of the buffer.
pub fn walk_inner(buffer: &[u8]) -> Framing {
    let mut out = Framing::default();
    let mut offset = 0usize;

    while offset < buffer.len() {
        if buffer[offset] == 0x00 {
            offset += 1;
            continue;
        }

        let length_info = read_varint(buffer, offset);
        if length_info.length <= 0 || length_info.value <= 0 {
            break;
        }

        // A length of 3 or less would give a zero-or-negative physical size.
        if length_info.value <= 3 {
            offset += 1;
            continue;
        }
        let total = (length_info.value - 3) as usize;

        let end = offset + total;
        if end > buffer.len() {
            break;
        }

        let payload_start = length_info.length as usize;
        let is_nested_bundle = total > payload_start + 1
            && buffer[offset + payload_start] == 0xFF
            && buffer[offset + payload_start + 1] == 0xFF;

        out.frames.push(Frame {
            kind: if is_nested_bundle { FrameKind::Bundle } else { FrameKind::Packet },
            start: offset,
            end,
            payload_start,
        });

        // Note: no `+ 1` here, unlike the outer walk.
        offset += total;
    }

    out.consumed = offset;
    out
}

/// Decompress a bundle payload (one that starts at its `FF FF`).
///
/// `FF FF (2) + decompressed_size (4 LE) + lz4 block`. Returns `None` for
/// anything malformed, because a bundle that will not decompress is a resync
/// artefact rather than an error worth surfacing.
pub fn decompress_bundle(payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() < 7 {
        return None;
    }
    let size = u32::from_le_bytes([payload[2], payload[3], payload[4], payload[5]]) as usize;
    if size == 0 || size > MAX_DECOMPRESSED_BYTES {
        return None;
    }
    lz4_flex::decompress(&payload[6..], size).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<varint len>` where len = payload + 3, per the quirk above.
    fn packet(payload: &[u8]) -> Vec<u8> {
        let total = payload.len() + 1; // 1-byte length prefix
        let mut v = vec![(total + 3) as u8];
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn walks_back_to_back_packets() {
        let mut buf = packet(&[0x23, 0x36, 0x01]);
        buf.extend(packet(&[0x41, 0x36, 0x02, 0x03]));
        let f = walk(&buf);
        assert_eq!(f.frames.len(), 2);
        assert_eq!(f.consumed, buf.len());
        assert_eq!(f.frames[0].payload(&buf), &[0x23, 0x36, 0x01]);
        assert_eq!(f.frames[1].payload(&buf), &[0x41, 0x36, 0x02, 0x03]);
    }

    #[test]
    fn skips_zero_padding() {
        let mut buf = vec![0x00, 0x00];
        buf.extend(packet(&[0xAA, 0xBB]));
        let f = walk(&buf);
        assert_eq!(f.frames.len(), 1);
        assert_eq!(f.frames[0].payload(&buf), &[0xAA, 0xBB]);
    }

    #[test]
    fn stops_on_an_incomplete_trailing_packet() {
        let mut buf = packet(&[1, 2, 3]);
        let full = buf.len();
        buf.push(0x40); // claims 0x40 - 3 = 61 bytes, none of which are here
        let f = walk(&buf);
        assert_eq!(f.frames.len(), 1);
        assert_eq!(f.consumed, full, "the fragment must be left for the next read");
    }

    #[test]
    fn recognises_a_bundle_and_its_extra_byte() {
        // payload = FF FF + 4-byte size + some lz4-ish bytes
        let payload = [0xFF, 0xFF, 0x10, 0, 0, 0, 0xAA, 0xBB];
        let buf = {
            let mut v = packet(&payload);
            v.push(0x00); // the bundle's trailing extra byte
            v
        };
        let f = walk(&buf);
        assert_eq!(f.frames.len(), 1);
        assert_eq!(f.frames[0].kind, FrameKind::Bundle);
        assert_eq!(f.frames[0].len(), buf.len(), "bundle spans the extra byte");
    }
}
