use super::varint::varint_at;
/// The stretches of `buffer[..end]` a record can sit in: each packet, and each
/// run of bytes between frames (padding, a walk resynchronising, the end of a
/// bundle that does not frame). A record never runs on into the next frame.
/// Bundles are left out: their records are read from the bundle opened, and
/// LZ4 keeps the bytes of two packets side by side in its literals.
pub fn regions(frames: &[Frame], end: usize) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::with_capacity(frames.len() * 2 + 1);
    let mut at = 0;
    for frame in frames {
        if frame.start > at {
            out.push(at..frame.start);
        }
        if frame.kind == ChunkForm::Packet {
            out.push(frame.start..frame.end);
        }
        at = frame.end;
    }
    if end > at {
        out.push(at..end);
    }
    out
}

/// Bytes a frame occupies, given its length varint's value and width. `None`
/// when that is shorter than the varint itself.
pub fn frame_size(value: i32, varint_bytes: i32) -> Option<usize> {
    let size = value as i64 - 4 + varint_bytes as i64;
    (size >= varint_bytes as i64 && size > 0).then_some(size as usize)
}

/// The length varint's value for a frame whose bytes after the varint are
/// `body_len` long.
pub fn length_value(body_len: usize) -> u32 {
    body_len as u32 + 4
}

// How the server's stream is cut into frames.
//
// Each frame is a length varint and a payload; the varint holds the payload
// length plus 4 (see `frame_size`). A payload that opens `FF FF` is a bundle:
// `FF FF <u32 LE size> <one raw LZ4 block>`, whose contents are frames again.
// Zero bytes may sit between frames. The capture can start mid-frame and a
// read can end mid-frame, so the top-level walk steps past lengths it cannot
// believe and waits for a frame still arriving. Bundle contents come from the
// game whole and are walked strictly.

/// How deep bundles inside bundles are opened; the outermost is depth 1.
/// The callers enforce it.
pub const NEST_LIMIT: usize = 4;

/// No top-level frame is longer; a longer length is noise.
const LARGEST_FRAME: usize = 65_535;
/// A cut frame this long or shorter is waited for even without a bundle
/// header. Longer ones are waited for only when they open like a bundle.
const CUT_FRAME_WAIT: usize = 16_384;
/// Most bytes a bundle may claim once opened.
const BUNDLE_CEILING: usize = 1_000_000;
/// `FF FF` and the u32 size, in front of a bundle's LZ4 block.
const BUNDLE_HEAD: usize = 6;
/// A length varint never runs longer, so a failed read with this many bytes
/// on hand is noise, not a varint still arriving.
const LONGEST_VARINT: usize = 5;

/// The frames found and how many leading bytes are done with. The bytes from
/// `consumed` on are a frame still arriving (or nothing) and are kept for the
/// next read.
#[derive(Debug, Default)]
pub struct Framing {
    pub consumed: usize,
    pub frames: Vec<Frame>,
}

/// Walks the server's stream as captured.
pub fn walk(buffer: &[u8]) -> Framing {
    scan(buffer, Source::Wire)
}

/// Walks a bundle's contents, which the game framed itself: anything that
/// does not frame ends the walk.
pub fn split_frames(buffer: &[u8]) -> Framing {
    scan(buffer, Source::Bundle)
}

/// One frame: `buffer[start..end]`, its length varint included.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub start: usize,
    /// Bytes of the length varint at `start`.
    pub varint_width: usize,
    pub end: usize,
    pub kind: ChunkForm,
}

impl Frame {
    /// The frame after its length varint. A bundle's starts at its `FF FF`,
    /// which is what `inflate_block` takes.
    pub fn payload<'a>(&self, buffer: &'a [u8]) -> &'a [u8] {
        &self.bytes(buffer)[self.varint_width..]
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes<'a>(&self, buffer: &'a [u8]) -> &'a [u8] {
        buffer.get(self.start..self.end).expect("a frame is read from the buffer it was walked in")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkForm {
    /// Opens `FF FF`: an LZ4 block of more frames.
    Bundle,
    Packet,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Wire,
    Bundle,
}

fn scan(buffer: &[u8], source: Source) -> Framing {
    let lenient = source == Source::Wire;
    let mut frames = Vec::new();
    let mut at = 0;
    while at < buffer.len() {
        if buffer[at] == 0 {
            at += 1;
            continue;
        }
        let left = buffer.len() - at;
        let head = varint_at(buffer, at);
        if head.length <= 0 || head.value <= 0 {
            if lenient && left >= LONGEST_VARINT {
                at += 1;
                continue;
            }
            break;
        }
        let Some(size) = frame_size(head.value, head.length) else {
            at += 1;
            continue;
        };
        let width = head.length as usize;
        if lenient && size > LARGEST_FRAME {
            at += 1;
            continue;
        }
        if size > left {
            let worth_waiting = !lenient || size <= CUT_FRAME_WAIT || opens_like_bundle(&buffer[at + width..]);
            if worth_waiting {
                break;
            }
            at += 1;
            continue;
        }
        let end = at + size;
        let kind = if buffer[at + width..end].starts_with(&[0xFF, 0xFF]) { ChunkForm::Bundle } else { ChunkForm::Packet };
        frames.push(Frame { kind, start: at, end, varint_width: width });
        at = end;
    }
    Framing { frames, consumed: at }
}

/// Whether a cut frame's first bytes after its varint are a whole bundle
/// header with a size worth believing.
fn opens_like_bundle(after_varint: &[u8]) -> bool {
    match after_varint.get(..BUNDLE_HEAD) {
        Some([0xFF, 0xFF, size @ ..]) => {
            let claimed = super::varint::le_u32_at(size, 0) as usize;
            (1..=BUNDLE_CEILING).contains(&claimed)
        }
        _ => false,
    }
}

/// Opens a bundle frame's payload (`FF FF <size> <lz4 block>`; the `FF FF`
/// is not checked). `None` for a size of 0 or over the ceiling, or a block
/// that does not decode within the size. Output shorter than the size is
/// returned as it is.
pub fn inflate_block(payload: &[u8]) -> Option<Vec<u8>> {
    if payload.len() <= BUNDLE_HEAD {
        return None;
    }
    let claimed = super::varint::le_u32_at(payload, 2) as usize;
    if claimed == 0 || claimed > BUNDLE_CEILING {
        return None;
    }
    lz4_flex::decompress(&payload[BUNDLE_HEAD..], claimed).ok()
}

/// A bundle frame sitting inside a packet's bytes: `packet[start..end]`,
/// and what it opens to.
#[derive(Debug, Clone)]
pub struct EmbeddedBundle {
    pub start: usize,
    pub end: usize,
    pub data: Vec<u8>,
}

/// Bundle frames inside one whole packet (length varint included), which the
/// walk never opens. Every `FF FF` from index 1 on with at least 9 bytes from
/// it to the end is tried as the start of a bundle payload, behind a length
/// varint of 3, 2 or 1 bytes. A found bundle is skipped whole.
pub fn embedded_bundles(packet: &[u8]) -> Vec<EmbeddedBundle> {
    let mut found = Vec::new();
    let mut i = 1;
    while i + 8 < packet.len() {
        let hit = if packet[i] == 0xFF && packet[i + 1] == 0xFF { bundle_at(packet, i) } else { None };
        match hit {
            Some(b) => {
                i = b.end;
                found.push(b);
            }
            None => i += 1,
        }
    }
    found
}

/// The bundle whose payload starts at `packet[ff]`, if a length varint right
/// in front of it frames one that opens.
fn bundle_at(packet: &[u8], ff: usize) -> Option<EmbeddedBundle> {
    [3usize, 2, 1].into_iter().filter(|&w| w <= ff).find_map(|w| {
        let start = ff - w;
        let head = varint_at(packet, start);
        if head.length != w as i32 {
            return None;
        }
        let end = start + frame_size(head.value, head.length)?;
        let data = inflate_block(packet.get(ff..end)?)?;
        Some(EmbeddedBundle { start, end, data })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{bundle, frame, varint};

    /// Bytes written as space-separated hex pairs.
    fn hex(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for pair in text.split(' ') {
            out.push(u8::from_str_radix(pair, 16).expect("hex pair"));
        }
        out
    }

    /// Where each top-level frame starts, and how much the walk used.
    fn starts(buffer: &[u8]) -> (Vec<usize>, usize) {
        let walked = walk(buffer);
        (walked.frames.iter().map(|one| one.start).collect(), walked.consumed)
    }

    /// The same for bundle contents.
    fn inner_starts(buffer: &[u8]) -> (Vec<usize>, usize) {
        let walked = split_frames(buffer);
        (walked.frames.iter().map(|one| one.start).collect(), walked.consumed)
    }

    fn kinds(walked: &Framing) -> Vec<ChunkForm> {
        let mut out = Vec::new();
        for one in &walked.frames {
            out.push(one.kind);
        }
        out
    }

    fn payloads(buffer: &[u8], f: &Framing) -> Vec<Vec<u8>> {
        f.frames.iter().map(|x| x.payload(buffer).to_vec()).collect()
    }

    /// A payload with opcode `<op> 36`, every other byte non-zero and not `FF`.
    fn body(op: u8, len: usize) -> Vec<u8> {
        let mut b = vec![op, 0x36];
        b.extend((0..len.saturating_sub(2)).map(|i| (i % 251) as u8 + 1));
        b.truncate(len);
        b
    }

    /// Bytes LZ4 cannot shrink.
    fn noise(len: usize, seed: &mut u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (*seed >> 33) as u8
            })
            .collect()
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    // Top-level walk.

    #[test]
    fn back_to_back_frames_are_all_taken() {
        let buf = cat(&[&frame(&hex("23 36 01")), &frame(&hex("41 36 02 03"))]);
        assert_eq!(buf, hex("07 23 36 01 08 41 36 02 03"));
        let f = walk(&buf);
        assert_eq!(f.consumed, 9);
        assert_eq!(payloads(&buf, &f), vec![hex("23 36 01"), hex("41 36 02 03")]);
        assert_eq!(kinds(&f), vec![ChunkForm::Packet, ChunkForm::Packet]);
    }

    #[test]
    fn leading_zeros_are_padding() {
        let buf = hex("00 00 06 AA BB");
        assert_eq!(payloads(&buf, &walk(&buf)), vec![hex("AA BB")]);
        assert_eq!(starts(&buf), (vec![2], 5));
        assert_eq!(starts(&hex("00 00 00")), (vec![], 3));
    }

    #[test]
    fn a_frame_still_arriving_is_left_for_the_next_read() {
        assert_eq!(starts(&hex("07 01 02 03 40")), (vec![0], 4));
    }

    #[test]
    fn an_unreadable_length_steps_when_more_bytes_follow() {
        assert_eq!(starts(&hex("80 00 07 23 36 01")), (vec![2], 6));
        assert_eq!(starts(&hex("80 80 80 80 80 07 23 36 01")), (vec![], 4));
    }

    #[test]
    fn an_unreadable_length_near_the_end_stops() {
        assert_eq!(starts(&hex("07 23 36 01 80 00")), (vec![0], 4));
        assert_eq!(starts(&hex("80 80 80 80")), (vec![], 0));
    }

    #[test]
    fn lengths_below_four_step_one_byte() {
        assert_eq!(starts(&hex("02 07 23 36 01")), (vec![1], 5));
    }

    #[test]
    fn a_length_of_four_is_an_empty_packet() {
        let buf = hex("04");
        let f = walk(&buf);
        assert_eq!(kinds(&f), vec![ChunkForm::Packet]);
        assert_eq!(f.frames[0].len(), 1);
        assert!(f.frames[0].payload(&buf).is_empty());
        assert!(!f.frames[0].is_empty());
        assert_eq!(f.consumed, 1);
    }

    #[test]
    fn a_frame_is_empty_only_with_no_bytes() {
        let none = Frame { kind: ChunkForm::Packet, start: 3, end: 3, varint_width: 0 };
        assert!(none.is_empty());
        assert_eq!(none.len(), 0);
        let some = Frame { kind: ChunkForm::Packet, start: 3, end: 7, varint_width: 1 };
        assert!(!some.is_empty());
        assert_eq!(some.len(), 4);
    }

    #[test]
    fn two_ff_bytes_make_a_bundle_frame() {
        assert_eq!(kinds(&walk(&hex("05 FF"))), vec![ChunkForm::Packet]);
        assert_eq!(kinds(&walk(&hex("06 FF FF"))), vec![ChunkForm::Bundle]);
        let buf = hex("0A FF FF 00 00 00 00");
        let f = walk(&buf);
        assert_eq!(kinds(&f), vec![ChunkForm::Bundle]);
        assert_eq!(f.consumed, 7);
        assert_eq!(inflate_block(f.frames[0].payload(&buf)), None);
    }

    #[test]
    fn long_cut_lengths_wait_only_behind_a_bundle_header() {
        let long = varint(40_000);
        assert_eq!(long.len(), 3);

        let buf = cat(&[&long, &hex("12 34 00 00 00 00"), &frame(&hex("23 36 07"))]);
        assert_eq!(walk(&buf).consumed, 1);

        let buf = cat(&[&long, &hex("FF FF 00 00 01 00")]);
        assert_eq!(walk(&buf).consumed, 0);

        let buf = cat(&[&long, &hex("FF FF FF FF FF 7F")]);
        assert_eq!(walk(&buf).consumed, 1);

        // Only five of the six header bytes have arrived.
        let buf = cat(&[&long, &hex("FF FF 00 00 01")]);
        assert_eq!(walk(&buf).consumed, 1);
    }

    #[test]
    fn cut_plain_frames_wait_up_to_16_kb() {
        let at_limit = frame(&body(0x23, 16_381));
        assert_eq!(at_limit.len(), 16_384);
        assert_eq!(walk(&at_limit[..100]).consumed, 0);

        let over = frame(&body(0x23, 16_382));
        assert_eq!(over.len(), 16_385);
        assert_eq!(walk(&over[..100]).consumed, 1);
    }

    #[test]
    fn a_frame_over_64_kb_is_never_believed() {
        let huge = frame(&body(0x23, 70_000));
        let f = walk(&huge);
        assert!(f.frames.iter().all(|x| x.start > 0));
        assert!(f.consumed > 0);
    }

    #[test]
    fn walking_the_kept_tail_again_takes_nothing() {
        let buf = hex("07 23 36 01 08 41 36");
        let f = walk(&buf);
        assert_eq!(f.consumed, 4);
        assert_eq!(walk(&buf[f.consumed..]).consumed, 0);
    }

    #[test]
    fn the_length_counts_the_payload_plus_four_at_every_width() {
        let lens = [3, 122, 123, 124, 200, 1_000, 16_379, 16_380, 20_000];
        // One digit per payload above.
        let widths = "111222233";
        for (n, digit) in widths.chars().enumerate() {
            let (len, width) = (lens[n], digit.to_digit(10).unwrap() as usize);
            let p = body(0x10 + n as u8, len);
            let buf = cat(&[&frame(&p), &frame(&hex("41 36 07"))]);
            let f = walk(&buf);
            assert_eq!(f.frames.len(), 2, "payload {len}");
            assert_eq!(f.frames[0].payload(&buf), &p[..], "payload {len}");
            assert_eq!(f.frames[0].varint_width, width, "payload {len}");
            assert_eq!(f.frames[1].payload(&buf), &hex("41 36 07")[..]);
            assert_eq!(f.consumed, buf.len());
        }
    }

    // Bundles.

    #[test]
    fn a_bundle_frame_opens_to_its_packets() {
        let first = [frame(&body(0x21, 5)), frame(&body(0x22, 300)), frame(&body(0x23, 40))];
        let mut plain = first.concat();
        for round in [false, true] {
            if round {
                for k in 0..400u32 {
                    plain.extend(frame(&body(0x30 + (k % 7) as u8, 3 + (k % 5) as usize)));
                }
            }
            let buf = cat(&[&bundle(&plain), &frame(&hex("41 36 09"))]);
            let f = walk(&buf);
            assert_eq!(kinds(&f), vec![ChunkForm::Bundle, ChunkForm::Packet]);
            assert_eq!(f.consumed, buf.len());
            let opened = inflate_block(f.frames[0].payload(&buf)).unwrap();
            assert_eq!(opened, plain);
            let inner = split_frames(&opened);
            assert_eq!(inner.consumed, opened.len());
            let got = payloads(&opened, &inner);
            assert_eq!(got[..3], [body(0x21, 5), body(0x22, 300), body(0x23, 40)]);
        }
    }

    #[test]
    fn a_bundle_in_a_bundle_walks_as_a_bundle_frame() {
        let nested = bundle(&frame(&body(0x24, 200)));
        let contents = cat(&[&frame(&body(0x25, 150)), &nested, &frame(&body(0x26, 4))]);
        let f = split_frames(&contents);
        assert_eq!(f.consumed, contents.len());
        assert_eq!(kinds(&f), vec![ChunkForm::Packet, ChunkForm::Bundle, ChunkForm::Packet]);
    }

    #[test]
    fn a_bundle_over_16_kb_is_held_across_reads() {
        let mut seed = 7;
        let packets: Vec<u8> = (0..12).flat_map(|_| frame(&noise(1_600, &mut seed))).collect();
        let big = bundle(&packets);
        assert!(big.len() > 16_384);
        let stream = cat(&[&frame(&hex("23 36 01")), &big, &frame(&hex("41 36 02"))]);

        let f = walk(&stream[..9_000]);
        assert_eq!(f.frames.len(), 1);
        assert_eq!(f.consumed, 4);

        let rest = &stream[4..];
        let f = walk(rest);
        assert_eq!(kinds(&f), vec![ChunkForm::Bundle, ChunkForm::Packet]);
        assert_eq!(inflate_block(f.frames[0].payload(rest)).unwrap(), packets);
    }

    #[test]
    fn any_cut_of_a_mixed_stream_gives_the_same_frames() {
        let parts = [
            frame(&body(0x11, 30)),
            frame(&body(0x12, 126)),
            hex("00 00"),
            bundle(&frame(&body(0x13, 250))),
            frame(&body(0x14, 700)),
            frame(&body(0x15, 3)),
            frame(&body(0x16, 16_380)),
            frame(&body(0x17, 124)),
        ];
        let stream = parts.concat();
        let whole = walk(&stream);
        assert_eq!(whole.consumed, stream.len());
        let expected = payloads(&stream, &whole);
        assert_eq!(expected.len(), 7);
        assert_eq!(expected[0], body(0x11, 30));
        assert_eq!(&expected[2][..2], &[0xFF, 0xFF]);
        assert_eq!(expected[6], body(0x17, 124));

        for piece in [1, 7, 100, 1_460] {
            let mut held: Vec<u8> = Vec::new();
            let mut got = Vec::new();
            for chunk in stream.chunks(piece) {
                held.extend_from_slice(chunk);
                let f = walk(&held);
                got.extend(payloads(&held, &f));
                held.drain(..f.consumed);
            }
            assert_eq!(got, expected, "pieces of {piece}");
            assert!(held.is_empty(), "pieces of {piece}");
        }
    }

    // Inner walk.

    #[test]
    fn inner_walk_skips_padding() {
        assert_eq!(inner_starts(&hex("00 07 23 36 01")), (vec![1], 5));
    }

    #[test]
    fn inner_walk_stops_at_a_bad_length() {
        assert_eq!(inner_starts(&hex("07 23 36 01 80 00 07 23 36 01")), (vec![0], 4));
    }

    #[test]
    fn inner_walk_steps_past_a_short_length() {
        assert_eq!(inner_starts(&hex("02 07 23 36 01")), (vec![1], 5));
    }

    #[test]
    fn inner_walk_stops_at_a_cut_frame() {
        assert_eq!(inner_starts(&hex("07 23 36 01 07 23")), (vec![0], 4));
    }

    #[test]
    fn one_ff_byte_is_still_a_packet_inside() {
        assert_eq!(kinds(&split_frames(&hex("07 FF 36 01"))), vec![ChunkForm::Packet]);
    }

    // Opening a bundle.

    #[test]
    fn opening_a_bundle_checks_its_size() {
        let block = hex("40 07 23 36 01");
        let open = |head: &str| inflate_block(&cat(&[&hex(head), &block]));
        assert_eq!(open("FF FF 04 00 00 00"), Some(hex("07 23 36 01")));
        assert_eq!(open("FF FF 64 00 00 00"), Some(hex("07 23 36 01")));
        assert_eq!(open("FF FF 03 00 00 00"), None);
        assert_eq!(open("FF FF 00 00 00 00"), None);
        assert_eq!(open("FF FF 41 42 0F 00"), None);
        assert_eq!(inflate_block(&hex("FF FF 04 00 00 00")), None);
    }

    // Bundles inside packets.

    fn tiny_bundle() -> Vec<u8> {
        hex("0F FF FF 04 00 00 00 40 07 23 36 01")
    }

    #[test]
    fn a_bundle_inside_a_packet_is_found() {
        let b = tiny_bundle();
        let pkt = frame(&cat(&[&hex("41 36 01"), &b, &hex("AA")]));
        let got = embedded_bundles(&pkt);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].start, got[0].end), (4, 16));
        assert_eq!(got[0].data, hex("07 23 36 01"));

        let pkt = frame(&cat(&[&hex("41 36 01"), &b]));
        assert_eq!(embedded_bundles(&pkt).len(), 1);

        let pkt = frame(&cat(&[&hex("41"), &b, &hex("AA")]));
        let got = embedded_bundles(&pkt);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].start, 2);
    }

    #[test]
    fn bundles_side_by_side_in_a_packet_are_both_found() {
        let b = tiny_bundle();
        let pkt = frame(&cat(&[&hex("41 36"), &b, &b, &hex("AA")]));
        let got = embedded_bundles(&pkt);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].start, got[0].end);
        assert_eq!(got[1].data, hex("07 23 36 01"));
    }

    #[test]
    fn a_bundle_in_a_packets_last_bytes_is_too_short_to_look_for() {
        let eight = hex("FF FF 01 00 00 00 10 AA");
        assert_eq!(inflate_block(&eight), Some(hex("AA")));
        let pkt = frame(&cat(&[&hex("41 36 0C"), &eight]));
        assert!(embedded_bundles(&pkt).is_empty());

        let seven = hex("FF FF 01 00 00 00 00");
        let pkt = frame(&cat(&[&hex("41 36 0B"), &seven]));
        assert!(embedded_bundles(&pkt).is_empty());
    }

    #[test]
    fn a_two_byte_length_in_front_is_read() {
        let mut seed = 3;
        let inner = frame(&noise(200, &mut seed));
        let b = bundle(&inner);
        let pkt = frame(&cat(&[&hex("41 36 05"), &b, &hex("AA BB")]));
        let got = embedded_bundles(&pkt);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, inner);
        assert_eq!(&pkt[got[0].start..got[0].end], &b[..]);
    }
}
