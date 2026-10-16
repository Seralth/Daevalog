//! libpcap, loaded at run time: devices, capture handles, the packet filter and
//! the TCP payload in each frame. No SDK is needed at compile time.

use std::ffi::{c_char, c_int, c_long, c_uint, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use libloading::{Library, Symbol};

use crate::{log, now_ms, Level, Segment};

// ===== libpcap's C types =====

/// A `pcap_t *`. libpcap never shows what is behind it.
type RawPcap = *mut c_void;

/// One entry of the device list from `pcap_findalldevs` (`struct pcap_if`).
#[repr(C)]
struct DevNode {
    next: *mut DevNode,
    name: *const c_char,
    description: *const c_char,
    /// A `struct pcap_addr` list. Only whether it is null is read.
    addresses: *mut c_void,
    flags: u32,
}

/// Bit 0 of `pcap_if.flags`.
const LOOPBACK_FLAG: u32 = 0x0000_0001;

/// `struct timeval`. C `long` is 32 bits on Windows and 64 on 64-bit Linux.
#[repr(C)]
struct WhenSeen {
    sec: c_long,
    usec: c_long,
}

/// `struct pcap_pkthdr`.
#[repr(C)]
struct PktHeader {
    ts: WhenSeen,
    caplen: u32,
    len: u32,
}

const ERRBUF_LEN: usize = 256;

/// `struct pcap_stat`. Windows has three more fields, which `pcap_stats`
/// leaves alone; the room for them is there anyway.
#[repr(C)]
#[derive(Default)]
struct PcapStat {
    ps_recv: c_uint,
    ps_drop: c_uint,
    ps_ifdrop: c_uint,
    _windows: [c_uint; 3],
}

/// How often each capture thread looks at its kernel drop count.
const STATS_MS: i64 = 60_000;

/// `struct bpf_program`, filled by `pcap_compile`.
#[repr(C)]
struct BpfProgram {
    bf_len: c_uint,
    bf_insns: *mut std::ffi::c_void,
}

const PCAP_NETMASK_UNKNOWN: c_uint = 0xffffffff;

/// libpcap as nearly every Linux distribution ships it, and the devices not
/// worth opening there. Here rather than in the meter so the helper has them.
pub mod linux {
    /// The kernel buffer of each capture. libpcap cuts it into 256 KiB blocks
    /// and hands one over at each read timeout (100 ms), however little is in
    /// it, so its default 2 MiB lasts 0.8 s when the reader falls behind.
    /// This lasts 3.2 s.
    pub const BUFFER_BYTES: usize = 8 << 20;

    /// Sonames to try, first match wins: Arch and Fedora have `.so.1`,
    /// Debian and Ubuntu only `.so.0.8` (plain `.so` comes with -dev).
    pub const LIBRARIES: &[&str] = &["libpcap.so.1", "libpcap.so.0.8", "libpcap.so"];

    /// Device name starts that never carry the game's traffic.
    const NOT_GAME_TRAFFIC: [&str; 9] =
        ["nflog", "nfqueue", "usbmon", "dbus", "bluetooth", "veth", "docker", "br-", "podman"];

    pub(super) fn skipped_by_name(name: &str) -> bool {
        name == "any" || NOT_GAME_TRAFFIC.iter().any(|start| name.starts_with(start))
    }

    /// A Linux bridge under any name, such as a container network.
    fn is_bridge(name: &str) -> bool {
        std::path::Path::new("/sys/class/net").join(name).join("bridge").is_dir()
    }

    /// Devices not worth a capture thread: pseudo devices, container and
    /// bridge links, and ones that are down.
    pub fn skip_device(name: &str) -> bool {
        skipped_by_name(name) || is_bridge(name) || is_down(name)
    }

    /// The device is down (IFF_UP clear). It keeps its addresses, but a
    /// capture on it fails, so it counts as gone until it is up again: a
    /// Wi-Fi or VPN link that went down shows as a change of the list.
    fn is_down(name: &str) -> bool {
        std::fs::read_to_string(std::path::Path::new("/sys/class/net").join(name).join("flags"))
            .ok()
            .and_then(|flags| u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16).ok())
            .is_some_and(|flags| flags & 1 == 0)
    }
}

/// The game's server port while it is locked (0 = none), and when a packet on
/// it was last captured. Capture threads narrow their filter to that port.
static FILTER_PORT: AtomicU32 = AtomicU32::new(0);
static FILTER_PORT_SEEN_MS: AtomicI64 = AtomicI64::new(0);

/// With no packet on the locked port for this long, the filter widens back to
/// all TCP, so the dispatcher sees traffic again and can drop a dead lock.
pub const FILTER_PORT_QUIET_MS: i64 = 10_000;

/// How often the watcher looks for capture threads with an outdated filter.
const FILTER_WATCH_MS: u64 = 1_000;

/// An open capture handle and the filter port it has set (0 = plain `tcp`).
struct LiveHandle {
    handle: usize,
    applied: Arc<AtomicU32>,
}

static LIVE_HANDLES: Mutex<Vec<LiveHandle>> = Mutex::new(Vec::new());

fn live_handles() -> MutexGuard<'static, Vec<LiveHandle>> {
    LIVE_HANDLES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Called when the combat port locks (`Some`) or the lock is cleared (`None`).
pub fn set_filter_port(port: Option<u16>) {
    FILTER_PORT_SEEN_MS.store(now_ms(), Ordering::Relaxed);
    FILTER_PORT.store(port.map_or(0, u32::from), Ordering::Relaxed);
}

fn note_filter_port_packet(segment: &Segment) {
    let port = FILTER_PORT.load(Ordering::Relaxed) as u16;
    if port != 0 && (segment.src_port == port || segment.dst_port == port) {
        FILTER_PORT_SEEN_MS.store(now_ms(), Ordering::Relaxed);
    }
}

/// The port the filter should be narrowed to now, if any.
pub fn wanted_filter_port(now: i64) -> Option<u16> {
    let port = FILTER_PORT.load(Ordering::Relaxed) as u16;
    let quiet = now - FILTER_PORT_SEEN_MS.load(Ordering::Relaxed) >= FILTER_PORT_QUIET_MS;
    (port != 0 && !quiet).then_some(port)
}

/// Filter expressions to try, in order: VLAN-tagged frames too (the parser
/// reads up to two tags), then plain, for link types that have no VLAN.
fn filter_exprs(port: Option<u16>) -> [String; 2] {
    let tcp = match port {
        Some(p) => format!("tcp port {p}"),
        None => "tcp".to_string(),
    };
    [format!("{tcp} or (vlan and ({tcp} or (vlan and {tcp})))"), tcp]
}

/// Wakes capture threads whose filter no longer matches the wanted port. An
/// idle Linux device can sit in `pcap_next_ex` well past its timeout, and
/// only its own thread may change its filter.
pub fn start_filter_watcher(pcap: Arc<PcapLib>, running: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(FILTER_WATCH_MS));
            let want = wanted_filter_port(now_ms()).map_or(0, u32::from);
            // Holding the list lock: `run` takes a handle off it before closing.
            for entry in live_handles().iter() {
                if entry.applied.load(Ordering::Relaxed) != want {
                    // SAFETY: the handle is still open while it is listed.
                    unsafe { (pcap.calls.breakloop)(entry.handle as RawPcap) };
                }
            }
        }
    });
}

// ===== Device info =====

#[derive(Clone)]
pub struct NetDevice {
    pub name: String,
    pub description: String,
    pub has_addresses: bool,
    pub is_loopback: bool,
}

impl NetDevice {
    /// The name shown and stored everywhere: the description when there is
    /// one (Windows names are GUIDs), else the name.
    pub fn label(&self) -> &str {
        if self.description.is_empty() { &self.name } else { &self.description }
    }

    /// Loopback, or a tunnel or loopback adapter going by its words.
    pub fn software_only(&self) -> bool {
        if self.is_loopback || self.name.to_lowercase().contains("loopback") {
            return true;
        }
        let label = self.label().to_lowercase();
        ["loopback", "tap", "wintun", "wireguard"].iter().any(|word| label.contains(word))
    }
}

/// Logs the devices about to be opened.
pub fn log_devices(devices: &[NetDevice]) {
    log(Level::Info, &format!("Found {} capture devices", devices.len()));
    for (index, device) in devices.iter().enumerate() {
        log(
            Level::Info,
            &format!(
                "  [{index}] {} (loopback={}, addresses={})",
                device.label(),
                device.is_loopback,
                device.has_addresses
            ),
        );
    }
}

/// Where captured segments go.
pub trait Sink {
    fn packet(&mut self, segment: Segment);
    /// The read timed out with no packet.
    fn idle(&mut self) {}
}

/// A capture handle opened by `PcapLib::open`, for `PcapLib::run`.
pub struct Live {
    handle: usize,
    label: String,
    link_type: c_int,
    reads_first: bool,
    applied: Arc<AtomicU32>,
}

impl Live {
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Read from the start, not after the physical devices' delay.
    pub fn reads_first(&self) -> bool {
        self.reads_first
    }
}

/// Whether a device is read from the start: loopback, a device named as a
/// tunnel, or one with no link layer. Linux names a tunnel freely (`wg0`,
/// `vpn0`, `tun0`) but gives it libpcap's raw IP link type. Through a VPN the
/// game's TCP stream exists only on the tunnel; the physical device carries
/// it encrypted, as UDP. A ping reducer's loopback relay still wins: the
/// meter's port detector locks a loopback flow at once and makes any other
/// flow wait 2.5 s.
pub fn reads_first(device: &NetDevice, link_type: c_int) -> bool {
    device.software_only() || is_raw_ip(link_type)
}

/// Link types whose frames start at the IP header.
fn is_raw_ip(link_type: c_int) -> bool {
    matches!(link_type, 12 | 14 | 101 | 228)
}

// ===== Pcap library wrapper =====

/// The calls that open a handle with a chosen buffer size. Old libraries lack
/// them; `pcap_open_live` does without.
struct Create {
    create: unsafe extern "C" fn(*const c_char, *mut c_char) -> RawPcap,
    set_snaplen: unsafe extern "C" fn(RawPcap, c_int) -> c_int,
    set_promisc: unsafe extern "C" fn(RawPcap, c_int) -> c_int,
    set_timeout: unsafe extern "C" fn(RawPcap, c_int) -> c_int,
    set_buffer_size: unsafe extern "C" fn(RawPcap, c_int) -> c_int,
    activate: unsafe extern "C" fn(RawPcap) -> c_int,
}

const SNAPLEN: c_int = 65535;
/// Not promiscuous: only this machine's own traffic.
const PROMISC: c_int = 0;
const TIMEOUT_MS: c_int = 100;

/// After a failed filter change, the next try waits this long.
const FILTER_RETRY_MS: i64 = 1_000;

/// The calls every libpcap has.
struct Calls {
    findalldevs: unsafe extern "C" fn(*mut *mut DevNode, *mut c_char) -> c_int,
    freealldevs: unsafe extern "C" fn(*mut DevNode),
    open_live: unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> RawPcap,
    close: unsafe extern "C" fn(RawPcap),
    next_ex: unsafe extern "C" fn(RawPcap, *mut *mut PktHeader, *mut *const u8) -> c_int,
    datalink: unsafe extern "C" fn(RawPcap) -> c_int,
    compile: unsafe extern "C" fn(RawPcap, *mut BpfProgram, *const c_char, c_int, c_uint) -> c_int,
    setfilter: unsafe extern "C" fn(RawPcap, *mut BpfProgram) -> c_int,
    freecode: unsafe extern "C" fn(*mut BpfProgram),
    geterr: unsafe extern "C" fn(RawPcap) -> *const c_char,
    breakloop: unsafe extern "C" fn(RawPcap),
}

/// The loaded library. Only plain function pointers are kept, so it is
/// `Send + Sync` by itself; `_library` keeps them valid.
pub struct PcapLib {
    calls: Calls,
    stats: Option<unsafe extern "C" fn(RawPcap, *mut PcapStat) -> c_int>,
    create: Option<Create>,
    /// Older libpcap's `pcap_compile` is not thread-safe.
    compiling: Mutex<()>,
    _library: Library,
}

/// One symbol, copied out of the library. `T` must be the fn pointer type
/// that matches the C signature.
fn symbol<T: Copy>(library: &Library, name: &str) -> Result<T, String> {
    // SAFETY: the caller names the C signature; the pointer lives as long as
    // `library`, which `PcapLib` keeps.
    let found: Symbol<T> = unsafe { library.get(name.as_bytes()) }
        .map_err(|e| format!("libpcap has no {name}: {e}"))?;
    Ok(*found)
}

/// Text from a C string that may be null.
fn c_text(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: libpcap hands out NUL-terminated strings.
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

/// Text in an error buffer, up to its NUL.
fn errbuf_text(buf: &[c_char; ERRBUF_LEN]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A packet time from libpcap in ms, or `None` when it is not a plausible
/// wall clock time (2001 to 2033).
fn stamp_ms(sec: i64, usec: i64) -> Option<i64> {
    let ms = sec.checked_mul(1000)?.checked_add(usec / 1000)?;
    (1_000_000_000_000 < ms && ms < 2_000_000_000_000).then_some(ms)
}

impl PcapLib {
    /// Loads the first of `libraries` that loads at all.
    pub fn load(libraries: &[&str], help: &str) -> Result<Self, String> {
        let mut failures = Vec::new();
        let first_loaded = libraries.iter().find_map(|name| {
            // SAFETY: loading libpcap runs no code of ours.
            unsafe { Library::new(name) }.map_err(|e| failures.push(format!("{name}: {e}"))).ok()
        });
        let Some(library) = first_loaded else {
            return Err(format!("Could not load {}. {help} ({})", libraries.join(" or "), failures.join("; ")));
        };
        let calls = Calls {
            findalldevs: symbol(&library, "pcap_findalldevs")?,
            freealldevs: symbol(&library, "pcap_freealldevs")?,
            open_live: symbol(&library, "pcap_open_live")?,
            close: symbol(&library, "pcap_close")?,
            next_ex: symbol(&library, "pcap_next_ex")?,
            datalink: symbol(&library, "pcap_datalink")?,
            compile: symbol(&library, "pcap_compile")?,
            setfilter: symbol(&library, "pcap_setfilter")?,
            freecode: symbol(&library, "pcap_freecode")?,
            geterr: symbol(&library, "pcap_geterr")?,
            breakloop: symbol(&library, "pcap_breakloop")?,
        };
        let stats = symbol(&library, "pcap_stats").ok();
        let create = (|| {
            Some(Create {
                create: symbol(&library, "pcap_create").ok()?,
                set_snaplen: symbol(&library, "pcap_set_snaplen").ok()?,
                set_promisc: symbol(&library, "pcap_set_promisc").ok()?,
                set_timeout: symbol(&library, "pcap_set_timeout").ok()?,
                set_buffer_size: symbol(&library, "pcap_set_buffer_size").ok()?,
                activate: symbol(&library, "pcap_activate").ok()?,
            })
        })();
        Ok(PcapLib { calls, stats, create, compiling: Mutex::new(()), _library: library })
    }

    /// Devices with addresses that `skip` keeps, in libpcap's order.
    pub fn capture_devices(&self, skip: fn(&str) -> bool) -> Result<Vec<NetDevice>, String> {
        let mut errbuf = [0 as c_char; ERRBUF_LEN];
        let mut first: *mut DevNode = ptr::null_mut();
        // SAFETY: both pointers are valid for the call.
        let status = unsafe { (self.calls.findalldevs)(&mut first, errbuf.as_mut_ptr()) };
        if status != 0 || first.is_null() {
            if !first.is_null() {
                // SAFETY: the list came from pcap_findalldevs.
                unsafe { (self.calls.freealldevs)(first) };
            }
            return Err(format!("Failed to list devices: {}", errbuf_text(&errbuf)));
        }
        let mut found = Vec::new();
        let mut node = first;
        while !node.is_null() {
            // SAFETY: a node of the list, freed only below.
            let entry = unsafe { &*node };
            found.push(NetDevice {
                name: c_text(entry.name),
                description: c_text(entry.description),
                has_addresses: !entry.addresses.is_null(),
                is_loopback: entry.flags & LOOPBACK_FLAG != 0,
            });
            node = entry.next;
        }
        // SAFETY: the list came from pcap_findalldevs and is not used again.
        unsafe { (self.calls.freealldevs)(first) };
        found.retain(|device| device.has_addresses && !skip(&device.name));
        Ok(found)
    }

    /// Opens a capture on `device`, with a kernel buffer of `buffer_bytes`
    /// when given and the library can set one.
    pub fn open(&self, device: &NetDevice, buffer_bytes: Option<usize>) -> Result<Live, String> {
        let name = CString::new(device.name.as_str())
            .map_err(|_| format!("device name {:?} holds a NUL byte", device.name))?;
        let mut errbuf = [0 as c_char; ERRBUF_LEN];
        let sized = buffer_bytes.zip(self.create.as_ref());
        let handle = match sized {
            Some((bytes, create)) => {
                // SAFETY: valid C string and error buffer; the handle is
                // closed on every failure below.
                unsafe {
                    let handle = (create.create)(name.as_ptr(), errbuf.as_mut_ptr());
                    if handle.is_null() {
                        return Err(format!("pcap_create failed: {}", errbuf_text(&errbuf)));
                    }
                    (create.set_snaplen)(handle, SNAPLEN);
                    (create.set_promisc)(handle, PROMISC);
                    (create.set_timeout)(handle, TIMEOUT_MS);
                    (create.set_buffer_size)(handle, bytes.min(i32::MAX as usize) as c_int);
                    let status = (create.activate)(handle);
                    if status < 0 {
                        let reason = self.handle_error(handle);
                        (self.calls.close)(handle);
                        return Err(format!("pcap_activate failed ({status}): {reason}"));
                    }
                    handle
                }
            }
            None => {
                // SAFETY: valid C string and error buffer.
                let handle = unsafe {
                    (self.calls.open_live)(name.as_ptr(), SNAPLEN, PROMISC, TIMEOUT_MS, errbuf.as_mut_ptr())
                };
                if handle.is_null() {
                    return Err(format!("pcap_open_live failed: {}", errbuf_text(&errbuf)));
                }
                handle
            }
        };

        // SAFETY: an open handle.
        let link_type = unsafe { (self.calls.datalink)(handle) };
        let label = device.label().to_string();
        let filter = self.apply_filter(handle, None).unwrap_or_else(|e| {
            log(Level::Warn, &format!("No capture filter on {label}, reading everything: {e}"));
            "none".to_string()
        });
        let buffer = match sized {
            Some((bytes, _)) => format!(", buffer {} KiB", bytes / 1024),
            None => String::new(),
        };
        log(Level::Info, &format!("Capture active on {label} (link type {link_type}, filter {filter}{buffer})"));

        let applied = Arc::new(AtomicU32::new(0));
        live_handles().push(LiveHandle { handle: handle as usize, applied: applied.clone() });
        Ok(Live { handle: handle as usize, label, link_type, reads_first: reads_first(device, link_type), applied })
    }

    /// Sets the filter for `port`: the VLAN form if it compiles, else the
    /// plain one. Gives the expression set.
    fn apply_filter(&self, handle: RawPcap, port: Option<u16>) -> Result<String, String> {
        let mut errors = Vec::new();
        for expr in filter_exprs(port) {
            let text = CString::new(expr.as_str()).expect("filter expressions hold no NUL");
            let mut program = BpfProgram { bf_len: 0, bf_insns: ptr::null_mut() };
            let compiled = {
                let _turn = self.compiling.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: an open handle, a program to fill and a C string.
                unsafe { (self.calls.compile)(handle, &mut program, text.as_ptr(), 1, PCAP_NETMASK_UNKNOWN) }
            };
            if compiled != 0 {
                errors.push(format!("{expr}: {}", self.handle_error(handle)));
                continue;
            }
            // SAFETY: the program was compiled for this handle and is freed once.
            let set = unsafe {
                let status = (self.calls.setfilter)(handle, &mut program);
                (self.calls.freecode)(&mut program);
                status
            };
            if set == 0 {
                return Ok(expr);
            }
            errors.push(format!("{expr}: {}", self.handle_error(handle)));
        }
        Err(errors.join("; "))
    }

    /// The handle's last error text (`pcap_geterr`).
    fn handle_error(&self, handle: RawPcap) -> String {
        // SAFETY: callers pass an open handle; the text lives until its next call.
        c_text(unsafe { (self.calls.geterr)(handle) })
    }

    /// Warns when the kernel dropped packets since the last look. `seen` is
    /// the drop total then.
    fn warn_drops(&self, handle: RawPcap, label: &str, seen: &mut u32) {
        let Some(stats) = self.stats else { return };
        let mut counts = PcapStat::default();
        // SAFETY: an open handle and a struct with room for every platform.
        if unsafe { stats(handle, &mut counts) } != 0 {
            return;
        }
        let fresh = counts.ps_drop.wrapping_sub(*seen);
        *seen = counts.ps_drop;
        if fresh > 0 {
            log(
                Level::Warn,
                &format!("{label}: the kernel dropped {fresh} packets in the last minute ({} received in all)", counts.ps_recv),
            );
        }
    }

    /// Reads `live` into `sink` until `running` is false or the device
    /// fails. True when the device failed (went down or away).
    pub fn run(&self, live: Live, running: &AtomicBool, skip_before_ms: i64, sink: &mut impl Sink) -> bool {
        let handle = live.handle as RawPcap;
        let mut next_drop_look = now_ms() + STATS_MS;
        let mut drops_seen = 0u32;
        let mut retry_at = i64::MIN;
        let mut warned_for: Option<u32> = None;
        let mut failed = false;

        while running.load(Ordering::Relaxed) {
            let now = now_ms();
            if now >= next_drop_look {
                next_drop_look = now + STATS_MS;
                self.warn_drops(handle, &live.label, &mut drops_seen);
            }

            let want = wanted_filter_port(now).map_or(0, u32::from);
            if want != live.applied.load(Ordering::Relaxed) && now >= retry_at {
                match self.apply_filter(handle, (want != 0).then_some(want as u16)) {
                    Ok(expr) => {
                        live.applied.store(want, Ordering::Relaxed);
                        retry_at = i64::MIN;
                        warned_for = None;
                        log(Level::Info, &format!("Capture filter on {}: {expr}", live.label));
                    }
                    Err(e) => {
                        retry_at = now + FILTER_RETRY_MS;
                        if warned_for != Some(want) {
                            warned_for = Some(want);
                            log(Level::Warn, &format!("Could not change the capture filter on {}: {e}", live.label));
                        }
                    }
                }
            }

            let mut header: *mut PktHeader = ptr::null_mut();
            let mut bytes: *const u8 = ptr::null();
            // SAFETY: an open handle; libpcap fills both pointers.
            match unsafe { (self.calls.next_ex)(handle, &mut header, &mut bytes) } {
                1 if !header.is_null() && !bytes.is_null() => {
                    // SAFETY: valid until the next pcap_next_ex on this handle.
                    let header = unsafe { &*header };
                    let at = stamp_ms(header.ts.sec as i64, header.ts.usec as i64).unwrap_or_else(now_ms);
                    if at < skip_before_ms {
                        continue;
                    }
                    // SAFETY: libpcap gives `caplen` bytes at `bytes`.
                    let frame = unsafe { std::slice::from_raw_parts(bytes, header.caplen as usize) };
                    if let Some(segment) = tcp_segment(frame, live.link_type, &live.label, at) {
                        note_filter_port_packet(&segment);
                        sink.packet(segment);
                    }
                }
                1 => {}
                0 => sink.idle(),
                // pcap_breakloop from the watcher: the filter is due a change.
                -2 => {}
                code => {
                    let reason = self.handle_error(handle);
                    log(Level::Warn, &format!("Capture on {} failed ({code}): {reason}", live.label));
                    failed = true;
                    break;
                }
            }
        }

        live_handles().retain(|entry| entry.handle != live.handle);
        // SAFETY: off the list, so the watcher no longer touches it.
        unsafe { (self.calls.close)(handle) };
        log(Level::Info, &format!("Capture stopped on {}", live.label));
        failed
    }
}

// ===== Frames =====

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

const ETHERTYPE_IPV4: u16 = 0x0800;
/// 802.1Q and 802.1ad (QinQ) tags.
const ETHERTYPE_VLAN: u16 = 0x8100;
const ETHERTYPE_QINQ: u16 = 0x88A8;
/// BSD loopback (`DLT_NULL`): AF_INET as a little-endian u32.
const NULL_AF_INET: [u8; 4] = [2, 0, 0, 0];

/// Link types from `dlt.h` (the raw IP ones are in `is_raw_ip`).
const DLT_NULL: c_int = 0;
const DLT_EN10MB: c_int = 1;
const DLT_LINUX_SLL: c_int = 113;
const DLT_LINUX_SLL2: c_int = 276;

/// Where IPv4 starts in a frame of `link_type`, going by the link header.
fn ipv4_start_for_link(frame: &[u8], link_type: c_int) -> Option<usize> {
    let start = match link_type {
        DLT_EN10MB => {
            let mut at = 12;
            for _ in 0..2 {
                if !matches!(be16(frame, at)?, ETHERTYPE_VLAN | ETHERTYPE_QINQ) {
                    break;
                }
                at += 4;
            }
            (be16(frame, at)? == ETHERTYPE_IPV4).then_some(at + 2)?
        }
        DLT_NULL => (frame.get(..4)? == NULL_AF_INET).then_some(4)?,
        t if is_raw_ip(t) => 0,
        DLT_LINUX_SLL => (be16(frame, 14)? == ETHERTYPE_IPV4).then_some(16)?,
        DLT_LINUX_SLL2 => (be16(frame, 0)? == ETHERTYPE_IPV4).then_some(20)?,
        _ => return None,
    };
    (frame.get(start)? >> 4 == 4).then_some(start)
}

/// Where IPv4 starts going by the bytes alone, for a link header that did
/// not fit its type.
fn ipv4_start_guessed(frame: &[u8]) -> Option<usize> {
    if frame.len() >= 14 {
        if be16(frame, 12) == Some(ETHERTYPE_IPV4) {
            return Some(14);
        }
        if frame[..4] == NULL_AF_INET {
            return Some(4);
        }
    }
    (frame.first()? >> 4 == 4).then_some(0)
}

/// The TCP segment in a captured frame, if it is IPv4 TCP with a payload.
fn tcp_segment(frame: &[u8], link_type: c_int, device: &str, captured_at_ms: i64) -> Option<Segment> {
    if frame.len() < 4 {
        return None;
    }
    let start = ipv4_start_for_link(frame, link_type).or_else(|| ipv4_start_guessed(frame))?;
    let ip = &frame[start..];
    if ip.len() < 20 || ip[0] >> 4 != 4 || ip[9] != 6 {
        return None;
    }
    let ip_len = usize::from(ip[0] & 0x0f) * 4;
    let end = match usize::from(be16(ip, 2)?) {
        // Segmentation offload leaves the length at 0.
        0 => ip.len(),
        total if total < ip_len + 20 => return None,
        // Past `total` is Ethernet padding; a cut capture ends sooner.
        total => total.min(ip.len()),
    };
    let tcp = ip.get(ip_len..)?;
    if tcp.len() < 20 {
        return None;
    }
    let data_from = ip_len + usize::from(tcp[12] >> 4) * 4;
    if data_from >= end {
        return None;
    }
    Some(Segment {
        device: device.to_string(),
        captured_at_ms,
        src_ip: ip[12..16].try_into().ok()?,
        dst_ip: ip[16..20].try_into().ok()?,
        src_port: be16(tcp, 0)?,
        dst_port: be16(tcp, 2)?,
        seq: be32(tcp, 4)?,
        ack: be32(tcp, 8)?,
        data: ip[data_from..end].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client's IPv4 TCP segment carrying "hi" to the game server.
    fn hi_packet() -> Vec<u8> {
        let mut p = vec![0x45, 0, 0, 0x2A, 0, 0, 0, 0, 0x40, 6, 0, 0, 10, 0, 0, 2, 0xC1, 0xCA, 0x70, 0x63];
        p.extend([0xC7, 0x38, 0x34, 0x10, 0, 0, 0, 1, 0, 0, 0, 0, 0x50, 0x18, 0, 0, 0, 0, 0, 0]);
        p.extend(b"hi");
        p
    }

    fn with_head(head: &[u8], body: &[u8]) -> Vec<u8> {
        [head, body].concat()
    }

    fn ether(ethertype: [u8; 2]) -> Vec<u8> {
        with_head(&[0; 12], &ethertype)
    }

    fn parse(frame: &[u8], link: c_int) -> Option<Segment> {
        tcp_segment(frame, link, "dev", 5)
    }

    fn assert_hi(found: Option<Segment>) {
        let s = found.expect("a segment");
        assert_eq!((s.src_port, s.dst_port), (51000, 13328));
        assert_eq!(s.src_ip, [10, 0, 0, 2]);
        assert_eq!(s.dst_ip, [193, 202, 112, 99]);
        assert_eq!((s.seq, s.ack), (1, 0));
        assert_eq!(s.data, b"hi");
        assert_eq!((s.device.as_str(), s.captured_at_ms), ("dev", 5));
    }

    fn device(name: &str, description: &str, is_loopback: bool) -> NetDevice {
        NetDevice { name: name.into(), description: description.into(), has_addresses: true, is_loopback }
    }

    #[test]
    fn link_types_locate_ipv4() {
        let p = hi_packet();
        let cases: [(Vec<u8>, c_int); 9] = [
            (ether([8, 0]), 1),
            (with_head(&ether([0x81, 0]), &[0, 5, 8, 0]), 1),
            (NULL_AF_INET.to_vec(), 0),
            (vec![], 12),
            (vec![], 14),
            (vec![], 101),
            (vec![], 228),
            (sll(), 113),
            (with_head(&[8, 0], &[0; 18]), 276),
        ];
        for (head, link) in cases {
            assert_hi(parse(&with_head(&head, &p), link));
        }
    }

    /// A Linux cooked (SLL) header carrying IPv4.
    fn sll() -> Vec<u8> {
        with_head(&[0; 14], &[8, 0])
    }

    #[test]
    fn unknown_link_uses_guess() {
        assert_hi(parse(&with_head(&ether([8, 0]), &hi_packet()), 999));
        assert_hi(parse(&hi_packet(), 1));
        assert_hi(parse(&with_head(&NULL_AF_INET, &hi_packet()), 999));
    }

    #[test]
    fn pad_bytes_not_payload() {
        let mut ack = vec![0x45, 0, 0, 0x28, 0, 0, 0, 0, 0x40, 6, 0, 0, 0xC1, 0xCA, 0x70, 0x63, 10, 0, 0, 2];
        ack.extend([0x34, 0x10, 0xC7, 0x38, 0, 0, 0, 1, 0, 0, 0, 2, 0x50, 0x10, 0, 0, 0, 0, 0, 0]);
        ack.extend([0; 6]);
        let frame = with_head(&ether([8, 0]), &ack);
        assert_eq!(frame.len(), 60);
        assert!(parse(&frame, 1).is_none());

        let padded = with_head(&ether([8, 0]), &with_head(&hi_packet(), &[0; 4]));
        assert_eq!(parse(&padded, 1).unwrap().data, b"hi");

        let mut no_length = hi_packet();
        no_length[2..4].copy_from_slice(&[0, 0]);
        assert_eq!(parse(&with_head(&ether([8, 0]), &no_length), 1).unwrap().data, b"hi");
    }

    #[test]
    fn non_ip_ethertype_gives_nothing() {
        assert!(parse(&with_head(&ether([8, 6]), &[0; 28]), 1).is_none());
    }

    #[test]
    fn two_vlan_tags_read_three_not() {
        let qinq = with_head(&ether([0x88, 0xA8]), &[0, 5, 0x81, 0, 0, 6, 8, 0]);
        assert_hi(parse(&with_head(&qinq, &hi_packet()), 1));

        let mut three = vec![0; 12];
        for _ in 0..3 {
            three.extend([0x81, 0, 0, 5]);
        }
        three.extend([8, 0]);
        assert!(parse(&with_head(&three, &hi_packet()), 1).is_none());
    }

    #[test]
    fn not_tcp_or_too_short_total() {
        let mut udp = hi_packet();
        udp[9] = 17;
        assert!(parse(&udp, 12).is_none());
        let mut short = hi_packet();
        short[3] = 39;
        assert!(parse(&short, 12).is_none());
    }

    #[test]
    fn cut_capture_keeps_what_arrived() {
        let p = hi_packet();
        assert_eq!(parse(&p[..41], 12).unwrap().data, b"h");
    }

    #[test]
    fn options_past_packet_end() {
        let mut p = hi_packet();
        p[32] = 0x60;
        assert!(parse(&p, 12).is_none());
    }

    #[test]
    fn sll_carrying_ipv6_first_byte() {
        let mut p = hi_packet();
        p[0] = 0x60;
        assert!(parse(&with_head(&sll(), &p), 113).is_none());
    }

    #[test]
    fn frames_under_four_bytes() {
        for link in [0, 1, 12, 113, 276, 999] {
            assert!(parse(&[0x45, 0, 0], link).is_none());
        }
    }

    #[test]
    fn label_falls_back_to_name() {
        assert_eq!(device("eth0", "", false).label(), "eth0");
        assert_eq!(device("\\Device\\NPF_{X}", "Realtek PCIe GbE", false).label(), "Realtek PCIe GbE");
    }

    #[test]
    fn virtual_by_flag_or_words() {
        assert!(device("lo", "", true).software_only());
        assert!(device("\\Device\\NPF_Loopback", "Adapter for loopback traffic capture", false).software_only());
        assert!(device("x", "TAP-Windows Adapter V9", false).software_only());
        assert!(device("x", "Wintun Userspace Tunnel", false).software_only());
        assert!(device("x", "WireGuard Tunnel", false).software_only());
        assert!(!device("eth0", "Intel(R) Ethernet Connection", false).software_only());
        assert!(!device("wg0", "", false).software_only());
    }

    #[test]
    fn tunnels_and_loopback_first() {
        assert!(reads_first(&device("lo", "", true), 1));
        assert!(reads_first(&device("vpn0", "", false), 12));
        assert!(reads_first(&device("wg0", "", false), 12));
        assert!(!reads_first(&device("eth0", "", false), 1));
        assert!(!reads_first(&device("wlan0", "", false), 1));
        for link in [14, 101, 228] {
            assert!(reads_first(&device("tun0", "", false), link));
        }
    }

    #[test]
    fn filter_narrows_then_widens() {
        let [vlan, plain] = filter_exprs(None);
        assert_eq!(plain, "tcp");
        assert_eq!(vlan, "tcp or (vlan and (tcp or (vlan and tcp)))");
        let [vlan, plain] = filter_exprs(Some(13328));
        assert_eq!(plain, "tcp port 13328");
        assert!(vlan.starts_with("tcp port 13328 or (vlan and"));

        set_filter_port(Some(13328));
        let now = now_ms();
        assert_eq!(wanted_filter_port(now), Some(13328));
        assert_eq!(wanted_filter_port(now + FILTER_PORT_QUIET_MS), None);
        set_filter_port(None);
        assert_eq!(wanted_filter_port(now_ms()), None);
    }

    #[test]
    fn linux_names_skipped() {
        for name in ["any", "nflog", "nfqueue", "usbmon1", "dbus-system", "bluetooth0", "docker0", "br-1a2b", "veth9", "podman0"] {
            assert!(linux::skip_device(name), "{name}");
        }
        for name in ["lo", "eth0", "eno1", "wlan0", "wg0", "anything"] {
            assert!(!linux::skipped_by_name(name), "{name}");
        }
        assert!(!linux::skip_device("lo"));
        let _: fn(&str) -> bool = linux::skip_device;
    }

    #[test]
    fn packet_time_in_ms() {
        assert_eq!(stamp_ms(1_700_000_000, 123_456), Some(1_700_000_000_123));
        assert_eq!(stamp_ms(0, 0), None);
        assert_eq!(stamp_ms(1_000_000_000, 0), None);
        assert_eq!(stamp_ms(2_000_000_000, 0), None);
        assert_eq!(stamp_ms(1_999_999_999, 999_999), Some(1_999_999_999_999));
    }

    #[test]
    fn load_names_each_library() {
        let Err(e) = PcapLib::load(&["nope1.so", "nope2.so"], "HELP") else { panic!("loaded") };
        assert!(e.contains("nope1.so") && e.contains("nope2.so") && e.contains("HELP"), "{e}");
    }

    #[test]
    fn c_layouts_match_libpcap() {
        let ptr = std::mem::size_of::<usize>();
        let long = std::mem::size_of::<c_long>();
        assert_eq!(std::mem::size_of::<PktHeader>(), (2 * long + 8).next_multiple_of(long));
        assert_eq!(std::mem::offset_of!(PktHeader, caplen), 2 * long);
        assert_eq!(std::mem::offset_of!(DevNode, flags), 4 * ptr);    }
}
