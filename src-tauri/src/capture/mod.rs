// Parser core — compiled for wasm32 as well as the desktop app.
pub mod abnormal;
pub mod captured_payload;
pub mod evidence_slice;
pub mod framing;
mod names;
mod opcodes;
pub mod packet_accumulator;
pub mod report_log;
pub mod stream_assembler;
pub mod stream_processor;
pub mod varint;

#[cfg(test)]
mod replay_report;
#[cfg(all(test, feature = "backend"))]
mod record_check;

// Live capture. pcap needs libloading, the port detector reads the wall clock,
// and the file replay drives them both — none of which exist on wasm32.
#[cfg(feature = "backend")]
pub mod combat_port_detector;
#[cfg(feature = "backend")]
pub mod file_replay;
#[cfg(feature = "backend")]
mod helper_process;
#[cfg(feature = "backend")]
pub mod live;
#[cfg(feature = "backend")]
pub mod pcap_capturer;
