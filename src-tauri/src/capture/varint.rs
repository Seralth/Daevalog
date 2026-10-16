//! Varint and byte readers shared by the parser, the framing and the Evidence Slice.

/// A varint read from a buffer: its value and how many bytes it took.
/// A failed read has both at -1.
#[derive(Debug, Clone, Copy)]
pub struct VarintRead {
    pub value: i32,
    pub length: i32,
}

/// The game never writes a varint longer than this.
const WIDEST: usize = 5;

/// Reads the varint at `bytes[offset]`: 7 bits a byte, low group first, a set
/// high bit meaning one more byte. Only the low 32 bits of the value are kept,
/// so a 5-byte varint can come out negative.
pub fn varint_at(bytes: &[u8], offset: usize) -> VarintRead {
    let tail = bytes.get(offset..).unwrap_or(&[]);
    let mut sum: u64 = 0;
    for (n, &b) in tail.iter().take(WIDEST).enumerate() {
        sum |= u64::from(b & 0x7F) << (7 * n);
        if b & 0x80 == 0 {
            return VarintRead { value: sum as u32 as i32, length: n as i32 + 1 };
        }
    }
    VarintRead { value: -1, length: -1 }
}

/// Whether `varint_at(bytes, offset)` would succeed.
pub fn varint_fits(bytes: &[u8], offset: usize) -> bool {
    varint_at(bytes, offset).length > 0
}

/// Reads a varint and moves `offset` past it. A failed read leaves `offset`
/// alone; a negative value still moves it but gives `None`.
pub fn next_varint(bytes: &[u8], offset: &mut usize) -> Option<i32> {
    match varint_at(bytes, *offset) {
        VarintRead { length, .. } if length < 1 => None,
        VarintRead { value, length } => {
            *offset += length as usize;
            u32::try_from(value).ok().map(|_| value)
        }
    }
}

/// The value of a varint whose last byte is `data[end - 1]`, for a field read
/// backwards from what follows it. Tries 1, 2 and 3 bytes. A reading counts
/// when it starts at or after `min_start`, is exactly that wide and its value
/// lies in `range`. The first counting reading with a clean byte before it
/// (high bit clear, or the buffer's start) wins; when every one sits behind a
/// continuation byte, the shortest is taken.
pub fn varint_before(
    data: &[u8],
    end: usize,
    min_start: usize,
    range: std::ops::RangeInclusive<i32>,
) -> Option<i32> {
    // (value, whether a clean byte or the buffer's start sits in front)
    let readings = (1..=3usize).filter_map(|width| {
        let from = end.checked_sub(width).filter(|&f| f >= min_start)?;
        let got = varint_at(data, from);
        let fits = got.length == width as i32 && range.contains(&got.value);
        fits.then(|| (got.value, from == 0 || data[from - 1] < 0x80))
    });
    let mut shortest = None;
    for (value, clean) in readings {
        if clean {
            return Some(value);
        }
        shortest.get_or_insert(value);
    }
    shortest
}

/// The u32 stored little-endian at `data[offset..offset + 4]`. Panics when
/// those bytes are not all there.
pub fn le_u32_at(data: &[u8], offset: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&data[offset..offset + 4]);
    u32::from_le_bytes(word)
}

/// The u16 stored little-endian at `data[at..at + 2]`, or `None` when those
/// bytes are not all there.
pub fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// The u32 stored little-endian at `data[at..at + 4]`, or `None` when those
/// bytes are not all there.
pub fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The u64 stored little-endian at `data[at..at + 8]`, or `None` when those
/// bytes are not all there.
pub fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// Where `pattern` first sits at or after `start`.
pub fn index_of_bytes(data: &[u8], start: usize, pattern: &[u8]) -> Option<usize> {
    let last = data.len().checked_sub(pattern.len())?;
    if start > last {
        return None;
    }
    (start..=last).find(|&at| data[at..].starts_with(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(bytes: &[u8], offset: usize) -> (i32, i32) {
        let got = varint_at(bytes, offset);
        (got.value, got.length)
    }

    #[test]
    fn varints_of_every_width_read_low_group_first() {
        assert_eq!(pair(&[0x07], 0), (7, 1));
        assert_eq!(pair(&[0xAC, 0x02], 0), (300, 2));
        assert_eq!(pair(&[0x80, 0x80, 0x01], 0), (16384, 3));
        assert_eq!(pair(&[0x80, 0x00], 0), (0, 2));
        assert_eq!(pair(&[0xFF, 0xFF, 0xFF, 0xFF, 0x07], 0), (i32::MAX, 5));
        assert_eq!(pair(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F], 0), (-1, 5));
        assert_eq!(pair(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F], 0), (-1, 5));
        assert_eq!(pair(&[0x55, 0xAC, 0x02], 1), (300, 2));
    }

    #[test]
    fn a_varint_that_never_ends_fails_whole() {
        assert_eq!(pair(&[0x80], 0), (-1, -1));
        assert_eq!(pair(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 0), (-1, -1));
        assert_eq!(pair(&[], 0), (-1, -1));
        assert_eq!(pair(&[0x07], 1), (-1, -1));
        assert_eq!(pair(&[0x07], 9), (-1, -1));
    }

    #[test]
    fn can_read_matches_read() {
        assert!(varint_fits(&[0x07], 0));
        assert!(!varint_fits(&[0x80], 0));
        assert!(!varint_fits(&[0x07], 1));
        assert!(!varint_fits(&[0x07], 4));
        assert!(!varint_fits(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01], 0));
    }

    #[test]
    fn try_read_moves_past_what_it_read() {
        let mut at = 0;
        assert_eq!(next_varint(&[0xAC, 0x02], &mut at), Some(300));
        assert_eq!(at, 2);
        let mut at = 0;
        assert_eq!(next_varint(&[0x80], &mut at), None);
        assert_eq!(at, 0);
        let mut at = 0;
        assert_eq!(next_varint(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F], &mut at), None);
        assert_eq!(at, 5);
        let mut at = 0;
        assert_eq!(next_varint(&[0x00], &mut at), Some(0));
        assert_eq!(at, 1);
    }

    #[test]
    fn a_varint_read_backwards_prefers_a_clean_start() {
        let back = |data: &[u8], end, from| varint_before(data, end, from, 100..=99_999).unwrap_or(-1);
        let cases: [(&[u8], usize, usize, i32); 7] = [
            (&[0x01, 0x9A, 0x6D, 0xE2, 0x07], 3, 0, 13978),
            (&[0x01, 0x98, 0x75, 0x18, 0x05], 3, 0, 15000),
            (&[0x01, 0x6D, 0xE2, 0x07], 2, 0, 109),
            (&[0x01, 0xBD, 0x44, 0xE2, 0x07], 3, 0, 8765),
            (&[0x81, 0x9A, 0x6D], 3, 0, 109),
            (&[0x01, 0x9A, 0x6D], 3, 2, 109),
            (&[0x01, 0x9A, 0x6D], 0, 0, -1),
        ];
        for (data, end, from, want) in cases {
            assert_eq!(back(data, end, from), want, "{data:02X?} ending at {end}");
        }
    }

    #[test]
    fn u32_little_endian() {
        assert_eq!(le_u32_at(&[0x01, 0x02, 0x03, 0x04], 0), 0x0403_0201);
        assert_eq!(le_u32_at(&[0x09, 0x01, 0x02, 0x03, 0x04], 1), 0x0403_0201);
    }

    #[test]
    fn checked_reads_need_every_byte() {
        let data = [0x09, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(u16_at(&data, 1), Some(0x0201));
        assert_eq!(u32_at(&data, 1), Some(0x0403_0201));
        assert_eq!(u64_at(&data, 1), Some(0x0807_0605_0403_0201));
        assert_eq!((u16_at(&data, 8), u32_at(&data, 6), u64_at(&data, 2)), (None, None, None));
        assert_eq!(u16_at(&data, 99), None);
    }

    #[test]
    fn pattern_search_from_a_start() {
        let data = [0x01, 0x02, 0x03, 0x02, 0x03];
        assert_eq!(index_of_bytes(&data, 0, &[0x02, 0x03]), Some(1));
        assert_eq!(index_of_bytes(&data, 2, &[0x02, 0x03]), Some(3));
        assert_eq!(index_of_bytes(&data, 4, &[0x02, 0x03]), None);
        assert_eq!(index_of_bytes(&data, 2, &[]), Some(2));
        assert_eq!(index_of_bytes(&data, 9, &[]), None);
        assert_eq!(index_of_bytes(&[0x02], 0, &[0x02, 0x03]), None);
    }
}
