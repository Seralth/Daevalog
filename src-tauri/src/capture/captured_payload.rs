/// Raw TCP payload captured from pcap.
#[derive(Debug, Clone)]
pub struct CapturedPayload {
    pub src_port: u16,
    pub dst_port: u16,
    pub data: Vec<u8>,
    pub device_name: Option<String>,
    pub captured_at_ms: i64,
    pub src_ip: Option<String>,
    pub dst_ip: Option<String>,
    pub tcp_seq: u32,
    pub tcp_ack: u32,
}

/// The stream name a server-to-client segment is logged and sliced under:
/// `Client:<client port>:<server port>`. One name per connection, so a second
/// connection from the same server port (the game server also talks TLS from
/// it) never runs into the game's bytes. Readers take the server port from
/// after the last `:`, as they did from the older `Client:<server port>`.
pub fn stream_key(server_port: u16, client_port: u16) -> String {
    format!("Client:{client_port}:{server_port}")
}
