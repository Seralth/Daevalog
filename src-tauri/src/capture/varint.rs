//! Varint and byte readers shared by the parser, the framing and the Evidence Slice.

/// VarInt decode result.
#[derive(Debug, Clone, Copy)]
pub struct VarIntResult {
    pub value: i32,
    pub length: i32,
}

impl VarIntResult {
    pub fn invalid() -> Self {
        Self { value: -1, length: -1 }
    }
}

/// The varint that ends just before `end`, starting no earlier than
/// `min_start` and at most three bytes back, whose value is in `range`.
///
/// The last byte of a multi-byte varint has its high bit clear, so it is also
/// a valid one-byte varint on its own: entity 13978 is `9A 6D`, and `6D`
/// alone is 109. Read shortest first, every id from 12,800 up came out as
/// `id >> 7`, which still passes a range check, so names and loot went to the
/// wrong entity (issue #10). A varint cannot start right after a byte with its
/// high bit set, since that byte would continue into it, so the candidate not
/// preceded by one wins; the shortest valid one is only the fallback.
pub fn varint_ending_at(
    data: &[u8],
    end: usize,
    min_start: usize,
    range: std::ops::RangeInclusive<i32>,
) -> Option<i32> {
    let mut fallback = None;
    for v_len in 1..=3usize {
        let Some(v_start) = end.checked_sub(v_len) else { break };
        if v_start < min_start || !can_read_varint(data, v_start) {
            continue;
        }
        let v = read_varint(data, v_start);
        if v.length != v_len as i32 || !range.contains(&v.value) {
            continue;
        }
        let continued = v_start > 0 && data[v_start - 1] & 0x80 != 0;
        if !continued {
            return Some(v.value);
        }
        fallback.get_or_insert(v.value);
    }
    fallback
}

pub fn read_varint(bytes: &[u8], offset: usize) -> VarIntResult {
    let mut value: i32 = 0;
    let mut shift = 0;
    let mut count = 0;

    loop {
        if offset + count >= bytes.len() {
            return VarIntResult::invalid();
        }

        let byte_val = bytes[offset + count] as u32;
        count += 1;

        value |= ((byte_val & 0x7F) as i32) << shift;

        if byte_val & 0x80 == 0 {
            return VarIntResult { value, length: count as i32 };
        }

        shift += 7;
        if shift >= 32 {
            return VarIntResult::invalid();
        }
    }
}

pub fn try_read_varint(bytes: &[u8], offset: &mut usize) -> Option<i32> {
    let result = read_varint(bytes, *offset);
    if result.length <= 0 {
        return None;
    }
    *offset += result.length as usize;
    if result.value < 0 { None } else { Some(result.value) }
}

pub fn can_read_varint(bytes: &[u8], offset: usize) -> bool {
    if offset >= bytes.len() {
        return false;
    }
    let mut idx = offset;
    let mut count = 0;
    while idx < bytes.len() && count < 5 {
        let byte_val = bytes[idx] as u32;
        if byte_val & 0x80 == 0 {
            return true;
        }
        idx += 1;
        count += 1;
    }
    false
}

pub fn parse_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
    ])
}

pub fn find_pattern(data: &[u8], start: usize, pattern: &[u8]) -> Option<usize> {
    if data.len() < pattern.len() + start {
        return None;
    }
    for i in start..=data.len() - pattern.len() {
        if data[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    None
}
