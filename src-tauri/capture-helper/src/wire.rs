//! The pipe between the meter and the capture helper.
//!
//! Each message is one frame: a little-endian `u32` body length, then the body.
//! The body starts with a kind byte. Strings are a `u16` length and UTF-8.
//! A frame longer than `MAX_FRAME` is refused before anything is read into
//! memory, so a broken peer cannot make the reader allocate without limit.

use std::io::{self, Read};

use crate::{Level, Segment};

/// The longest body either side accepts. A packet is at most the 65535-byte
/// snap length plus its header.
pub const MAX_FRAME: usize = 256 * 1024;

/// What the helper sends to the meter, on its standard output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    Packet(Segment),
    /// A line for the meter's log.
    Log(Level, String),
    /// Sent once the capture handles are open.
    Status(Status),
    /// The labels of the devices worth capturing on: once before `Status`,
    /// the devices found at start, then as the answer to `Control::ListDevices`.
    Devices(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// The helper had CAP_NET_RAW when it started.
    pub capable: bool,
    /// How many devices it opened.
    pub opened: u16,
    /// It gave up its capabilities after opening them.
    pub dropped: bool,
}

/// What the meter sends to the helper, on its standard input. The helper also
/// stops when its input ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Narrow the filter to the game's server port, or widen it to all TCP.
    SetFilterPort(Option<u16>),
    ListDevices,
    Stop,
}

const PACKET: u8 = 1;
const LOG: u8 = 2;
const STATUS: u8 = 3;
const DEVICES: u8 = 4;

const SET_FILTER_PORT: u8 = 1;
const LIST_DEVICES: u8 = 2;
const STOP: u8 = 3;

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// Read one frame's body. `Ok(None)` when the stream ends between frames; a
/// stream that ends inside a frame is an error.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match reader.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 {
        return Err(invalid("empty frame"));
    }
    if len > MAX_FRAME {
        return Err(invalid("frame too long"));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

/// A body with its length in front, ready to write.
fn frame(body: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    // Cut at a character boundary rather than write a length that lies.
    let mut end = s.len().min(u16::MAX as usize);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out.extend_from_slice(&(end as u16).to_le_bytes());
    out.extend_from_slice(&s.as_bytes()[..end]);
}

/// Reads fields from the front of a body.
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(invalid("frame too short"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn ip(&mut self) -> io::Result<[u8; 4]> {
        Ok(self.take(4)?.try_into().unwrap())
    }

    fn str(&mut self) -> io::Result<String> {
        let len = self.u16()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| invalid("string is not UTF-8"))
    }

    fn end(&self) -> io::Result<()> {
        if self.0.is_empty() { Ok(()) } else { Err(invalid("frame too long for its kind")) }
    }
}

fn level_byte(level: Level) -> u8 {
    match level {
        Level::Error => 1,
        Level::Warn => 2,
        Level::Info => 3,
    }
}

impl Report {
    /// The whole frame, length included.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Report::Packet(s) => {
                b.reserve(32 + s.device.len() + s.data.len());
                b.push(PACKET);
                b.extend_from_slice(&s.captured_at_ms.to_le_bytes());
                b.extend_from_slice(&s.src_ip);
                b.extend_from_slice(&s.dst_ip);
                b.extend_from_slice(&s.src_port.to_le_bytes());
                b.extend_from_slice(&s.dst_port.to_le_bytes());
                b.extend_from_slice(&s.seq.to_le_bytes());
                b.extend_from_slice(&s.ack.to_le_bytes());
                put_str(&mut b, &s.device);
                b.extend_from_slice(&s.data);
            }
            Report::Log(level, text) => {
                b.push(LOG);
                b.push(level_byte(*level));
                put_str(&mut b, text);
            }
            Report::Status(s) => {
                b.push(STATUS);
                b.push(s.capable as u8);
                b.extend_from_slice(&s.opened.to_le_bytes());
                b.push(s.dropped as u8);
            }
            Report::Devices(labels) => {
                b.push(DEVICES);
                let count_at = b.len();
                b.extend_from_slice(&0u16.to_le_bytes());
                let mut count = 0u16;
                for label in labels {
                    // A list too long for one frame is cut short.
                    if b.len() + 2 + label.len() > MAX_FRAME || count == u16::MAX {
                        break;
                    }
                    put_str(&mut b, label);
                    count += 1;
                }
                b[count_at..count_at + 2].copy_from_slice(&count.to_le_bytes());
            }
        }
        frame(b)
    }

    pub fn decode(body: &[u8]) -> io::Result<Self> {
        let mut f = Fields(body);
        let report = match f.u8()? {
            PACKET => {
                let captured_at_ms = f.i64()?;
                let src_ip = f.ip()?;
                let dst_ip = f.ip()?;
                let src_port = f.u16()?;
                let dst_port = f.u16()?;
                let seq = f.u32()?;
                let ack = f.u32()?;
                let device = f.str()?;
                let data = std::mem::take(&mut f.0).to_vec();
                Report::Packet(Segment { device, captured_at_ms, src_ip, dst_ip, src_port, dst_port, seq, ack, data })
            }
            LOG => {
                let level = match f.u8()? {
                    1 => Level::Error,
                    2 => Level::Warn,
                    _ => Level::Info,
                };
                Report::Log(level, f.str()?)
            }
            STATUS => Report::Status(Status { capable: f.u8()? != 0, opened: f.u16()?, dropped: f.u8()? != 0 }),
            DEVICES => {
                let count = f.u16()?;
                Report::Devices((0..count).map(|_| f.str()).collect::<io::Result<_>>()?)
            }
            _ => return Err(invalid("unknown report")),
        };
        f.end()?;
        Ok(report)
    }
}

impl Control {
    /// The whole frame, length included.
    pub fn encode(&self) -> Vec<u8> {
        frame(match self {
            Control::SetFilterPort(port) => {
                let mut b = vec![SET_FILTER_PORT];
                b.extend_from_slice(&port.unwrap_or(0).to_le_bytes());
                b
            }
            Control::ListDevices => vec![LIST_DEVICES],
            Control::Stop => vec![STOP],
        })
    }

    pub fn decode(body: &[u8]) -> io::Result<Self> {
        let mut f = Fields(body);
        let control = match f.u8()? {
            SET_FILTER_PORT => Control::SetFilterPort(Some(f.u16()?).filter(|&p| p != 0)),
            LIST_DEVICES => Control::ListDevices,
            STOP => Control::Stop,
            _ => return Err(invalid("unknown control")),
        };
        f.end()?;
        Ok(control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment() -> Segment {
        Segment {
            device: "enp5s0".into(),
            captured_at_ms: 1_791_000_000_123,
            src_ip: [193, 202, 112, 99],
            dst_ip: [192, 168, 1, 20],
            src_port: 13328,
            dst_port: 51000,
            seq: 0xDEAD_BEEF,
            ack: 7,
            data: vec![0x34, 0x21, 0x36, 0, 1, 2, 3],
        }
    }

    fn reports() -> Vec<Report> {
        vec![
            Report::Packet(segment()),
            Report::Packet(Segment { data: vec![0xAB; 65535], device: "Adapter for loopback".into(), ..segment() }),
            Report::Log(Level::Warn, "Capture filter on lo: tcp port 13328".into()),
            Report::Log(Level::Error, String::new()),
            Report::Status(Status { capable: true, opened: 3, dropped: true }),
            Report::Devices(vec!["lo".into(), "enp5s0".into(), "wg0".into()]),
            Report::Devices(vec![]),
        ]
    }

    /// Hands out at most `step` bytes per read, as a pipe may.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.step.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn every_report_comes_back_as_sent() {
        for report in reports() {
            let bytes = report.encode();
            let body = read_frame(&mut bytes.as_slice()).unwrap().unwrap();
            assert_eq!(Report::decode(&body).unwrap(), report);
        }
    }

    #[test]
    fn every_control_comes_back_as_sent() {
        for control in [
            Control::SetFilterPort(Some(13328)),
            Control::SetFilterPort(None),
            Control::ListDevices,
            Control::Stop,
        ] {
            let bytes = control.encode();
            let body = read_frame(&mut bytes.as_slice()).unwrap().unwrap();
            assert_eq!(Control::decode(&body).unwrap(), control);
        }
    }

    #[test]
    fn frames_survive_reads_of_any_size() {
        let stream: Vec<u8> = reports().iter().flat_map(Report::encode).collect();
        for step in [1, 2, 3, 5, 4096] {
            let mut reader = Trickle { data: &stream, step };
            let mut got = Vec::new();
            while let Some(body) = read_frame(&mut reader).unwrap() {
                got.push(Report::decode(&body).unwrap());
            }
            assert_eq!(got, reports(), "reads of {step} bytes");
        }
    }

    #[test]
    fn a_stream_cut_inside_a_frame_is_an_error() {
        let bytes = Report::Packet(segment()).encode();
        for cut in [1, 3, 4, 10, bytes.len() - 1] {
            let err = read_frame(&mut &bytes[..cut]).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
        assert!(read_frame(&mut &[][..]).unwrap().is_none(), "end between frames");
    }

    #[test]
    fn an_oversized_frame_is_refused_before_reading_it() {
        let mut bytes = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[PACKET; 16]);
        let err = read_frame(&mut bytes.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let huge = u32::MAX.to_le_bytes();
        assert_eq!(read_frame(&mut huge.as_slice()).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let empty = 0u32.to_le_bytes();
        assert_eq!(read_frame(&mut empty.as_slice()).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn broken_bodies_are_refused() {
        assert!(Report::decode(&[]).is_err());
        assert!(Report::decode(&[9]).is_err(), "unknown kind");
        assert!(Report::decode(&[STATUS, 1, 0]).is_err(), "too short");
        assert!(Report::decode(&[STATUS, 1, 0, 0, 0, 0]).is_err(), "too long");
        assert!(Report::decode(&[LOG, 3, 2, 0, 0xFF, 0xFE]).is_err(), "not UTF-8");
        assert!(Control::decode(&[SET_FILTER_PORT, 1]).is_err(), "port cut short");
        assert!(Control::decode(&[STOP, 0]).is_err(), "stop with a tail");
        assert!(Control::decode(&[0]).is_err(), "unknown control");
    }

    #[test]
    fn long_strings_are_cut_at_a_character_boundary() {
        let text = "é".repeat(40_000); // 80,000 bytes
        let Report::Log(_, back) = Report::decode(&read_frame(&mut Report::Log(Level::Info, text).encode().as_slice()).unwrap().unwrap()).unwrap() else {
            panic!("not a log line");
        };
        assert!(back.len() <= u16::MAX as usize);
        assert!(back.chars().all(|c| c == 'é'));
    }
}
