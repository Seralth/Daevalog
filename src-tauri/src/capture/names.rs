//! Checks for the character names the packets carry.

/// How many bytes a name field may hold: a name has at most 12 characters,
/// and UTF-8 spends at most 4 bytes on one.
pub(crate) const NAME_LEN_RANGE: std::ops::RangeInclusive<usize> = 1..=48;

/// The longest name the game allows, in characters.
const LONGEST_NAME: usize = 12;

/// Whether `c` may stand in a name: a letter or a digit, in any script.
pub(super) fn name_char(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

/// Han ideographs and Hangul, where one sign is a whole name. Kana and the
/// compatibility blocks are left out.
pub(crate) fn in_cjk_blocks(c: char) -> bool {
    matches!(u32::from(c), 0x1100..=0x11FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0x20000..=0x2A6DF)
}

/// The name a field holds when the field is nothing but that name: 1 to 12
/// letters or digits, at least one of them a letter.
pub(crate) fn clean_name_field(field: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(field).ok()?;
    let (mut seen, mut letters) = (0, 0);
    for c in text.chars() {
        if !name_char(c) {
            return None;
        }
        seen += 1;
        letters += usize::from(c.is_alphabetic());
    }
    (seen <= LONGEST_NAME && letters > 0).then(|| text.to_owned())
}

/// What the game calls a new character before the player names it: `$`
/// and four or more ASCII letters and digits.
pub(super) fn tutorial_placeholder(field: &[u8]) -> bool {
    matches!(field, [b'$', tail @ ..] if tail.len() >= 4 && tail.iter().all(u8::is_ascii_alphanumeric))
}

/// A forgiving read for the scans that meet names among other bytes: the text
/// up to a NUL, white space trimmed, and then its leading run of letters and
/// digits. No upper length: replays depend on that. A run of digits alone is
/// no name, and neither is one character outside the CJK blocks.
fn loose_name(text: &str) -> Option<String> {
    let text = text.split('\0').next().unwrap_or_default().trim();
    let run = &text[..text.find(|c: char| !name_char(c)).unwrap_or(text.len())];
    let mut chars = run.chars();
    let first = chars.next()?;
    let lone = chars.next().is_none();
    if !run.chars().any(char::is_alphabetic) || (lone && !in_cjk_blocks(first)) {
        return None;
    }
    Some(run.to_owned())
}

/// The name `loose_name` reads from `packet[start..start + len]`, and
/// where its bytes sit: the field's first run of letters and digits.
pub(super) fn sanitized_at(packet: &[u8], start: usize, len: usize) -> Option<(String, std::ops::Range<usize>)> {
    let field = std::str::from_utf8(packet.get(start..start.checked_add(len)?)?).ok()?;
    let name = loose_name(field)?;
    let at = start + field.len() - field.trim_start().len();
    let end = at + name.len();
    Some((name, at..end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_of_every_script_make_names() {
        for c in ['é', 'ぁ', '丂', 'Ñ', 'Ъ', 'q'] {
            assert!(c.is_alphabetic() && name_char(c), "{c}");
        }
        // Arabic-Indic three and one half.
        for c in ['\u{663}', '½', '7'] {
            assert!(!c.is_alphabetic() && name_char(c), "{c}");
        }
        for c in ['_', ' ', '\u{301}', '\u{FFFD}', '$', '\0'] {
            assert!(!name_char(c), "{:04x}", u32::from(c));
        }
    }

    #[test]
    fn the_cjk_blocks_are_han_and_hangul_only() {
        for c in ['丂', '\u{3400}', '\u{20000}', '\u{1100}', '갃'] {
            assert!(in_cjk_blocks(c), "{:04x}", u32::from(c));
        }
        // Kana, a compatibility jamo, a compatibility ideograph, and the
        // first sign past the extension B block.
        for c in ['ぁ', '\u{3131}', '\u{F900}', '\u{2A700}', 'q', 'Ъ'] {
            assert!(!in_cjk_blocks(c), "{:04x}", u32::from(c));
        }
    }

    #[test]
    fn an_exact_name_is_the_whole_field() {
        let good = ["A", "é", "ぁ", "Qxqz", "Qxqz1", "1Q", "Ñqxz", "ぁぃぅ", "丂丂", "갃갃", "Ъщъщ", "Qxqzqxqzqxqz", "Qq½"];
        for name in good {
            assert_eq!(clean_name_field(name.as_bytes()).as_deref(), Some(name));
        }
        let bad: [&[u8]; 9] = [
            b"Qxqzqxqzqxqzq",
            b"12345",
            b"Qx qz",
            b"Qxqz\x06",
            b"\x05Qxqz",
            b"",
            &[0xC3],
            "Qxe\u{301}".as_bytes(),
            b"$Qx9z",
        ];
        for field in bad {
            assert_eq!(clean_name_field(field), None, "{field:02x?}");
        }
    }

    #[test]
    fn a_name_field_holds_twelve_wide_characters() {
        let widest = "\u{20000}".repeat(LONGEST_NAME);
        assert!(NAME_LEN_RANGE.contains(&widest.len()));
        assert!(clean_name_field(widest.as_bytes()).is_some());
    }

    #[test]
    fn a_placeholder_is_a_dollar_and_four_or_more_ascii_signs() {
        for text in ["$Qx9Zz0Aa0q1", "$Qx9Z"] {
            assert!(tutorial_placeholder(text.as_bytes()), "{text}");
        }
        for text in ["$Qx9", "$Qx", "$", "$Qx_9", "$Qxé9", "Qx9Zz", ""] {
            assert!(!tutorial_placeholder(text.as_bytes()), "{text}");
        }
    }

    #[test]
    fn the_loose_reader_takes_the_leading_run() {
        let cases: [(&str, Option<&str>); 19] = [
            ("Qxqz", Some("Qxqz")),
            ("  Qxqz ", Some("Qxqz")),
            ("Qxqz\0Zz", Some("Qxqz")),
            ("\0Qxqz", None),
            ("Qx-qz", Some("Qx")),
            ("Qx qz", Some("Qx")),
            ("-Qxqz", None),
            ("12", None),
            ("1Q", Some("1Q")),
            ("Q", None),
            ("丂", Some("丂")),
            ("갃", Some("갃")),
            ("\u{1100}", Some("\u{1100}")),
            ("ぁ", None),
            ("ぁぁ", Some("ぁぁ")),
            ("e\u{301}x", None),
            ("", None),
            ("   ", None),
            ("Ъщъщ", Some("Ъщъщ")),
        ];
        for (text, want) in cases {
            assert_eq!(loose_name(text).as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn the_loose_reader_has_no_upper_length() {
        let long = "Qxqz".repeat(5);
        assert_eq!(loose_name(&long), Some(long.clone()));
    }

    #[test]
    fn the_loose_reader_stops_at_controls_and_replacement_signs() {
        assert_eq!(loose_name("Qx\u{5}").as_deref(), Some("Qx"));
        assert_eq!(loose_name("Qxq\u{5}z").as_deref(), Some("Qxq"));
        assert_eq!(loose_name("\u{5}Qx"), None);
        assert_eq!(loose_name("Qx\u{FFFD}").as_deref(), Some("Qx"));
        assert_eq!(loose_name("\u{FFFD}Qx"), None);
    }

    #[test]
    fn sanitized_at_gives_where_the_name_itself_sits() {
        let packet = [0x20, 0x51, 0x78, 0x71, 0x7A];
        assert_eq!(sanitized_at(&packet, 0, 5), Some(("Qxqz".to_owned(), 1..5)));
        assert_eq!(sanitized_at(&packet, 0, 6), None);
        assert_eq!(sanitized_at(&packet, 2, 3), Some(("xqz".to_owned(), 2..5)));
    }
}
