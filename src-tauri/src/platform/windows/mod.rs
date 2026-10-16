//! The Windows port: capture in the meter's own process through Npcap, the
//! game found by its process and window, the sign-in kept in Windows
//! Credential Manager.

pub mod admin;
pub mod clock;
pub mod files;
pub mod pcap;
pub mod process;
pub mod secret;
pub mod window_detector;

/// `text` as UTF-16 with a closing NUL, for the wide Win32 calls.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 up to the first NUL, or all of it when there is none.
fn from_wide(text: &[u16]) -> String {
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_text_round_trips() {
        assert_eq!(wide("ab"), vec![0x61, 0x62, 0]);
        assert_eq!(from_wide(&wide("Aion 2 · 한")), "Aion 2 · 한");
        assert_eq!(from_wide(&[0x61, 0, 0x62]), "a");
        assert_eq!(from_wide(&[0x61, 0x62]), "ab");
    }
}
