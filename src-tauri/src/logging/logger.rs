//! The console log, debug.log and the packet capture files. All state here is
//! process-wide: the capture thread, the UI and the settings writer all call in.

use std::fs::File;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{SecondsFormat, TimeZone};
use parking_lot::Mutex;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::filter::{EnvFilter, LevelFilter};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

use crate::capture::tcp_chunk::{stream_key, TcpChunk};
use crate::platform::files::private_options;

/// debug.log stops growing once it holds this much.
const DEBUG_FILE_CAP: u64 = 5 * 1024 * 1024;
/// Line bytes per capture file before the next one starts.
const ROLL_AT: u64 = 32 * 1024 * 1024;
/// Room for every capture file in the folder together.
const CAPTURES_BUDGET: u64 = 2 * 1024 * 1024 * 1024;
/// What is kept when a new file is about to start, so it fits too.
const TRIM_TO: u64 = CAPTURES_BUDGET - ROLL_AT;
/// No game traffic for this long ends a play session: the next packet starts
/// a new capture file.
const SESSION_GAP_MS: i64 = 10 * 60 * 1000;

/// The console at RUST_LOG's level, and debug.log at DEBUG whatever RUST_LOG says.
/// Once per process: a second call panics.
pub fn init_logging() {
    let console = console_filter(std::env::var("RUST_LOG").ok().as_deref());
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false).with_filter(console))
        .with(debug_file_layer())
        .init();
}

/// The console level from a RUST_LOG value; `info` when missing or unreadable.
fn console_filter(spec: Option<&str>) -> EnvFilter {
    spec.filter(|s| !s.trim().is_empty())
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new("info"))
}

fn debug_file_layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    DebugTap.with_filter(LevelFilter::DEBUG)
}

/// Cut a long message to 240 bytes like the Kotlin version, on a character
/// boundary: names and chat in Korean are three bytes a character.
fn shorten(msg: String) -> String {
    if msg.len() <= 240 {
        return msg;
    }
    let mut cut = 237;
    while !msg.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &msg[..cut])
}

// ---- debug.log ----

struct DebugSink {
    file: File,
    /// Bytes in the file, counted from its size when it was opened.
    size: u64,
}

/// Read on every event, so turned-off logging costs one load.
static DEBUG_ON: AtomicBool = AtomicBool::new(false);
static DEBUG_SINK: Mutex<Option<DebugSink>> = Mutex::new(None);

/// Turn debug.log in `log_dir` on or off. Turning on twice keeps the first folder.
pub fn set_debug_enabled(enabled: bool, log_dir: &Path) {
    if enabled {
        open_debug_file(log_dir);
    } else {
        close_debug_file();
    }
}

fn open_debug_file(dir: &Path) {
    let path = dir.join("debug.log");
    let opened = {
        let mut slot = DEBUG_SINK.lock();
        if slot.is_some() {
            return;
        }
        let sink = private_options()
            .append(true)
            .create(true)
            .open(&path)
            .and_then(|file| Ok(DebugSink { size: file.metadata()?.len(), file }));
        sink.map(|sink| {
            *slot = Some(sink);
            DEBUG_ON.store(true, Ordering::Release);
        })
    };
    // The lock is free again here, so these lines can reach the file.
    if opened.is_err() {
        tracing::warn!("Could not start debug.log in {}", dir.display());
        return;
    }
    tracing::info!("Debug file logging started: {}", path.display());
}

fn close_debug_file() {
    let closed = DEBUG_SINK.lock().take();
    if closed.is_none() {
        return;
    }
    DEBUG_ON.store(false, Ordering::Release);
    drop(closed);
    tracing::info!("Debug file logging stopped");
}

/// Hands every event it is given to debug.log while that is open.
struct DebugTap;

impl<S: Subscriber> Layer<S> for DebugTap {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if !DEBUG_ON.load(Ordering::Acquire) {
            return;
        }
        let row = debug_row(event);
        // Never wait for the file: when another thread is writing, or this
        // thread already holds it, the line is dropped.
        let Some(mut slot) = DEBUG_SINK.try_lock() else { return };
        let Some(sink) = slot.as_mut() else { return };
        if sink.size > DEBUG_FILE_CAP {
            return;
        }
        if sink.file.write_all(row.as_bytes()).and_then(|()| sink.file.flush()).is_ok() {
            sink.size += row.len() as u64;
        }
    }
}

/// `HH:MM:SS.mmm LEVEL module - message`, with its newline.
fn debug_row(event: &Event<'_>) -> String {
    let meta = event.metadata();
    let mut text = EventText::default();
    event.record(&mut text);
    // Records from the `log` crate carry their own module in a field.
    let module = text.origin.clone().or(meta.module_path().map(str::to_owned)).unwrap_or_default();
    let module = module.rsplit("::").next().unwrap_or_default().to_owned();
    let clock = chrono::Local::now().format("%H:%M:%S%.3f");
    format!("{clock} {} {module} - {}\n", meta.level(), shorten(text.finish()))
}

#[derive(Default)]
struct EventText {
    message: Option<String>,
    pairs: Vec<String>,
    origin: Option<String>,
}

impl EventText {
    fn add(&mut self, field: &Field, value: String) {
        match field.name() {
            "message" => self.message = Some(value),
            "log.module_path" => self.origin = Some(value),
            // The rest of a `log` record's bookkeeping says nothing new.
            name if name.starts_with("log.") => {}
            name => self.pairs.push(format!("{name}={value}")),
        }
    }

    fn finish(self) -> String {
        match self.message {
            None => self.pairs.join(" "),
            Some(mut text) => {
                for pair in &self.pairs {
                    text.push(' ');
                    text.push_str(pair);
                }
                text
            }
        }
    }
}

impl Visit for EventText {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.add(field, value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.add(field, format!("{value:?}"));
    }
}

// ---- packet capture files ----

struct CaptureFile {
    file: File,
    path: PathBuf,
    folder: PathBuf,
    /// Line bytes written to this file; the header is not counted.
    lines_bytes: u64,
    /// The local date of the first line.
    day: Option<chrono::NaiveDate>,
    /// Capture time of the newest line.
    last_ms: Option<i64>,
    /// Your character, once the game's record named it.
    you: Option<String>,
}

impl CaptureFile {
    fn new(file: File, path: PathBuf, folder: PathBuf) -> Self {
        CaptureFile { file, path, folder, lines_bytes: 0, day: None, last_ms: None, you: None }
    }

    /// Why a line captured `at` starts a new file, if it does.
    fn turn_over(&self, at: chrono::DateTime<chrono::Local>, you: Option<&str>) -> Option<&'static str> {
        if self.last_ms.is_some_and(|last| at.timestamp_millis() - last >= SESSION_GAP_MS) {
            Some("no game traffic for 10 minutes")
        } else if you.is_some() && self.you.is_some() && self.you.as_deref() != you {
            Some("another character")
        } else if self.day.is_some_and(|day| at.date_naive() > day) {
            Some("a new day")
        } else {
            None
        }
    }
}

/// Checked first by `write_segment_line`, which runs for every payload.
static CAPTURE_ON: AtomicBool = AtomicBool::new(false);
static CAPTURE: Mutex<Option<CaptureFile>> = Mutex::new(None);

/// Start or stop writing raw packets to a new file in `log_dir`.
pub fn set_packet_log_enabled(enabled: bool, log_dir: &Path) {
    let mut slot = CAPTURE.lock();
    if enabled && slot.is_none() {
        trim_captures(log_dir, TRIM_TO);
        match create_capture_file(log_dir, chrono::Local::now()) {
            Ok((file, path)) => {
                tracing::info!("Raw packet logging started: {}", path.display());
                *slot = Some(CaptureFile::new(file, path, log_dir.to_path_buf()));
                CAPTURE_ON.store(true, Ordering::Release);
            }
            Err(_) => tracing::warn!("Could not start the packet log in {}", log_dir.display()),
        }
    } else if !enabled {
        if let Some(done) = slot.take() {
            CAPTURE_ON.store(false, Ordering::Release);
            let path = done.path.clone();
            drop(done);
            tracing::info!("Raw packet logging stopped: {}", path.display());
        }
    }
}

/// Whether a packet log is being written.
pub fn packet_log_on() -> bool {
    CAPTURE_ON.load(Ordering::Relaxed)
}

/// Append one payload to the capture file, if one is open. The line is on
/// disk before this returns, so a crash keeps the tail. `you` is your
/// character, once the game's record named it: another one starts a new file,
/// as do a long quiet and midnight.
pub fn write_segment_line(cap: &TcpChunk, you: Option<&str>) {
    if !CAPTURE_ON.load(Ordering::Relaxed) {
        return;
    }
    let at = capture_moment(cap);
    let row = format_packet_row(cap);
    let mut slot = CAPTURE.lock();
    let Some(open) = slot.as_ref() else { return };
    if let Some(why) = open.turn_over(at, you) {
        tracing::info!("New packet capture file: {why}");
        start_next_file(&mut slot, at);
    }
    let Some(open) = slot.as_mut() else { return };
    if let Some(you) = you {
        open.you = Some(you.to_owned());
    }
    open.day.get_or_insert(at.date_naive());
    open.last_ms = Some(at.timestamp_millis());
    if open.file.write_all(row.as_bytes()).and_then(|()| open.file.flush()).is_ok() {
        open.lines_bytes += row.len() as u64;
    }
    if open.lines_bytes >= ROLL_AT {
        start_next_file(&mut slot, chrono::Local::now());
    }
}

/// Carry on in a fresh file started `now`, or stop if none opens.
fn start_next_file(slot: &mut Option<CaptureFile>, now: chrono::DateTime<chrono::Local>) {
    let Some(current) = slot.as_mut() else { return };
    trim_captures(&current.folder, TRIM_TO);
    match create_capture_file(&current.folder, now) {
        Ok((file, path)) => {
            tracing::info!("Packet capture rolled over: {} -> {}", current.path.display(), path.display());
            *current = CaptureFile { you: current.you.take(), ..CaptureFile::new(file, path, current.folder.clone()) };
        }
        Err(_) => {
            tracing::warn!("Could not roll over packet capture; stopping capture");
            CAPTURE_ON.store(false, Ordering::Release);
            *slot = None;
        }
    }
}

/// The payload's capture time, or now when the capture gave none.
fn capture_moment(cap: &TcpChunk) -> chrono::DateTime<chrono::Local> {
    cap.capture_time_ms()
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .unwrap_or_else(chrono::Local::now)
}

/// `<local time>|<stream key>|<HEX>`, with its newline.
fn format_packet_row(cap: &TcpChunk) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let stamp = capture_moment(cap).to_rfc3339_opts(SecondsFormat::Millis, false);
    let key = stream_key(cap.src_port, cap.dst_port);
    let mut row = String::with_capacity(stamp.len() + key.len() + cap.data.len() * 2 + 3);
    row.push_str(&stamp);
    row.push('|');
    row.push_str(&key);
    row.push('|');
    for &byte in &cap.data {
        row.push(DIGITS[usize::from(byte >> 4)] as char);
        row.push(DIGITS[usize::from(byte & 0x0F)] as char);
    }
    row.push('\n');
    row
}

/// Make `packets_<stamp>.txt` in `dir`, or the first free `_1` to `_99` after
/// the stamp, and write the header. Never reuses a name that is there: with
/// all 100 taken the open fails.
fn create_capture_file(dir: &Path, now: chrono::DateTime<chrono::Local>) -> std::io::Result<(File, PathBuf)> {
    let stamp = now.format("%Y%m%d_%H%M%S").to_string();
    for suffix in 0..100u32 {
        let name = match suffix {
            0 => format!("packets_{stamp}.txt"),
            n => format!("packets_{stamp}_{n}.txt"),
        };
        let path = dir.join(name);
        let mut file = match private_options().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let header = format!(
            "# Packet capture started at {}\n# Format: TIMESTAMP|STREAMKEY|HEX_DATA\n\n",
            now.to_rfc3339()
        );
        if let Err(e) = file.write_all(header.as_bytes()).and_then(|()| file.flush()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        return Ok((file, path));
    }
    Err(ErrorKind::AlreadyExists.into())
}

/// Delete the oldest capture files in `dir` until the rest fit in `budget`.
/// Only `packets_*.txt` names count; the stamps sort them oldest first.
fn trim_captures(dir: &Path, budget: u64) {
    let Ok(listing) = std::fs::read_dir(dir) else { return };
    let mut captures: Vec<(String, PathBuf, u64)> = listing
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !(name.starts_with("packets_") && name.ends_with(".txt")) {
                return None;
            }
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            Some((name, entry.path(), bytes))
        })
        .collect();
    captures.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut total: u64 = captures.iter().map(|c| c.2).sum();
    let mut oldest_first = captures.into_iter();
    while total > budget {
        let Some((_, path, bytes)) = oldest_first.next() else { break };
        // A file that would not go is still left out, so one stuck file
        // cannot take the newer ones with it.
        total = total.saturating_sub(bytes);
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!("Could not remove {}: {e}", path.display());
        } else {
            tracing::info!("Removed old packet capture {} ({bytes} bytes)", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packet_is_stamped_with_its_capture_time() {
        // Off the 100 ms read grid, and long ago, so the time of writing
        // cannot pass for it.
        let at = 1_767_355_234_407;
        let line = format_packet_row(&sample_capture(at));
        let mut parts = line.trim_end().splitn(3, '|');
        let (ts, key, hex) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        // Read back the way every replay reads a capture.
        assert_eq!(chrono::DateTime::parse_from_rfc3339(ts).unwrap().timestamp_millis(), at, "{line}");
        assert_eq!(ts.len(), "2026-01-02T12:00:34.407+00:00".len(), "{ts}");
        assert_eq!(key, "Client:50000:13328");
        assert_eq!(hex, "050438AB");
    }

    #[test]
    fn old_captures_with_nanosecond_stamps_still_read() {
        let old = "2026-01-02T12:00:05.123456789+00:00";
        assert_eq!(chrono::DateTime::parse_from_rfc3339(old).unwrap().timestamp_millis(), 1_767_355_205_123);
    }

    #[test]
    fn a_payload_without_a_capture_time_is_stamped_now() {
        let before = chrono::Local::now().timestamp_millis();
        let line = format_packet_row(&sample_capture(0));
        let ts = line.split('|').next().unwrap();
        let at = chrono::DateTime::parse_from_rfc3339(ts).unwrap().timestamp_millis();
        assert!(at >= before - 1 && at <= chrono::Local::now().timestamp_millis(), "{line}");
    }

    #[test]
    fn a_long_korean_message_is_cut_between_characters() {
        let msg = format!("a{}", "가".repeat(100));
        let short = shorten(msg.clone());
        assert!(short.len() <= 240 && short.ends_with("..."));
        assert!(msg.starts_with(short.trim_end_matches("...")));
        assert_eq!(shorten("short".into()), "short");
    }

    use std::sync::Arc;

    use crate::built::Scratch;

    /// Tests that turn the process-wide logs on and off hold these. A test
    /// that needs both takes CAPTURE_TURN first (`watched` takes DEBUG_TURN).
    static DEBUG_TURN: Mutex<()> = Mutex::new(());
    static CAPTURE_TURN: Mutex<()> = Mutex::new(());

    const AT: i64 = 1_767_380_434_407;

    fn sample_capture(captured_at_ms: i64) -> TcpChunk {
        crate::built::segment(Some("eth0"), 13328, 50000, &[0x05, 0x04, 0x38, 0xAB], captured_at_ms)
    }

    /// A payload whose line is exactly the rollover size.
    fn full_file_capture() -> TcpChunk {
        let mut cap = sample_capture(AT);
        cap.data = vec![0x7E; ((ROLL_AT - 50) / 2) as usize];
        cap
    }

    fn put(dir: &Path, name: &str, text: &str) {
        File::create(dir.join(name)).and_then(|mut f| f.write_all(text.as_bytes())).unwrap();
    }

    fn files_in_dir(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// Keeps every event as a debug.log line would read.
    struct Heard(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Heard {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            self.0.lock().push(debug_row(event));
        }
    }

    /// Run `f` under the debug file writer (scoped to this thread, not global)
    /// and return every line logged meanwhile.
    fn watched(f: impl FnOnce()) -> Vec<String> {
        let _turn = DEBUG_TURN.lock();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(debug_file_layer()).with(Heard(heard.clone()));
        crate::trace_tap::with_default(subscriber, f);
        let lines = heard.lock().clone();
        lines
    }

    /// The part after `HH:MM:SS.mmm `, once the clock part is checked.
    fn after_clock(line: &str) -> &str {
        let clock = line.as_bytes();
        assert!(clock.len() > 13 && clock[12] == b' ', "{line}");
        for (i, c) in clock[..12].iter().enumerate() {
            match i {
                2 | 5 => assert_eq!(*c, b':', "{line}"),
                8 => assert_eq!(*c, b'.', "{line}"),
                _ => assert!(c.is_ascii_digit(), "{line}"),
            }
        }
        &line[13..]
    }

    #[test]
    fn console_level_follows_rust_log() {
        let level = |spec: Option<&str>| console_filter(spec).max_level_hint();
        assert_eq!(level(None), Some(LevelFilter::INFO));
        assert_eq!(level(Some("")), Some(LevelFilter::INFO));
        assert_eq!(level(Some("foo=notalevel")), Some(LevelFilter::INFO));
        assert_eq!(level(Some("debug")), Some(LevelFilter::DEBUG));
        assert_eq!(level(Some("warn")), Some(LevelFilter::WARN));
    }

    const CHILD_FOLDER: &str = "DAEVALOG_LOGS_CHILD_FOLDER";

    /// Runs only inside `init_logging_in_a_fresh_process`, which starts this
    /// test binary again with CHILD_FOLDER set.
    #[test]
    fn init_logging_child_half() {
        let Some(dir) = std::env::var_os(CHILD_FOLDER) else { return };
        let dir = PathBuf::from(dir);
        init_logging();
        tracing::debug!("console-debug-mark");
        tracing::info!("console-info-mark");
        set_debug_enabled(true, &dir);
        tracing::debug!("file-debug-mark");
        tracing::trace!("file-trace-mark");
        set_debug_enabled(false, &dir);
        let text = read(&dir.join("debug.log"));
        assert!(text.contains("DEBUG tests - file-debug-mark"), "{text}");
        assert!(!text.contains("file-trace-mark"), "{text}");
        assert!(std::panic::catch_unwind(init_logging).is_err(), "a second init_logging must panic");
    }

    #[test]
    fn init_logging_in_a_fresh_process() {
        for (case, rust_log, info_shown) in [("unset", None, true), ("error", Some("error"), false)] {
            let dir = Scratch::new(&format!("logs.child-{case}"));
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args(["--exact", "logging::logger::tests::init_logging_child_half", "--nocapture", "--test-threads=1"])
                .env(CHILD_FOLDER, dir.as_os_str());
            match rust_log {
                Some(value) => child.env("RUST_LOG", value),
                None => child.env_remove("RUST_LOG"),
            };
            let out = child.output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "{case}: {stdout}\n{stderr}");
            assert!(stdout.contains("1 passed"), "{case}: {stdout}");
            assert_eq!(stdout.contains("console-info-mark"), info_shown, "{case}: {stdout}");
            assert!(!stdout.contains("console-debug-mark"), "{case}: {stdout}");
            }
    }

    #[test]
    fn debug_log_that_cannot_open_stays_off_until_a_good_folder() {
        let dir = Scratch::new("logs.debug-bad");
        put(&dir, "plain-file", "x");
        let not_a_folder = dir.join("plain-file");
        let lines = watched(|| {
            set_debug_enabled(true, &not_a_folder);
            assert!(DEBUG_SINK.lock().is_none());
            assert!(!DEBUG_ON.load(Ordering::Acquire));
            set_debug_enabled(true, &dir);
            assert!(DEBUG_SINK.lock().is_some());
            set_debug_enabled(false, &dir);
        });
        let warned = format!("WARN logger - Could not start debug.log in {}\n", not_a_folder.display());
        assert!(lines.iter().any(|l| after_clock(l) == warned), "{lines:?}");
        assert!(read(&dir.join("debug.log")).contains("Debug file logging started"));
    }

    #[test]
    fn a_second_folder_is_ignored_while_debug_log_is_open() {
        let first = Scratch::new("logs.debug-first");
        let second = Scratch::new("logs.debug-second");
        let lines = watched(|| {
            set_debug_enabled(false, &first);
            set_debug_enabled(true, &first);
            set_debug_enabled(true, &second);
            tracing::info!("after both");
            set_debug_enabled(false, &first);
            set_debug_enabled(false, &first);
        });
        assert!(read(&first.join("debug.log")).contains("after both"));
        assert!(files_in_dir(&second).is_empty());
        assert_eq!(lines.iter().filter(|l| l.contains("Debug file logging stopped")).count(), 1, "{lines:?}");
    }

    #[test]
    fn stopping_debug_log_closes_it_and_says_so_on_the_console_only() {
        let dir = Scratch::new("logs.debug-stop");
        let lines = watched(|| {
            set_debug_enabled(true, &dir);
            tracing::info!("kept line");
            set_debug_enabled(false, &dir);
            tracing::info!("late line");
        });
        let text = read(&dir.join("debug.log"));
        assert!(text.contains("kept line") && !text.contains("late line"), "{text}");
        assert!(!text.contains("stopped"), "{text}");
        assert!(lines.iter().any(|l| after_clock(l) == "INFO logger - Debug file logging stopped\n"), "{lines:?}");
    }

    #[test]
    fn debug_lines_carry_clock_level_module_and_message() {
        let dir = Scratch::new("logs.debug-format");
        watched(|| {
            set_debug_enabled(true, &dir);
            tracing::info!(count = 3, "Saved");
            tracing::warn!(path = "a/b", n = 2);
            tracing::debug!("deep");
            tracing::trace!("too deep");
            tracing::error!(name = ?"quoted", "Failed");
            set_debug_enabled(false, &dir);
        });
        let text = read(&dir.join("debug.log"));
        let bodies: Vec<&str> = text.lines().map(after_clock).collect();
        for want in ["INFO tests - Saved count=3", "WARN tests - path=a/b n=2", "DEBUG tests - deep", "ERROR tests - Failed name=\"quoted\""] {
            assert!(bodies.contains(&want), "{want} in {text}");
        }
        assert!(!text.contains("too deep"), "{text}");
    }

    #[test]
    fn log_crate_records_name_their_own_module() {
        // The shape tracing-log gives a `log` record: its origin in log.* fields.
        let lines = watched(|| {
            tracing::info!(
                log.target = "some_crate::net",
                log.module_path = "some_crate::net::pool",
                log.file = "src/net/pool.rs",
                log.line = 7u32,
                "connection reused"
            );
        });
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(after_clock(&lines[0]), "INFO pool - connection reused\n");
    }

    #[test]
    fn long_messages_are_cut_to_240_bytes() {
        let exact = "b".repeat(240);
        assert_eq!(shorten(exact.clone()), exact);
        assert_eq!(shorten("c".repeat(241)), format!("{}...", "c".repeat(237)));
        let korean = shorten(format!("a{}", "가".repeat(100)));
        assert_eq!(korean, format!("a{}...", "가".repeat(78)));
        assert_eq!(korean.len(), 238);
    }

    #[test]
    fn a_debug_log_over_5_mib_gets_nothing_more() {
        let dir = Scratch::new("logs.debug-full");
        let file = dir.join("debug.log");
        std::fs::File::create(&file).unwrap().set_len(6 * 1024 * 1024).unwrap();
        watched(|| {
            set_debug_enabled(true, &dir);
            tracing::info!("one more");
            set_debug_enabled(false, &dir);
        });
        assert_eq!(std::fs::metadata(&file).unwrap().len(), 6 * 1024 * 1024);
    }

    #[test]
    fn a_busy_debug_log_drops_lines_instead_of_waiting() {
        let dir = Scratch::new("logs.debug-busy");
        let file = dir.join("debug.log");
        watched(|| {
            set_debug_enabled(true, &dir);
            let held = DEBUG_SINK.lock();
            tracing::info!("dropped while held");
            drop(held);
            tracing::info!("written after");
            // A write that fails leaves the count where it was.
            let before = {
                let mut slot = DEBUG_SINK.lock();
                let sink = slot.as_mut().unwrap();
                sink.file = File::open(&file).unwrap();
                sink.size
            };
            tracing::info!("cannot be written");
            assert_eq!(DEBUG_SINK.lock().as_ref().unwrap().size, before);
            set_debug_enabled(false, &dir);
        });
        let text = read(&file);
        assert!(text.contains("written after"), "{text}");
        assert!(!text.contains("dropped while held") && !text.contains("cannot be written"), "{text}");
    }

    #[test]
    fn trimming_drops_oldest_captures_first() {
        let dir = Scratch::new("logs.trim-oldest");
        for day in 1..=4 {
            put(&dir, &format!("packets_2026010{day}_000000.txt"), "x");
        }
        let lines = watched(|| trim_captures(&dir, 2));
        assert_eq!(files_in_dir(&dir), ["packets_20260103_000000.txt", "packets_20260104_000000.txt"]);
        let removed = format!("INFO logger - Removed old packet capture {} (1 bytes)\n", dir.join("packets_20260101_000000.txt").display());
        assert!(lines.iter().any(|l| after_clock(l) == removed), "{lines:?}");
    }

    #[test]
    fn trimming_spares_files_that_are_not_captures() {
        let dir = Scratch::new("logs.trim-others");
        let others = ["debug.log", "my-important-capture.txt", "rawpackets_20260101_000000.txt"];
        for name in others.iter().chain(&["packets_20260101_000000.txt", "packets_20260102_000000.txt", "packets_20260103_000000.txt"]) {
            put(&dir, name, "x");
        }
        trim_captures(&dir, 1);
        let mut left = others.to_vec();
        left.push("packets_20260103_000000.txt");
        left.sort();
        assert_eq!(files_in_dir(&dir), left);
    }

    #[test]
    fn trimming_under_budget_removes_nothing() {
        let dir = Scratch::new("logs.trim-under");
        put(&dir, "packets_20260101_000000.txt", "x");
        trim_captures(&dir, 5);
        assert_eq!(files_in_dir(&dir), ["packets_20260101_000000.txt"]);
    }

    #[test]
    fn trimming_a_folder_that_cannot_be_read_does_nothing() {
        let dir = Scratch::new("logs.trim-unreadable");
        put(&dir, "packets_20260101_000000.txt", "x");
        trim_captures(&dir.join("packets_20260101_000000.txt"), 0);
        trim_captures(&dir.join("missing"), 0);
        assert_eq!(files_in_dir(&dir), ["packets_20260101_000000.txt"]);
    }

    // NTFS gives every folder a size of 0.
    #[cfg(unix)]
    #[test]
    fn a_capture_that_will_not_go_still_counts_as_gone() {
        let dir = Scratch::new("logs.trim-stuck");
        // A folder with a capture's name: removing it as a file fails.
        let stuck = dir.join("packets_20260101_000000.txt");
        std::fs::create_dir(&stuck).unwrap();
        // Some file systems give an empty folder a size of 0.
        put(&stuck, "inside", "x");
        put(&dir, "packets_20260102_000000.txt", "x");
        assert!(std::fs::metadata(&stuck).unwrap().len() > 0, "this test needs a folder size above 0");
        let lines = watched(|| trim_captures(&dir, 1));
        let warned = format!("WARN logger - Could not remove {}: ", stuck.display());
        assert!(lines.iter().any(|l| after_clock(l).starts_with(&warned)), "{lines:?}");
        assert_eq!(files_in_dir(&dir), ["packets_20260101_000000.txt", "packets_20260102_000000.txt"]);
    }

    #[test]
    fn two_files_in_one_second_get_different_names() {
        let dir = Scratch::new("logs.same-second");
        let now = chrono::Local::now();
        let (_, first) = create_capture_file(&dir, now).unwrap();
        let (_, second) = create_capture_file(&dir, now).unwrap();
        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
        // A rollover's file sorts after the one it replaces.
        assert!(first.file_name().unwrap().to_str() < second.file_name().unwrap().to_str());
    }

    #[test]
    fn taken_names_move_on_to_numbered_suffixes() {
        let dir = Scratch::new("logs.suffixes");
        let when = chrono::Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        for name in ["packets_20260102_030405.txt", "packets_20260102_030405_1.txt", "packets_20260102_030405_2.txt"] {
            put(&dir, name, "older");
        }
        let (_, path) = create_capture_file(&dir, when).unwrap();
        assert_eq!(path, dir.join("packets_20260102_030405_3.txt"));
        assert_eq!(read(&dir.join("packets_20260102_030405_2.txt")), "older");
    }

    #[test]
    fn with_every_name_taken_the_open_fails_and_keeps_the_files() {
        let dir = Scratch::new("logs.all-taken");
        let when = chrono::Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        put(&dir, "packets_20260102_030405.txt", "older");
        for n in 1..=99 {
            put(&dir, &format!("packets_20260102_030405_{n}.txt"), "older");
        }
        assert!(create_capture_file(&dir, when).is_err());
        let names = files_in_dir(&dir);
        assert_eq!(names.len(), 100);
        assert!(names.iter().all(|n| read(&dir.join(n)) == "older"));
    }

    #[test]
    fn a_new_capture_file_starts_with_the_header() {
        let dir = Scratch::new("logs.header");
        let now = chrono::Local::now();
        let (_, path) = create_capture_file(&dir, now).unwrap();
        assert_eq!(path, dir.join(format!("packets_{}.txt", now.format("%Y%m%d_%H%M%S"))));
        let text = read(&path);
        let (first, rest) = text.split_once('\n').unwrap();
        let stamp = first.strip_prefix("# Packet capture started at ").unwrap();
        assert_eq!(chrono::DateTime::parse_from_rfc3339(stamp).unwrap(), now);
        assert_eq!(rest, "# Format: TIMESTAMP|STREAMKEY|HEX_DATA\n\n");
        assert_eq!(rest.len(), 40);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn an_empty_payload_gives_an_empty_hex_field() {
        let mut cap = sample_capture(AT);
        cap.data.clear();
        let row = format_packet_row(&cap);
        assert!(row.ends_with("|Client:50000:13328|\n"), "{row}");
        assert_eq!(row.len(), 29 + 1 + 18 + 2);
    }

    #[test]
    fn packet_log_writes_each_line_at_once_until_stopped() {
        let _turn = CAPTURE_TURN.lock();
        let dir = Scratch::new("logs.packets-run");
        let other = Scratch::new("logs.packets-other");
        write_segment_line(&sample_capture(AT), None);
        assert!(files_in_dir(&dir).is_empty());
        let row = format_packet_row(&sample_capture(AT));
        let mut path = PathBuf::new();
        let lines = watched(|| {
            set_packet_log_enabled(true, &dir);
            write_segment_line(&sample_capture(AT), None);
            write_segment_line(&sample_capture(AT), None);
            set_packet_log_enabled(true, &other);
            path = CAPTURE.lock().as_ref().unwrap().path.clone();
            // On disk before the log stops.
            assert!(read(&path).ends_with(&format!("\n\n{row}{row}")));
            set_packet_log_enabled(false, &dir);
            set_packet_log_enabled(false, &dir);
            write_segment_line(&sample_capture(AT), None);
        });
        assert_eq!(path.parent(), Some(&*dir));
        assert_eq!(files_in_dir(&dir).len(), 1);
        assert!(files_in_dir(&other).is_empty());
        assert!(read(&path).ends_with(&format!("HEX_DATA\n\n{row}{row}")));
        let started = format!("INFO logger - Raw packet logging started: {}\n", path.display());
        let stopped = format!("INFO logger - Raw packet logging stopped: {}\n", path.display());
        assert_eq!(lines.iter().filter(|l| after_clock(l) == started).count(), 1, "{lines:?}");
        assert_eq!(lines.iter().filter(|l| after_clock(l) == stopped).count(), 1, "{lines:?}");
    }

    #[test]
    fn packet_log_that_cannot_open_stays_off_until_a_good_folder() {
        let _turn = CAPTURE_TURN.lock();
        let dir = Scratch::new("logs.packets-bad");
        put(&dir, "plain-file", "x");
        let not_a_folder = dir.join("plain-file");
        let lines = watched(|| {
            set_packet_log_enabled(true, &not_a_folder);
            assert!(CAPTURE.lock().is_none() && !CAPTURE_ON.load(Ordering::Acquire));
            write_segment_line(&sample_capture(AT), None);
            set_packet_log_enabled(true, &dir);
            assert!(CAPTURE.lock().is_some());
            set_packet_log_enabled(false, &dir);
        });
        let warned = format!("WARN logger - Could not start the packet log in {}\n", not_a_folder.display());
        assert!(lines.iter().any(|l| after_clock(l) == warned), "{lines:?}");
        assert_eq!(files_in_dir(&dir).len(), 2);
    }

    #[test]
    fn starting_the_packet_log_trims_the_folder_first() {
        let _turn = CAPTURE_TURN.lock();
        let dir = Scratch::new("logs.packets-trim");
        // Sparse, so the test writes almost nothing.
        File::create(dir.join("packets_20200101_000000.txt")).unwrap().set_len(TRIM_TO).unwrap();
        put(&dir, "packets_20200102_000000.txt", "x");
        watched(|| {
            set_packet_log_enabled(true, &dir);
            set_packet_log_enabled(false, &dir);
        });
        let names = files_in_dir(&dir);
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(names[0], "packets_20200102_000000.txt");
    }

    #[test]
    fn a_full_capture_file_rolls_over_at_once() {
        let _turn = CAPTURE_TURN.lock();
        let dir = Scratch::new("logs.packets-roll");
        let full = full_file_capture();
        assert_eq!(format_packet_row(&full).len() as u64, ROLL_AT);
        let lines = watched(|| {
            set_packet_log_enabled(true, &dir);
            write_segment_line(&full, None);
            set_packet_log_enabled(false, &dir);
        });
        let names = files_in_dir(&dir);
        assert_eq!(names.len(), 2, "{names:?}");
        let (old, new) = (dir.join(&names[0]), dir.join(&names[1]));
        assert_eq!(std::fs::metadata(&old).unwrap().len(), ROLL_AT + read_header_len(&old));
        assert_eq!(std::fs::metadata(&new).unwrap().len(), read_header_len(&new));
        let rolled = format!("INFO logger - Packet capture rolled over: {} -> {}\n", old.display(), new.display());
        assert!(lines.iter().any(|l| after_clock(l) == rolled), "{lines:?}");
    }

    fn read_header_len(path: &Path) -> u64 {
        let mut text = String::new();
        let mut file = std::io::BufReader::new(File::open(path).unwrap());
        for _ in 0..3 {
            std::io::BufRead::read_line(&mut file, &mut text).unwrap();
        }
        text.len() as u64
    }

    /// Capture time in ms of a local time on day `day` of January 2026.
    fn local_ms(day: u32, h: u32, m: u32, s: u32) -> i64 {
        chrono::Local.with_ymd_and_hms(2026, 1, day, h, m, s).unwrap().timestamp_millis()
    }

    /// Writes each (time, character) line to a fresh packet log in `dir`
    /// and gives the text of each file made, by name.
    fn logged(dir: &Path, lines: &[(i64, Option<&str>)]) -> std::collections::BTreeMap<String, String> {
        let _turn = CAPTURE_TURN.lock();
        set_packet_log_enabled(true, dir);
        for (at, you) in lines {
            write_segment_line(&sample_capture(*at), *you);
        }
        set_packet_log_enabled(false, dir);
        files_in_dir(dir).into_iter().map(|name| (name.clone(), read(&dir.join(name)))).collect()
    }

    /// The text of a file started by a line captured `at`, named by that time,
    /// with the header and that line first.
    fn file_started_at(files: &std::collections::BTreeMap<String, String>, at: i64) -> &str {
        let when = chrono::Local.timestamp_millis_opt(at).unwrap();
        let name = format!("packets_{}.txt", when.format("%Y%m%d_%H%M%S"));
        let text = files.get(&name).unwrap_or_else(|| panic!("{name} in {files:?}"));
        let header = format!("# Packet capture started at {}\n# Format: TIMESTAMP|STREAMKEY|HEX_DATA\n\n", when.to_rfc3339());
        let first = format_packet_row(&sample_capture(at));
        assert!(text.starts_with(&format!("{header}{first}")), "{text}");
        text
    }

    /// Lines in every file, headers left out.
    fn rows(files: &std::collections::BTreeMap<String, String>) -> usize {
        files.values().map(|text| text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).count()).sum()
    }

    #[test]
    fn ten_quiet_minutes_start_a_new_file_with_the_next_packet() {
        let dir = Scratch::new("logs.packets-quiet");
        let first = local_ms(2, 12, 0, 0);
        let near = first + SESSION_GAP_MS - 1;
        let after = near + SESSION_GAP_MS;
        let files = logged(&dir, &[(first, None), (near, None), (after, None)]);
        assert_eq!((files.len(), rows(&files)), (2, 3), "{files:?}");
        assert_eq!(file_started_at(&files, after).lines().count(), 4);
    }

    #[test]
    fn another_character_starts_a_new_file() {
        let dir = Scratch::new("logs.packets-character");
        let at = local_ms(2, 12, 0, 0);
        let lines = [(at, None), (at + 1, Some("Qxqz")), (at + 2, None), (at + 3, Some("Qxqz")), (at + 4, Some("Vrelt")), (at + 5, None)];
        let files = logged(&dir, &lines);
        assert_eq!((files.len(), rows(&files)), (2, 6), "{files:?}");
        assert_eq!(file_started_at(&files, at + 4).lines().count(), 5);
    }

    #[test]
    fn midnight_starts_a_new_file() {
        let dir = Scratch::new("logs.packets-midnight");
        let (late, early) = (local_ms(2, 23, 59, 30), local_ms(3, 0, 0, 10));
        let files = logged(&dir, &[(late, None), (early, None), (early + 1, None)]);
        assert_eq!((files.len(), rows(&files)), (2, 3), "{files:?}");
        assert_eq!(file_started_at(&files, early).lines().count(), 5);
        assert!(files.contains_key("packets_20260103_000010.txt"), "{files:?}");
    }

    // Windows does not delete a folder that holds an open file.
    #[cfg(unix)]
    #[test]
    fn a_rollover_that_cannot_open_stops_the_packet_log() {
        let _turn = CAPTURE_TURN.lock();
        let dir = Scratch::new("logs.packets-roll-fail");
        let lines = watched(|| {
            set_packet_log_enabled(true, &dir);
            // The open file keeps working; a new one cannot be made.
            std::fs::remove_dir_all(&*dir).unwrap();
            write_segment_line(&full_file_capture(), None);
            assert!(CAPTURE.lock().is_none() && !CAPTURE_ON.load(Ordering::Acquire));
            write_segment_line(&sample_capture(AT), None);
        });
        let warned = "WARN logger - Could not roll over packet capture; stopping capture\n";
        assert!(lines.iter().any(|l| after_clock(l) == warned), "{lines:?}");
    }
}
