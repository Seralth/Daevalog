//! Joins one stream's reads and hands whole frames to the parser.

use super::{byte_queue::ByteQueue, decoder::PacketDecoder};

#[derive(Debug, Default)]
pub struct Reassembler {
    pending: ByteQueue,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one read and parses every frame now whole. True when the parser
    /// took any bytes, padding and resync steps included: the dispatcher
    /// counts that as the stream being alive.
    pub fn push_bytes(&mut self, data: &[u8], processor: &mut PacketDecoder) -> bool {
        self.pending.append(data);
        let mut rounds = 0;
        loop {
            let used = match self.pending.size() {
                0 => 0,
                _ => processor.decode_buffer(self.pending.snapshot()),
            };
            if used == 0 {
                return rounds > 0;
            }
            self.pending.drop_front(used);
            rounds += 1;
        }
    }

    pub fn pending_len(&self) -> usize {
        self.pending.size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{bundle, frame};
    use crate::combat::data_storage::DataStorage;
    use crate::i18n::lookup::{NpcLookup, SkillLookup};
    use std::sync::Arc;

    fn parser() -> PacketDecoder {
        let storage = Arc::new(DataStorage::new());
        let skills = Arc::new(SkillLookup::new());
        PacketDecoder::new(storage, skills, Arc::new(NpcLookup::new()))
    }

    #[test]
    fn padding_alone_counts_as_progress() {
        let mut joiner = Reassembler::new();
        assert!(joiner.push_bytes(&[0, 0], &mut parser()));
        assert_eq!(joiner.pending_len(), 0);
    }

    #[test]
    fn a_packet_split_over_two_reads_lands_on_the_second() {
        let mut joiner = Reassembler::new();
        let mut p = parser();
        assert!(!joiner.push_bytes(&[0x07, 0x23], &mut p));
        assert_eq!(joiner.pending_len(), 2);
        assert!(joiner.push_bytes(&[0x36, 0x01], &mut p));
        assert_eq!(joiner.pending_len(), 0);
    }

    #[test]
    fn a_long_bundle_waits_for_its_last_piece() {
        // Packets of noise, so the bundle stays long after LZ4.
        let mut state = 0x2545_F491u32;
        let mut inner = Vec::new();
        while inner.len() < 16_900 {
            let mut body = Vec::with_capacity(1_200);
            for _ in 0..1_200 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                body.push(state.to_be_bytes()[0]);
            }
            inner.extend(frame(&body));
        }
        let stream = bundle(&inner);
        assert!(stream.len() > 16_384, "bundle only {} bytes", stream.len());
        let mut joiner = Reassembler::new();
        let mut p = parser();
        let pieces: Vec<&[u8]> = stream.chunks(1_460).collect();
        let mut held = 0;
        for (n, piece) in pieces.iter().enumerate() {
            let moved = joiner.push_bytes(piece, &mut p);
            if n + 1 < pieces.len() {
                assert!(!moved, "piece {n}");
                assert!(joiner.pending_len() > held);
                held = joiner.pending_len();
            } else {
                assert!(moved);
                assert_eq!(joiner.pending_len(), 0);
            }
        }
    }
}
