//! Packet capture with libpcap, shared by the meter and its capture helper.
//!
//! On Linux the meter holds no capability: the helper binary (`main.rs`) holds
//! `cap_net_raw`, captures, and sends each TCP payload to the meter over a pipe
//! (`wire`). On Windows the meter captures in its own process with the same
//! code (`pcap`).

pub mod owner;
pub mod pcap;
pub mod wire;

use std::sync::OnceLock;

/// One TCP segment with a payload, as captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The device's label (its description, or its name when it has none).
    pub device: String,
    pub captured_at_ms: i64,
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
}

static LOGGER: OnceLock<fn(Level, &str)> = OnceLock::new();

/// Where capture log lines go: the meter's log, or the helper's pipe.
pub fn set_logger(logger: fn(Level, &str)) {
    let _ = LOGGER.set(logger);
}

pub(crate) fn log(level: Level, message: &str) {
    match LOGGER.get() {
        Some(logger) => logger(level, message),
        None => eprintln!("{message}"),
    }
}

/// Wall-clock time in unix ms; 0 if the clock is set before 1970.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_clock_reads_unix_ms_and_does_not_go_back() {
        let first = super::now_ms();
        let second = super::now_ms();
        assert!(first > 1_700_000_000_000);
        assert!(second >= first);
    }
}
