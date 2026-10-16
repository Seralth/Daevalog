//! Hotkey labels as `settings.json` keeps them ("Ctrl+Alt+R"), in Win32's
//! numbers, and the trigger text the desktop's GlobalShortcuts portal takes.

const ALT: u32 = 0x0001;
const CTRL: u32 = 0x0002;
const SHIFT: u32 = 0x0004;
const WIN: u32 = 0x0008;
/// F1's virtual-key code; F2..F12 follow it.
const F1: u32 = 0x70;

/// Win32 virtual-key codes with a name, and the xkb keysym name of each.
/// Letters, digits and F1..F12 are worked out instead.
const NAMED_KEYS: &[(&[&str], u32, &str)] = &[
    (&["BACKSPACE"], 0x08, "BackSpace"),
    (&["TAB"], 0x09, "Tab"),
    (&["ENTER", "RETURN"], 0x0D, "Return"),
    (&["ESC", "ESCAPE"], 0x1B, "Escape"),
    (&["SPACE"], 0x20, "space"),
    (&["PAGEUP"], 0x21, "Page_Up"),
    (&["PAGEDOWN"], 0x22, "Page_Down"),
    (&["END"], 0x23, "End"),
    (&["HOME"], 0x24, "Home"),
    (&["LEFT"], 0x25, "Left"),
    (&["UP"], 0x26, "Up"),
    (&["RIGHT"], 0x27, "Right"),
    (&["DOWN"], 0x28, "Down"),
    (&["INSERT"], 0x2D, "Insert"),
    (&["DELETE"], 0x2E, "Delete"),
];

fn modifier_bit(part: &str) -> Option<u32> {
    let is = |name: &str| part.eq_ignore_ascii_case(name);
    if is("ctrl") || is("control") {
        Some(CTRL)
    } else if is("alt") {
        Some(ALT)
    } else if is("shift") {
        Some(SHIFT)
    } else if is("win") || is("super") {
        Some(WIN)
    } else {
        None
    }
}

fn key_code(part: &str) -> Option<u32> {
    let upper = part.to_uppercase();
    if let Some(&(_, code, _)) = NAMED_KEYS.iter().find(|(names, _, _)| names.contains(&upper.as_str())) {
        return Some(code);
    }
    if let Some(n) = (0..12u32).find(|n| upper == format!("F{}", n + 1)) {
        return Some(F1 + n);
    }
    match upper.as_bytes() {
        [b] if b.is_ascii_alphanumeric() => Some(*b as u32),
        _ => None,
    }
}

/// The modifier flags and virtual-key code of a label like "Ctrl+Alt+R".
/// `None` unless it has a key we know and at least one modifier.
pub fn parse_hotkey_label(label: &str) -> Option<(u32, u32)> {
    // An empty label is one empty part, which is no key.
    let (mut mods, mut key) = (0u32, 0u32);
    for part in label.trim().split('+') {
        let part = part.trim();
        if let Some(bit) = modifier_bit(part) {
            mods |= bit;
            continue;
        }
        key = key_code(part)?;
    }
    (mods != 0 && key != 0).then_some((mods, key))
}

/// The portal's trigger text for a hotkey, e.g. "CTRL+ALT+l". `None` for a
/// key the portal has no name for, or with no modifier.
pub fn portal_trigger(mods: u32, vk: u32) -> Option<String> {
    let key = match vk {
        0x30..=0x39 => char::from(vk as u8).to_string(),
        0x41..=0x5A => char::from(vk as u8).to_ascii_lowercase().to_string(),
        F1..=0x7B => format!("F{}", vk - F1 + 1),
        _ => NAMED_KEYS.iter().find(|&&(_, code, _)| code == vk)?.2.to_string(),
    };
    let mut parts: Vec<String> = [(CTRL, "CTRL"), (ALT, "ALT"), (SHIFT, "SHIFT"), (WIN, "LOGO")]
        .into_iter()
        .filter(|&(bit, _)| mods & bit != 0)
        .map(|(_, name)| name.to_string())
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.push(key);
    Some(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_label(label: &str) -> Option<String> {
        let (mods, vk) = parse_hotkey_label(label)?;
        portal_trigger(mods, vk)
    }

    #[test]
    fn saved_hotkeys_become_portal_triggers() {
        assert_eq!(from_label("Ctrl+Alt+L").as_deref(), Some("CTRL+ALT+l"));
        assert_eq!(from_label("Shift+Win+F5").as_deref(), Some("SHIFT+LOGO+F5"));
        assert_eq!(from_label("Alt+Ctrl+PageUp").as_deref(), Some("CTRL+ALT+Page_Up"));
        assert_eq!(from_label("Ctrl+7").as_deref(), Some("CTRL+7"));
        assert_eq!(from_label("Ctrl+Space").as_deref(), Some("CTRL+space"));
        assert_eq!(from_label("Ctrl+F12").as_deref(), Some("CTRL+F12"));
    }

    #[test]
    fn unknown_keys_and_bare_keys_have_no_trigger() {
        assert_eq!(portal_trigger(0x0002, 0xBA), None);
        assert_eq!(portal_trigger(0, 0x4C), None);
    }

    #[test]
    fn labels_give_win32_flags_and_codes() {
        let cases = [
            ("Ctrl+Alt+R", (0x0003, 0x52)),
            (" control + shift + f5 ", (0x0006, 0x74)),
            ("Win+l", (0x0008, 0x4C)),
            ("Super+7", (0x0008, 0x37)),
            ("Ctrl+Esc", (0x0002, 0x1B)),
            ("Ctrl+A+B", (0x0002, 0x42)),
            ("Ctrl+Ctrl+X", (0x0002, 0x58)),
            ("Ctrl+Alt+Up", (0x0003, 0x26)),
            ("Ctrl+Backspace", (0x0002, 0x08)),
            ("Ctrl+Return", (0x0002, 0x0D)),
            ("Ctrl+Delete", (0x0002, 0x2E)),
            ("Ctrl+F1", (0x0002, 0x70)),
        ];
        for (label, want) in cases {
            assert_eq!(parse_hotkey_label(label), Some(want), "{label}");
        }
    }

    #[test]
    fn labels_without_a_known_key_or_modifier_are_refused() {
        let refused: [&str; 12] =
            ["", "  ", "R", "Ctrl+Alt", "Ctrl+F13", "Ctrl+F0", "Ctrl+F01", "Ctrl+;", "Ctrl++", "Ctrl+ß", "Ctrl+é", "Ctrl+Numpad1"];
        let accepted: Vec<&str> = refused.into_iter().filter(|l| parse_hotkey_label(l).is_some()).collect();
        assert!(accepted.is_empty(), "{accepted:?}");
    }

    #[test]
    fn triggers_list_modifiers_in_portal_order() {
        let table: [(u32, u32, Option<&str>); 5] = [
            (0x0003, 0x4C, Some("CTRL+ALT+l")),
            (0x000F, 0x2E, Some("CTRL+ALT+SHIFT+LOGO+Delete")),
            (0x0010, 0x41, None),
            (0x0002, 0x70, Some("CTRL+F1")),
            (0x0012, 0x0D, Some("CTRL+Return")),
        ];
        for (mods, vk, text) in table {
            assert_eq!(portal_trigger(mods, vk).as_deref(), text, "{mods:#x} {vk:#x}");
        }
    }
}
