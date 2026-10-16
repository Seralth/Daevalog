//! Replays a capture's payloads through `PingTracker` as the dispatcher feeds
//! it, and prints the ping the meter shows, read every 500 ms of capture time.
//!
//! ```text
//! A2_REPLAY_FILE=packets_x.txt   the capture (required)
//! cargo test --lib ping_report -- --ignored --nocapture
//! ```
//!
//! Each line is `ping <time of day> <ms>`, printed when the reading changes
//! (`-` while there is none). Captures hold the server's side only, so the
//! ping requests are never seen: the readings come from the replies alone.

use crate::capture::tcp_chunk::TcpChunk;
use crate::capture::packet_log::LogLine;
use crate::capture::ping_tracker::PingTracker;

/// How often the meter reads the ping.
const TICK_MS: i64 = 500;

/// Server and client port from a stream name: `Client:<client>:<server>`,
/// or the older `Client:<server>`.
pub(super) fn ports(stream: &str) -> (u16, u16) {
    let mut parts = stream.rsplit(':');
    let server = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let client = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (server, client)
}

/// The payload as the capture handed it over: from the server to us.
pub(super) fn payload(line: &LogLine) -> TcpChunk {
    let (server, client) = ports(line.stream);
    TcpChunk {
        src_port: server,
        dst_port: client,
        data: line.bytes.clone(),
        device_name: None,
        captured_at_ms: line.at_ms,
        src_ip: None,
        dst_ip: None,
        tcp_seq: None,
        tcp_ack: 0,
    }
}

pub(crate) fn run(text: &str, out: &mut dyn FnMut(String)) {
    let tracker = PingTracker::new();
    let (mut next_tick, mut shown) = (i64::MIN, None::<String>);
    let mut last_tod = String::new();
    for line in text.lines() {
        let Some(line) = LogLine::parse(line) else { continue };
        let (server, _) = ports(line.stream);
        tracker.observe_segment(&payload(&line), server);
        last_tod = line.time_of_day().to_string();
        if line.at_ms >= next_tick {
            next_tick = line.at_ms + TICK_MS;
            let reading = tracker.current_ping_ms().map_or("-".to_string(), |ms| ms.to_string());
            if shown.as_deref() != Some(reading.as_str()) {
                out(format!("ping {last_tod} {reading}"));
                shown = Some(reading);
            }
        }
    }
    let reading = tracker.current_ping_ms().map_or("-".to_string(), |ms| ms.to_string());
    out(format!("ping end {last_tod} {reading}"));
}

#[test]
#[ignore]
fn ping_report() {
    let path = std::env::var("A2_REPLAY_FILE").expect("A2_REPLAY_FILE");
    let text = std::fs::read_to_string(&path).expect("read the capture");
    run(&text, &mut |line| println!("{line}"));
}

#[test]
fn stream_names_give_both_ports() {
    assert_eq!(ports("Client:59876:13328"), (13328, 59876));
    assert_eq!(ports("Client:13328"), (13328, 0));
}
