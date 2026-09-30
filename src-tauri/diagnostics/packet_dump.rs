// Standalone packet-capture diagnostic for A2Tools DPS Meter.
//
// Purpose: after an AION 2 game update, the meter stops detecting packets.
// The built-in packet logger only runs AFTER the combat port locks (which
// itself requires the MAGIC signature 06 00 36), so it cannot diagnose a
// signature/opcode change. This tool taps the RAW pcap stream before any
// filtering and dumps everything, plus a live per-flow summary that shows
// which connection is the game and whether the known signatures still appear.
//
// Run from an ADMIN terminal (Npcap requires it), with the game running:
//     cargo run --bin packet_dump
// or run the built exe directly (as admin):
//     target\debug\packet_dump.exe
//
// Go fight something for ~30s, then press Ctrl+C. Send me the printed
// summary and the dump file path.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use a2tools_dps_meter_lib::capture::captured_payload::CapturedPayload;
use a2tools_dps_meter_lib::capture::pcap_capturer::PcapCapturer;
use tokio::sync::mpsc;

// Known protocol signatures (pre-update) we want to detect presence of.
const MAGIC: [u8; 3] = [0x06, 0x00, 0x36]; // combat port-lock signature
const SIG_DAMAGE: [u8; 2] = [0x04, 0x38]; // damage packet opcode
const SIG_DOT: [u8; 2] = [0x05, 0x38]; // damage-over-time
const SIG_DEATH: [u8; 2] = [0x41, 0x36]; // death event
const SIG_HP: [u8; 2] = [0x1B, 0x92]; // hp/mp update
const SIG_SPAWN: [u8; 2] = [0x40, 0x36]; // mob/summon spawn
const SIG_BUNDLE: [u8; 2] = [0xFF, 0xFF]; // lz4 compressed bundle

const TLS_CONTENT_TYPES: [u8; 4] = [0x14, 0x15, 0x16, 0x17];
const TLS_VERSIONS: [u8; 5] = [0x00, 0x01, 0x02, 0x03, 0x04];

const MAX_DUMP_BYTES: u64 = 80 * 1024 * 1024; // safety cap on dump file

#[derive(Default, Clone)]
struct FlowStats {
    device: String,
    packets: u64,
    bytes: u64,
    tls_packets: u64,
    has_magic: u64,
    has_damage: u64,
    has_dot: u64,
    has_death: u64,
    has_hp: u64,
    has_spawn: u64,
    has_bundle: u64,
    first_seen: i64,
    last_seen: i64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn looks_like_tls(data: &[u8]) -> bool {
    if data.len() < 3 {
        return false;
    }
    TLS_CONTENT_TYPES.contains(&data[0]) && data[1] == 0x03 && TLS_VERSIONS.contains(&data[2])
}

fn contains(data: &[u8], needle: &[u8]) -> bool {
    if needle.len() > data.len() {
        return false;
    }
    data.windows(needle.len()).any(|w| w == needle)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02X}", b));
    }
    s
}

#[tokio::main]
async fn main() {
    // Console logging so PcapCapturer's device-list / "Capture active" lines show.
    tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .init();

    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dump_path = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join(format!("rawpackets_{}.txt", stamp));

    let file = match std::fs::File::create(&dump_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("FATAL: could not create dump file {}: {}", dump_path.display(), e);
            return;
        }
    };
    let mut dump = std::io::BufWriter::new(file);
    let _ = writeln!(
        dump,
        "# A2Tools raw packet dump {}\n# Format: TIMESTAMP_MS|SRCIP:SRCPORT->DSTIP:DSTPORT|DEV|TLS/PLAIN|LEN|HEX\n",
        chrono::Local::now().format("%+")
    );

    println!("\n========================================================");
    println!(" A2Tools DPS Meter — RAW packet capture (diagnostic)");
    println!("========================================================");
    println!(" Dump file: {}", dump_path.display());
    println!(" 1. Make sure AION 2 is running.");
    println!(" 2. Go fight something and deal damage for ~30s.");
    println!(" 3. Press Ctrl+C to stop, then send me the summary + dump file.");
    println!("--------------------------------------------------------\n");

    let (tx, mut rx) = mpsc::channel::<CapturedPayload>(8192);
    let capturer = PcapCapturer::new(tx);
    capturer.start();

    // Flush + final summary on Ctrl+C (handler flips a global flag).
    install_ctrlc_handler();
    let stop = &STOP_FLAG;

    let mut flows: HashMap<(String, u16, String, u16), FlowStats> = HashMap::new();
    let mut dumped_bytes: u64 = 0;
    let mut dump_capped = false;
    let mut last_summary = Instant::now();
    let mut total_packets: u64 = 0;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let cap = match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(c)) => c,
            Ok(None) => break, // channel closed
            Err(_) => {
                // timeout: periodic summary even when idle
                if last_summary.elapsed() >= Duration::from_secs(3) {
                    print_summary(&flows, total_packets, dumped_bytes, dump_capped);
                    last_summary = Instant::now();
                }
                continue;
            }
        };

        total_packets += 1;
        let src_ip = cap.src_ip.clone().unwrap_or_default();
        let dst_ip = cap.dst_ip.clone().unwrap_or_default();
        let dev = cap.device_name.clone().unwrap_or_default();
        let is_tls = looks_like_tls(&cap.data);

        let key = (src_ip.clone(), cap.src_port, dst_ip.clone(), cap.dst_port);
        let st = flows.entry(key).or_insert_with(|| {
            let mut s = FlowStats::default();
            s.device = dev.clone();
            s.first_seen = now_ms();
            s
        });
        st.packets += 1;
        st.bytes += cap.data.len() as u64;
        st.last_seen = now_ms();
        if is_tls {
            st.tls_packets += 1;
        } else {
            if contains(&cap.data, &MAGIC) { st.has_magic += 1; }
            if contains(&cap.data, &SIG_DAMAGE) { st.has_damage += 1; }
            if contains(&cap.data, &SIG_DOT) { st.has_dot += 1; }
            if contains(&cap.data, &SIG_DEATH) { st.has_death += 1; }
            if contains(&cap.data, &SIG_HP) { st.has_hp += 1; }
            if contains(&cap.data, &SIG_SPAWN) { st.has_spawn += 1; }
            if contains(&cap.data, &SIG_BUNDLE) { st.has_bundle += 1; }
        }

        // Dump non-TLS payloads (TLS is encrypted = useless hex, only count it).
        if !is_tls && !dump_capped {
            let line = format!(
                "{}|{}:{}->{}:{}|{}|PLAIN|{}|{}\n",
                cap.captured_at_ms,
                src_ip, cap.src_port, dst_ip, cap.dst_port,
                dev, cap.data.len(), to_hex(&cap.data)
            );
            if dump.write_all(line.as_bytes()).is_ok() {
                let _ = dump.flush();
                dumped_bytes += line.len() as u64;
                if dumped_bytes >= MAX_DUMP_BYTES {
                    dump_capped = true;
                    println!("\n[!] Dump file hit {} MB cap — stopping further writes (stats still live).\n", MAX_DUMP_BYTES / (1024 * 1024));
                }
            }
        }

        if last_summary.elapsed() >= Duration::from_secs(3) {
            print_summary(&flows, total_packets, dumped_bytes, dump_capped);
            last_summary = Instant::now();
        }
    }

    capturer.stop();
    let _ = dump.flush();

    println!("\n\n================= FINAL SUMMARY =================");
    print_summary(&flows, total_packets, dumped_bytes, dump_capped);
    println!("\nDump file written to:\n  {}", dump_path.display());
    println!("Send me that file + the table above.\n");
}

fn print_summary(
    flows: &HashMap<(String, u16, String, u16), FlowStats>,
    total_packets: u64,
    dumped_bytes: u64,
    capped: bool,
) {
    let mut rows: Vec<(&(String, u16, String, u16), &FlowStats)> = flows.iter().collect();
    rows.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes));

    println!(
        "\n--- flows: {} | total pkts: {} | dumped: {} KB{} ---",
        flows.len(),
        total_packets,
        dumped_bytes / 1024,
        if capped { " (CAPPED)" } else { "" }
    );
    println!(
        "{:<42} {:>6} {:>9} {:>5} {:>5} {:>4} {:>4} {:>4} {:>4} {:>4} {:>4}",
        "flow (src->dst)", "pkts", "bytes", "TLS", "MAG", "DMG", "DOT", "DTH", "HP", "SPN", "BND"
    );
    for (k, s) in rows.iter().take(18) {
        let flow = format!("{}:{}->{}:{}", k.0, k.1, k.2, k.3);
        let flow = if flow.len() > 42 { flow[flow.len() - 42..].to_string() } else { flow };
        println!(
            "{:<42} {:>6} {:>9} {:>5} {:>5} {:>4} {:>4} {:>4} {:>4} {:>4} {:>4}",
            flow, s.packets, s.bytes, s.tls_packets,
            s.has_magic, s.has_damage, s.has_dot, s.has_death, s.has_hp, s.has_spawn, s.has_bundle
        );
    }
    println!("(MAG=06 00 36 lock sig, DMG=04 38, DOT=05 38, DTH=41 36, HP=1B 92, SPN=40 36, BND=FF FF)");
}

// Global stop flag; the Ctrl+C handler sets it, the main loop polls it.
static STOP_FLAG: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
fn install_ctrlc_handler() {
    unsafe extern "system" fn handler(_ctrl_type: u32) -> i32 {
        STOP_FLAG.store(true, Ordering::SeqCst);
        1 // TRUE: handled (don't terminate immediately; let main flush & exit)
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(windows))]
fn install_ctrlc_handler() {}
