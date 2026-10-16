//! The bug-report copy of debug.log, for a public issue.
//!
//! debug.log stays as it is, for local debugging. It holds the names of the
//! players the meter met and the home folder, which shows the user name. The
//! copy swaps every character name for a token (`<name1>`, the same token
//! for the same name all through the file), the home folder for `~`, and
//! the user name for `<user>`.
//!
//! Names come from the meter (what it resolved) and from the log itself: as
//! the slice blinder does with known names and name-shaped text, anything
//! quoted that has a name's shape is a name, and so is the roster line's.
//! Tokens are numbered, not made from the name, so a list of names cannot
//! tell whose a token is.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::report::PreparedLog;
use crate::capture::names::clean_name_field;
use crate::platform;

/// The copy's file name, in the data folder.
pub const REPORT_NAME: &str = "report_debug.log";

/// Stands in for the user name until the end, so the name pass never reads
/// a part of it.
const USER_MARK: char = '\u{E000}';

#[derive(Debug, PartialEq)]
pub struct Redacted {
    pub text: String,
    /// Names found and swapped.
    pub names: usize,
}

/// Every name-shaped word between quotes (`'x'` or `"x"`), and the roster
/// line's unquoted one.
fn quoted_names(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in text.lines() {
        for quote in ['\'', '"'] {
            let mut rest = line;
            // From each quote mark on, so an apostrophe ("player's") does
            // not pair up with the quote of a name after it.
            while let Some(open) = rest.find(quote) {
                let before = rest[..open].chars().next_back();
                let after = &rest[open + 1..];
                rest = after;
                let Some(close) = after.find(quote) else { break };
                let next = after[close + 1..].chars().next();
                let apart = |c: Option<char>| !c.is_some_and(char::is_alphanumeric);
                if apart(before) && apart(next) && let Some(name) = clean_name_field(after[..close].as_bytes()) {
                    out.insert(name);
                    rest = &after[close + 1..];
                }
            }
        }
        if let Some((_, tail)) = line.split_once("Roster: ")
            && let Some((name, _)) = tail.split_once(" is entity")
            && let Some(name) = clean_name_field(name.as_bytes())
        {
            out.insert(name);
        }
    }
    out
}

/// Every place `needle` is in `text`, ASCII case ignored, swapped for `with`.
fn replace_ignoring_case(text: &str, needle: &str, with: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(at) = lower[from..].find(&needle) {
        out.push_str(&text[from..from + at]);
        out.push_str(with);
        from += at + needle.len();
    }
    out.push_str(&text[from..]);
    out
}

/// `text` with the names in `known` and in the text, the home folder `home`
/// and the user names `users` taken out.
pub fn redact(text: &str, known: &[String], home: Option<&str>, users: &[String]) -> Redacted {
    let mut text = text.to_string();
    if let Some(home) = home.map(|h| h.trim_end_matches('/')).filter(|h| !h.is_empty()) {
        text = text.replace(home, "~");
    }
    // A long user name wherever it is (a folder "name-old" too); a short one
    // as a word of its own, or every "me" in English would go.
    let mut short_users = Vec::new();
    for user in users.iter().filter(|u| !u.is_empty()) {
        if user.chars().count() >= 4 {
            text = replace_ignoring_case(&text, user, &USER_MARK.to_string());
        } else {
            short_users.push(user.to_ascii_lowercase());
        }
    }

    let mut names: HashSet<String> = quoted_names(&text);
    names.extend(known.iter().filter_map(|n| clean_name_field(n.as_bytes())));
    let mut tokens: HashMap<String, String> = HashMap::new();
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String, tokens: &mut HashMap<String, String>| {
        if word.is_empty() {
            return;
        }
        if let Some(name) = names.get(word.as_str()) {
            let next = tokens.len() + 1;
            out.push_str(tokens.entry(name.clone()).or_insert_with(|| format!("<name{next}>")));
        } else if short_users.iter().any(|u| word.eq_ignore_ascii_case(u)) {
            out.push(USER_MARK);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out, &mut tokens);
            out.push(c);
        }
    }
    flush(&mut word, &mut out, &mut tokens);
    let names = tokens.len();
    Redacted { text: out.replace(USER_MARK, "<user>"), names }
}

/// The variables that hold the session's user name: USERNAME on Windows.
#[cfg(not(windows))]
const USER_VARS: [&str; 2] = ["USER", "LOGNAME"];
#[cfg(windows)]
const USER_VARS: [&str; 1] = ["USERNAME"];

/// The user names to take out: the session's, and the home folder's last part.
fn user_names(home: Option<&Path>) -> Vec<String> {
    let mut users: Vec<String> = USER_VARS.iter().filter_map(|v| std::env::var(v).ok()).collect();
    users.extend(home.and_then(|h| h.file_name()).and_then(|n| n.to_str()).map(str::to_string));
    users.retain(|u| !u.is_empty());
    users.sort();
    users.dedup();
    users
}

/// Write `report_debug.log`: debug.log in `data_dir` with the names in
/// `known` (what the meter resolved), any other name, the home folder and
/// the user name taken out.
pub fn prepare_debug_log(data_dir: &Path, known: &[String]) -> Result<PreparedLog, String> {
    let source = data_dir.join("debug.log");
    let raw = std::fs::read(&source).map_err(|_| {
        "There is no debug.log. Turn on Enable debug logging, make the problem happen again, then try again.".to_string()
    })?;
    let home = dirs::home_dir();
    let users = user_names(home.as_deref());
    let home = home.map(|h| h.display().to_string());
    let done = redact(&String::from_utf8_lossy(&raw), known, home.as_deref(), &users);
    let left = home.iter().chain(users.iter().filter(|u| u.chars().count() >= 4)).any(|s| done.text.contains(s.as_str()));
    if left {
        return Err("Your home folder or user name was still in the copy, so no copy was written.".into());
    }
    let out = data_dir.join(REPORT_NAME);
    platform::files::write_private(&out, done.text.as_bytes()).map_err(|e| format!("Could not write {REPORT_NAME}: {e}"))?;
    tracing::info!("Prepared {REPORT_NAME} for a bug report: {} names taken out", done.names);
    Ok(PreparedLog { name: REPORT_NAME.to_string(), names_known: done.names })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
12:00:00.000 INFO logger - Debug file logging started: /home/someone/.local/share/daevalog-dps-meter/debug.log
12:00:01.000 INFO identity - self record: local player 'Velmorin' -> entity 3001
12:00:02.000 DEBUG identity - player record: 'Quellar' -> entity 3002
12:00:02.500 DEBUG identity - player record: '루나' -> entity 3003
12:00:03.000 INFO roster - Roster: Tobin is entity 3004, the one unnamed player of their class
12:00:04.000 INFO spawn - Summon spawn: owner 3002 is 'Quellar'
12:00:05.000 DEBUG names - Nickname: keeping 'Quellar' for 3002, rejecting shorter 'Qu'
12:00:06.000 WARN fight_history - Could not prune /home/someone/old-someone/x.json: gone
12:00:07.000 INFO meter - Velmorin hit Tobin; me and someone else
12:00:08.000 INFO capture - Capture helper started: /usr/bin/daevalog-capture
";

    fn redacted() -> Redacted {
        redact(LOG, &["Velmorin".to_string(), "Zed".to_string()], Some("/home/someone"), &["someone".to_string()])
    }

    #[test]
    fn no_name_or_path_is_left() {
        let done = redacted();
        for name in ["Velmorin", "Quellar", "루나", "Tobin", "'Qu'", "someone", "/home"] {
            assert!(!done.text.contains(name), "{name} is in the copy:\n{}", done.text);
        }
        assert_eq!(done.names, 5, "{}", done.text);
    }

    #[test]
    fn the_log_stays_readable() {
        let done = redacted();
        assert!(done.text.contains("started: ~/.local/share/daevalog-dps-meter/debug.log"), "{}", done.text);
        assert!(done.text.contains("Could not prune ~/old-<user>/x.json"), "{}", done.text);
        assert!(done.text.contains("/usr/bin/daevalog-capture"));
        assert!(done.text.contains("me and <user> else"), "{}", done.text);
        assert_eq!(done.text.lines().count(), LOG.lines().count());
    }

    #[test]
    fn a_name_keeps_one_token() {
        let done = redacted();
        let quellar = done.text.lines().nth(2).unwrap().split('\'').nth(1).unwrap().to_string();
        assert!(quellar.starts_with("<name"), "{quellar}");
        assert_eq!(done.text.matches(&quellar).count(), 3, "{}", done.text);
        let mirabel = done.text.lines().nth(1).unwrap().split('\'').nth(1).unwrap().to_string();
        assert_ne!(mirabel, quellar);
        assert!(done.text.contains(&format!("{mirabel} hit ")), "{}", done.text);
    }

    #[test]
    fn a_short_user_name_goes_only_as_a_word() {
        let done = redact("user me at /x/me/y and meter\n", &[], None, &["me".to_string()]);
        assert_eq!(done.text, "user <user> at /x/<user>/y and meter\n");
    }

    #[test]
    fn the_copy_is_written_beside_debug_log() {
        let dir = std::env::temp_dir().join(format!("a2t-report-debug-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("a fresh test folder");
        assert!(prepare_debug_log(&dir, &[]).is_err(), "no debug.log yet");
        std::fs::write(dir.join("debug.log"), LOG).unwrap();
        let done = prepare_debug_log(&dir, &["Velmorin".to_string()]).unwrap();
        assert_eq!(done.name, REPORT_NAME);
        let copy = std::fs::read_to_string(dir.join(REPORT_NAME)).unwrap();
        assert!(!copy.contains("Velmorin") && !copy.contains("Quellar"));
        assert_eq!(std::fs::read_to_string(dir.join("debug.log")).unwrap(), LOG, "debug.log is left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
