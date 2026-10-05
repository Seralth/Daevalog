//! Checks for the character names the packets carry.

/// The name the game gives a character that has not been named yet: `$` then
/// letters and digits.
pub(super) fn is_placeholder_name(raw: &str) -> bool {
    raw.strip_prefix('$')
        .is_some_and(|rest| rest.len() >= 4 && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Byte lengths a length-prefixed name field can have: 1 to 12 characters of
/// up to 4 UTF-8 bytes each.
pub(super) const NAME_FIELD_BYTES: std::ops::RangeInclusive<usize> = 1..=48;

/// A character name read from a field whose length the packet states.
///
/// Names are 1 to 12 characters: letters in any script (Latin with accents,
/// Japanese, Hangul, Han…) and digits, with at least one letter. The whole
/// field must be the name; anything else means we are not on a name field.
/// Unlike `sanitize_nickname`, a one-character name is fine here: the stated
/// length is what guards against picking up junk.
pub(super) fn exact_name(field: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(field).ok()?;
    let chars = name.chars().count();
    let valid = (1..=12).contains(&chars)
        && name.chars().all(char::is_alphanumeric)
        && name.chars().any(char::is_alphabetic);
    valid.then(|| name.to_string())
}

pub(super) fn sanitize_nickname(nickname: &str) -> Option<String> {
    let trimmed = nickname.split('\0').next().unwrap_or("").trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut result = String::new();
    let mut only_numbers = true;
    let mut has_cjk = false;

    for ch in trimmed.chars() {
        if !ch.is_alphanumeric() {
            if result.is_empty() {
                return None;
            }
            break;
        }
        if ch == '\u{FFFD}' {
            if result.is_empty() {
                return None;
            }
            break;
        }
        if ch.is_control() {
            if result.is_empty() {
                return None;
            }
            break;
        }
        result.push(ch);
        if ch.is_alphabetic() {
            only_numbers = false;
        }
        if is_cjk_char(ch) {
            has_cjk = true;
        }
    }

    if result.is_empty() || only_numbers {
        return None;
    }

    if result.chars().count() < 2 && !has_cjk {
        return None;
    }

    Some(result)
}

fn is_cjk_char(ch: char) -> bool {
    let cp = ch as u32;
    // CJK Unified Ideographs
    (0x4E00..=0x9FFF).contains(&cp)
    // Hangul Syllables
    || (0xAC00..=0xD7AF).contains(&cp)
    // CJK Extension A/B
    || (0x3400..=0x4DBF).contains(&cp)
    || (0x20000..=0x2A6DF).contains(&cp)
    // Hangul Jamo
    || (0x1100..=0x11FF).contains(&cp)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum UnicodeScript {
    Han,
    Hangul,
    Other,
}

pub(super) fn unicode_script(ch: char) -> UnicodeScript {
    let cp = ch as u32;
    if (0x4E00..=0x9FFF).contains(&cp) || (0x3400..=0x4DBF).contains(&cp) || (0x20000..=0x2A6DF).contains(&cp) {
        UnicodeScript::Han
    } else if (0xAC00..=0xD7AF).contains(&cp) || (0x1100..=0x11FF).contains(&cp) {
        UnicodeScript::Hangul
    } else {
        UnicodeScript::Other
    }
}
