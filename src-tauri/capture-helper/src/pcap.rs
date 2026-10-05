//! libpcap, loaded at run time: devices, capture handles, the packet filter and
//! the TCP payload in each frame. No SDK is needed at compile time.

use std::ffi::{c_char, c_int, c_long, c_uint, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use libloading::{Library, Symbol};

use crate::{log, now_ms, Level, Segment};

// ===== Raw pcap FFI types =====

type PcapT = *mut std::ffi::c_void;
type PcapIfT = *mut PcapIf;

#[repr(C)]
struct PcapIf {
    next: *mut PcapIf,
    name: *const c_char,
    description: *const c_char,
    addresses: *mut PcapAddr,
    flags: c_uint,
}

#[repr(C)]
struct PcapAddr {
    next: *mut PcapAddr,
    addr: *mut SockAddr,
    netmask: *mut SockAddr,
    broadaddr: *mut SockAddr,
    dstaddr: *mut SockAddr,
}

#[repr(C)]
struct SockAddr {
    sa_family: u16,
    sa_data: [u8; 14],
}

/// `struct pcap_pkthdr`. The timestamp is a C `struct timeval`, two `long`s:
/// 32-bit on Windows, 64-bit on 64-bit Linux. Declaring them `i32` (as this
/// once did) reads `caplen` from the middle of the timestamp on Linux.
#[repr(C)]
struct PcapPkthdr {
    ts_sec: c_long,
    ts_usec: c_long,
    caplen: c_uint,
    len: c_uint,
}

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

const PCAP_IF_LOOPBACK: c_uint = 0x00000001;
const PCAP_NETMASK_UNKNOWN: c_uint = 0xffffffff;

/// libpcap as nearly every Linux distribution ships it, and the devices not
/// worth opening there. Here rather than in the meter so the helper has them.
pub mod linux {
    /// Tried in order. Distributions disagree on the soname: Arch and Fedora ship
    /// `libpcap.so.1`, while Debian and Ubuntu keep the old `libpcap.so.0.8` and
    /// have no `.so.1` at all (only the -dev package adds a bare `libpcap.so`).
    /// Loading `.so.1` alone left the meter unable to capture on every
    /// Debian-family system.
    pub const LIBRARIES: &[&str] = &["libpcap.so.1", "libpcap.so.0.8", "libpcap.so"];

    /// Skip libpcap's pseudo-devices. "any" sees every packet a second time on top
    /// of the real interface it arrived on; the rest carry no IP traffic.
    ///
    /// Container networking too: Docker and Podman give every container a `veth`
    /// pair and every network a `docker0` / `br-<id>` / `podman` bridge. The game,
    /// under Proton, talks through the real interface, so these only add capture
    /// threads (one player had 22 of them).
    pub fn skip_device(name: &str) -> bool {
        name == "any"
            || ["nflog", "nfqueue", "usbmon", "dbus", "bluetooth", "veth", "docker", "br-", "podman"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
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

/// An idle interface on Linux blocks in `pcap_next_ex` until a packet comes,
/// timeout or not, so its thread would never see a filter change. Wake each
/// thread whose filter is out of date.
pub fn start_filter_watcher(pcap: Arc<PcapLib>, running: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(FILTER_WATCH_MS));
            let want = wanted_filter_port(now_ms()).map_or(0, u32::from);
            for live in live_handles().iter() {
                if live.applied.load(Ordering::Relaxed) != want {
                    unsafe { (pcap.breakloop)(live.handle as PcapT) };
                }
            }
        }
    });
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

// ===== Device info =====

#[derive(Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub description: String,
    pub has_addresses: bool,
    pub is_loopback: bool,
}

impl DeviceInfo {
    pub fn label(&self) -> &str {
        if self.description.is_empty() {
            &self.name
        } else {
            &self.description
        }
    }

    pub fn is_virtual(&self) -> bool {
        let label = self.label().to_lowercase();
        self.is_loopback
            || self.name.to_lowercase().contains("loopback")
            || label.contains("loopback")
            || label.contains("tap-windows")
            || label.contains("tap")
            || label.contains("wintun")
            || label.contains("wireguard")
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
    applied: Arc<AtomicU32>,
}

impl Live {
    pub fn label(&self) -> &str {
        &self.label
    }
}

// ===== Pcap library wrapper =====

pub struct PcapLib {
    _lib: Library,
    findalldevs: unsafe extern "C" fn(*mut PcapIfT, *mut c_char) -> c_int,
    freealldevs: unsafe extern "C" fn(PcapIfT),
    open_live: unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> PcapT,
    close: unsafe extern "C" fn(PcapT),
    next_ex: unsafe extern "C" fn(PcapT, *mut *mut PcapPkthdr, *mut *const u8) -> c_int,
    datalink: unsafe extern "C" fn(PcapT) -> c_int,
    compile: unsafe extern "C" fn(PcapT, *mut BpfProgram, *const c_char, c_int, c_uint) -> c_int,
    setfilter: unsafe extern "C" fn(PcapT, *mut BpfProgram) -> c_int,
    freecode: unsafe extern "C" fn(*mut BpfProgram),
    geterr: unsafe extern "C" fn(PcapT) -> *const c_char,
    breakloop: unsafe extern "C" fn(PcapT),
    stats: Option<unsafe extern "C" fn(PcapT, *mut PcapStat) -> c_int>,
    /// Older libpcap's filter compiler is not thread-safe.
    compile_lock: Mutex<()>,
}

impl PcapLib {
    /// The first of `libraries` that loads. `help` goes into the error when
    /// none does.
    pub fn load(libraries: &[&str], help: &str) -> Result<Self, String> {
        let mut errors = Vec::new();
        let mut loaded = None;
        for name in libraries {
            // SAFETY: loading libpcap runs no initialisation we depend on not running.
            match unsafe { Library::new(name) } {
                Ok(lib) => {
                    loaded = Some(lib);
                    break;
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        let lib = loaded.ok_or_else(|| {
            format!("Failed to load {}. {}\nError: {}", libraries.join(" or "), help, errors.join("; "))
        })?;

        unsafe {
            let findalldevs: Symbol<unsafe extern "C" fn(*mut PcapIfT, *mut c_char) -> c_int> =
                lib.get(b"pcap_findalldevs").map_err(|e| format!("pcap_findalldevs: {}", e))?;
            let freealldevs: Symbol<unsafe extern "C" fn(PcapIfT)> =
                lib.get(b"pcap_freealldevs").map_err(|e| format!("pcap_freealldevs: {}", e))?;
            let open_live: Symbol<
                unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, *mut c_char) -> PcapT,
            > = lib.get(b"pcap_open_live").map_err(|e| format!("pcap_open_live: {}", e))?;
            let close: Symbol<unsafe extern "C" fn(PcapT)> =
                lib.get(b"pcap_close").map_err(|e| format!("pcap_close: {}", e))?;
            let next_ex: Symbol<
                unsafe extern "C" fn(PcapT, *mut *mut PcapPkthdr, *mut *const u8) -> c_int,
            > = lib.get(b"pcap_next_ex").map_err(|e| format!("pcap_next_ex: {}", e))?;
            let datalink: Symbol<unsafe extern "C" fn(PcapT) -> c_int> =
                lib.get(b"pcap_datalink").map_err(|e| format!("pcap_datalink: {}", e))?;
            let compile: Symbol<
                unsafe extern "C" fn(PcapT, *mut BpfProgram, *const c_char, c_int, c_uint) -> c_int,
            > = lib.get(b"pcap_compile").map_err(|e| format!("pcap_compile: {}", e))?;
            let setfilter: Symbol<unsafe extern "C" fn(PcapT, *mut BpfProgram) -> c_int> =
                lib.get(b"pcap_setfilter").map_err(|e| format!("pcap_setfilter: {}", e))?;
            let freecode: Symbol<unsafe extern "C" fn(*mut BpfProgram)> =
                lib.get(b"pcap_freecode").map_err(|e| format!("pcap_freecode: {}", e))?;
            let geterr: Symbol<unsafe extern "C" fn(PcapT) -> *const c_char> =
                lib.get(b"pcap_geterr").map_err(|e| format!("pcap_geterr: {}", e))?;
            let breakloop: Symbol<unsafe extern "C" fn(PcapT)> =
                lib.get(b"pcap_breakloop").map_err(|e| format!("pcap_breakloop: {}", e))?;
            let stats = lib.get::<unsafe extern "C" fn(PcapT, *mut PcapStat) -> c_int>(b"pcap_stats").ok().map(|f| *f);

            Ok(Self {
                findalldevs: *findalldevs,
                freealldevs: *freealldevs,
                open_live: *open_live,
                close: *close,
                next_ex: *next_ex,
                datalink: *datalink,
                compile: *compile,
                setfilter: *setfilter,
                freecode: *freecode,
                geterr: *geterr,
                breakloop: *breakloop,
                stats,
                compile_lock: Mutex::new(()),
                _lib: lib,
            })
        }
    }

    pub fn find_all_devs(&self) -> Result<Vec<DeviceInfo>, String> {
        let mut alldevs: PcapIfT = ptr::null_mut();
        let mut errbuf = [0u8; 256];

        let ret =
            unsafe { (self.findalldevs)(&mut alldevs, errbuf.as_mut_ptr() as *mut c_char) };

        if ret != 0 || alldevs.is_null() {
            let err = unsafe { CStr::from_ptr(errbuf.as_ptr() as *const c_char) }
                .to_string_lossy()
                .to_string();
            return Err(format!("pcap_findalldevs failed: {}", err));
        }

        let mut devices = Vec::new();
        let mut dev = alldevs;
        while !dev.is_null() {
            let d = unsafe { &*dev };
            let name = if d.name.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(d.name) }
                    .to_string_lossy()
                    .to_string()
            };
            let description = if d.description.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(d.description) }
                    .to_string_lossy()
                    .to_string()
            };
            let has_addresses = !d.addresses.is_null();
            let is_loopback = (d.flags & PCAP_IF_LOOPBACK) != 0;

            devices.push(DeviceInfo {
                name,
                description,
                has_addresses,
                is_loopback,
            });
            dev = d.next;
        }

        unsafe { (self.freealldevs)(alldevs) };
        Ok(devices)
    }

    /// The devices worth capturing on: those that have addresses, and that
    /// `skip` (the OS's list) does not rule out. Linux's catch-all "any"
    /// would duplicate every packet.
    pub fn capture_devices(&self, skip: fn(&str) -> bool) -> Result<Vec<DeviceInfo>, String> {
        let devices = self.find_all_devs().map_err(|e| format!("Failed to list devices: {}", e))?;
        Ok(devices.into_iter().filter(|d| d.has_addresses && !skip(&d.name)).collect())
    }

    fn open_live_handle(&self, name: &str) -> Result<PcapT, String> {
        let c_name = CString::new(name).map_err(|e| format!("Invalid device name: {}", e))?;
        let mut errbuf = [0u8; 256];

        let handle = unsafe {
            (self.open_live)(
                c_name.as_ptr(),
                65535,  // snaplen
                0,      // not promiscuous: only this machine's own traffic
                100,    // timeout ms
                errbuf.as_mut_ptr() as *mut c_char,
            )
        };

        if handle.is_null() {
            let err = unsafe { CStr::from_ptr(errbuf.as_ptr() as *const c_char) }
                .to_string_lossy()
                .to_string();
            return Err(format!("pcap_open_live failed: {}", err));
        }

        Ok(handle)
    }

    /// Open a capture on `device` with the plain `tcp` filter.
    pub fn open(&self, device: &DeviceInfo) -> Result<Live, String> {
        let label = device.label().to_string();
        let handle = self.open_live_handle(&device.name)?;

        let link_type = unsafe { (self.datalink)(handle) };
        // Only TCP reaches the meter. A filter that fails leaves the capture
        // unfiltered, as it was before filters.
        let filter = self.set_filter(handle, None).unwrap_or_else(|e| {
            log(Level::Warn, &format!("No packet filter on {}: {}", label, e));
            "none".to_string()
        });
        log(Level::Info, &format!("Capture active on {} (link type {}, filter {})", label, link_type, filter));
        let applied = Arc::new(AtomicU32::new(0));
        live_handles().push(LiveHandle { handle: handle as usize, applied: applied.clone() });
        Ok(Live { handle: handle as usize, label, link_type, applied })
    }

    /// Read `live` until `running` turns false or the device fails, then close
    /// it. Packets captured before `skip_before_ms` are dropped.
    pub fn run(&self, live: Live, running: &AtomicBool, skip_before_ms: i64, sink: &mut impl Sink) {
        let Live { handle, label, link_type, applied } = live;
        let handle = handle as PcapT;

        // A filter change that failed is tried again a second later, and
        // logged once per wanted port.
        let mut retry_at = 0;
        let mut failed: Option<u32> = None;
        let mut stats_at = now_ms() + STATS_MS;
        let mut dropped = 0;
        while running.load(Ordering::SeqCst) {
            let now = now_ms();
            if now >= stats_at {
                stats_at = now + STATS_MS;
                if let Some(total) = self.kernel_drops(handle) {
                    let new = total.wrapping_sub(dropped);
                    dropped = total;
                    if new > 0 {
                        log(Level::Warn, &format!("Capture on {}: the kernel dropped {} packets in the last minute", label, new));
                    }
                }
            }
            let want = wanted_filter_port(now);
            let want_port = want.map_or(0, u32::from);
            if want_port != applied.load(Ordering::Relaxed) && now >= retry_at {
                match self.set_filter(handle, want) {
                    Ok(expr) => {
                        applied.store(want_port, Ordering::Relaxed);
                        failed = None;
                        log(Level::Info, &format!("Capture filter on {}: {}", label, expr));
                    }
                    Err(e) => {
                        retry_at = now + FILTER_WATCH_MS as i64;
                        if failed != Some(want_port) {
                            failed = Some(want_port);
                            log(Level::Warn, &format!("Capture filter on {} not changed: {}", label, e));
                        }
                    }
                }
            }

            let mut header: *mut PcapPkthdr = ptr::null_mut();
            let mut data: *const u8 = ptr::null();

            let ret = unsafe { (self.next_ex)(handle, &mut header, &mut data) };

            match ret {
                1 => {
                    let hdr = unsafe { &*header };
                    let len = hdr.caplen as usize;
                    // Use pcap hardware timestamp (seconds + microseconds since epoch)
                    let pcap_ts_ms = (hdr.ts_sec as i64) * 1000 + (hdr.ts_usec as i64) / 1000;
                    // Sanity check: if pcap timestamp looks bogus, fall back to wall clock
                    let ts = if pcap_ts_ms > 1_000_000_000_000 && pcap_ts_ms < 2_000_000_000_000 {
                        pcap_ts_ms
                    } else {
                        now_ms()
                    };
                    if ts < skip_before_ms {
                        continue;
                    }
                    let frame = unsafe { std::slice::from_raw_parts(data, len) };
                    if let Some(mut segment) = parse_tcp_payload(frame, link_type, &label) {
                        segment.captured_at_ms = ts;
                        note_filter_port_packet(&segment);
                        sink.packet(segment);
                    }
                }
                0 => {
                    // Timeout
                    sink.idle();
                    continue;
                }
                -2 => continue, // woken by the filter watcher
                _ => {
                    log(Level::Warn, &format!("Capture error on {} (ret={})", label, ret));
                    break;
                }
            }
        }

        live_handles().retain(|live| live.handle != handle as usize);
        unsafe { (self.close)(handle) };
        log(Level::Info, &format!("Capture stopped on {}", label));
    }

    /// Set a kernel packet filter on `handle`: the first of `filter_exprs(port)`
    /// that compiles. Returns the expression set.
    fn set_filter(&self, handle: PcapT, port: Option<u16>) -> Result<String, String> {
        let mut errors = Vec::new();
        for expr in filter_exprs(port) {
            match self.try_filter(handle, &expr) {
                Ok(()) => return Ok(expr),
                Err(e) => errors.push(e),
            }
        }
        Err(errors.join("; "))
    }

    fn try_filter(&self, handle: PcapT, expr: &str) -> Result<(), String> {
        let c_expr = CString::new(expr).map_err(|e| e.to_string())?;
        let mut program = BpfProgram { bf_len: 0, bf_insns: ptr::null_mut() };
        let compiled = {
            let _guard = self.compile_lock.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { (self.compile)(handle, &mut program, c_expr.as_ptr(), 1, PCAP_NETMASK_UNKNOWN) }
        };
        if compiled != 0 {
            return Err(format!("pcap_compile \"{}\": {}", expr, self.last_error(handle)));
        }
        let set = unsafe { (self.setfilter)(handle, &mut program) };
        unsafe { (self.freecode)(&mut program) };
        if set != 0 {
            return Err(format!("pcap_setfilter \"{}\": {}", expr, self.last_error(handle)));
        }
        Ok(())
    }

    /// Packets the kernel dropped for this handle since it was opened: its
    /// buffer was full. (Not `ps_ifdrop`: on Linux that is the whole
    /// interface's drop count, mostly traffic nobody asked for.)
    fn kernel_drops(&self, handle: PcapT) -> Option<c_uint> {
        let stats = self.stats?;
        let mut stat = PcapStat::default();
        (unsafe { stats(handle, &mut stat) } == 0).then_some(stat.ps_drop)
    }

    fn last_error(&self, handle: PcapT) -> String {
        let err = unsafe { (self.geterr)(handle) };
        if err.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(err) }.to_string_lossy().to_string()
    }
}

// Safety: PcapLib function pointers are thread-safe (each thread gets its own pcap handle)
unsafe impl Send for PcapLib {}
unsafe impl Sync for PcapLib {}

/// The device list as the capture log shows it at start.
pub fn log_devices(devices: &[DeviceInfo]) {
    log(Level::Info, &format!("Found {} capture devices", devices.len()));
    for (i, dev) in devices.iter().enumerate() {
        log(
            Level::Info,
            &format!("  [{}] {} (loopback={}, addresses={})", i, dev.label(), dev.is_loopback, dev.has_addresses),
        );
    }
}

/// Where IPv4 starts in a frame of pcap link type `link_type`, or `None` if this
/// frame is not IPv4 under that type. Link types are libpcap's (DLT_*/LINKTYPE_*),
/// the same on every OS.
fn link_header_len(link_type: c_int, frame: &[u8]) -> Option<usize> {
    let be16 = |at: usize| frame.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    match link_type {
        // Ethernet, with up to two 802.1Q / 802.1ad VLAN tags (some NICs, e.g.
        // Realtek with "Priority & VLAN" on, tag every frame).
        1 => {
            let mut at = 12;
            for _ in 0..2 {
                match be16(at)? {
                    0x8100 | 0x88A8 => at += 4,
                    _ => break,
                }
            }
            (be16(at)? == 0x0800).then_some(at + 2)
        }
        // BSD loopback / Npcap loopback: 4-byte address family, AF_INET = 2.
        0 => (frame.get(..4)? == [2, 0, 0, 0]).then_some(4),
        // Raw IP (VPN and tunnel adapters, WireGuard).
        12 | 14 | 101 | 228 => Some(0),
        // Linux "cooked" capture v1: protocol at bytes 14-15, 16-byte header.
        113 => (be16(14)? == 0x0800).then_some(16),
        // Linux "cooked" capture v2: protocol at bytes 0-1, 20-byte header.
        276 => (be16(0)? == 0x0800).then_some(20),
        _ => None,
    }
}

/// The original guess at the link layer, from the frame alone. Kept as the
/// fallback so nothing that parsed before the link type was consulted can stop
/// parsing now.
fn guess_link_header_len(frame: &[u8]) -> Option<usize> {
    if frame.len() >= 14 {
        let ether_type = u16::from_be_bytes([frame[12], frame[13]]);
        if ether_type == 0x0800 {
            Some(14) // Standard Ethernet
        } else if frame[0] == 2 && frame[1] == 0 && frame[2] == 0 && frame[3] == 0 {
            Some(4) // NULL/Loopback: AF_INET (little-endian 2) = IPv4
        } else if (frame[0] >> 4) == 4 {
            Some(0) // Raw IPv4
        } else {
            None
        }
    } else if (frame[0] >> 4) == 4 {
        Some(0) // Raw IPv4 (short frame)
    } else {
        None
    }
}

/// Parse raw captured frame to extract TCP payload. The link type says where
/// IPv4 starts; when it does not fit, the frame-only guess gets a chance.
fn parse_tcp_payload(frame: &[u8], link_type: c_int, device_name: &str) -> Option<Segment> {
    if frame.len() < 4 {
        return None;
    }
    let ip_offset = link_header_len(link_type, frame)
        .filter(|&at| frame.get(at).is_some_and(|b| b >> 4 == 4))
        .or_else(|| guess_link_header_len(frame))?;

    if frame.len() < ip_offset + 20 {
        return None;
    }
    let ip_header = &frame[ip_offset..];
    if ip_header[0] >> 4 != 4 {
        return None;
    }
    let ip_header_len = ((ip_header[0] & 0x0F) as usize) * 4;
    if ip_header[9] != 6 {
        return None; // Not TCP
    }
    // The packet ends where IPv4's total length says. Ethernet pads short
    // frames to 60 bytes, and the padding is not payload. A total length of
    // 0 (a large segment from TCP offload) means the rest of the frame.
    let total_len = u16::from_be_bytes([ip_header[2], ip_header[3]]) as usize;
    let ip_end = match total_len {
        0 => frame.len(),
        n if n < ip_header_len + 20 => return None,
        n => (ip_offset + n).min(frame.len()),
    };

    let src_ip = [ip_header[12], ip_header[13], ip_header[14], ip_header[15]];
    let dst_ip = [ip_header[16], ip_header[17], ip_header[18], ip_header[19]];

    let tcp_offset = ip_offset + ip_header_len;
    if frame.len() < tcp_offset + 20 {
        return None;
    }
    let tcp_header = &frame[tcp_offset..];
    let src_port = u16::from_be_bytes([tcp_header[0], tcp_header[1]]);
    let dst_port = u16::from_be_bytes([tcp_header[2], tcp_header[3]]);
    let tcp_seq = u32::from_be_bytes([tcp_header[4], tcp_header[5], tcp_header[6], tcp_header[7]]);
    let tcp_ack =
        u32::from_be_bytes([tcp_header[8], tcp_header[9], tcp_header[10], tcp_header[11]]);
    let tcp_header_len = ((tcp_header[12] >> 4) as usize) * 4;

    let payload_offset = tcp_offset + tcp_header_len;
    if payload_offset >= ip_end {
        return None;
    }
    let payload = &frame[payload_offset..ip_end];
    if payload.is_empty() {
        return None;
    }

    Some(Segment {
        device: device_name.to_string(),
        captured_at_ms: now_ms(),
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        seq: tcp_seq,
        ack: tcp_ack,
        data: payload.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IPv4 + TCP from 10.0.0.2:51000 to 193.202.112.99:13328, payload "hi".
    fn ipv4_tcp() -> Vec<u8> {
        let mut ip = vec![0x45, 0, 0, 42, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 2, 193, 202, 112, 99];
        let tcp = [0xC7, 0x38, 0x34, 0x10, 0, 0, 0, 1, 0, 0, 0, 0, 0x50, 0x18, 0, 0, 0, 0, 0, 0];
        ip.extend_from_slice(&tcp);
        ip.extend_from_slice(b"hi");
        ip
    }

    fn check(link_type: c_int, header: &[u8]) {
        let frame = [header, &ipv4_tcp()].concat();
        let p = parse_tcp_payload(&frame, link_type, "dev").expect("parsed");
        assert_eq!((p.src_port, p.dst_port), (51000, 13328), "link type {link_type}");
        assert_eq!(p.dst_ip, [193, 202, 112, 99]);
        assert_eq!(p.data, b"hi");
    }

    #[test]
    fn reads_every_link_layer_the_meter_meets() {
        let ether = [[0u8; 12].as_slice(), &[0x08, 0x00]].concat();
        check(1, &ether); // Ethernet / Wi-Fi
        let vlan = [[0u8; 12].as_slice(), &[0x81, 0x00, 0x00, 0x05, 0x08, 0x00]].concat();
        check(1, &vlan); // Ethernet with an 802.1Q tag
        check(0, &[2, 0, 0, 0]); // Npcap / BSD loopback
        check(12, &[]); // raw IP (VPN, WireGuard)
        check(101, &[]);
        let sll = [[0u8; 14].as_slice(), &[0x08, 0x00]].concat();
        check(113, &sll); // Linux cooked v1
        let sll2 = [[0x08u8, 0x00].as_slice(), &[0u8; 18]].concat();
        check(276, &sll2); // Linux cooked v2
    }

    #[test]
    fn an_unknown_or_mismatched_link_type_still_gets_the_old_guess() {
        // Ethernet framing reported as some other link type: guessed as before.
        let ether = [[0u8; 12].as_slice(), &[0x08, 0x00]].concat();
        check(999, &ether);
        // Raw IP on a device that claims Ethernet: the exact read fails, the
        // guess finds IPv4 at 0, as it always did.
        check(1, &[]);
    }

    /// A bare ACK is 54 bytes on Ethernet, padded to the 60-byte minimum.
    /// The 6 padding bytes are not payload.
    #[test]
    fn ethernet_padding_is_not_payload() {
        let mut ip = vec![0x45, 0, 0, 40, 0, 0, 0, 0, 64, 6, 0, 0, 193, 202, 112, 99, 10, 0, 0, 2];
        ip.extend_from_slice(&[0x34, 0x10, 0xC7, 0x38, 0, 0, 0, 1, 0, 0, 0, 2, 0x50, 0x10, 0, 0, 0, 0, 0, 0]);
        let frame = [[0u8; 12].as_slice(), &[0x08, 0x00], &ip, &[0u8; 6]].concat();
        assert_eq!(frame.len(), 60);
        assert!(parse_tcp_payload(&frame, 1, "dev").is_none());

        // With a payload, the padding after it is cut off too.
        let mut short = ipv4_tcp();
        let frame = [[0u8; 12].as_slice(), &[0x08, 0x00], &short, &[0u8; 4]].concat();
        assert_eq!(parse_tcp_payload(&frame, 1, "dev").expect("parsed").data, b"hi");
        // A total length of 0 (TCP offload) keeps the rest of the frame.
        short[2..4].copy_from_slice(&[0, 0]);
        let frame = [[0u8; 12].as_slice(), &[0x08, 0x00], &short].concat();
        assert_eq!(parse_tcp_payload(&frame, 1, "dev").expect("parsed").data, b"hi");
    }

    #[test]
    fn non_ipv4_frames_are_ignored() {
        let arp = [[0u8; 12].as_slice(), &[0x08, 0x06], &[0u8; 28]].concat();
        assert!(parse_tcp_payload(&arp, 1, "dev").is_none());
    }

    #[test]
    fn the_filter_narrows_to_the_locked_port_while_it_carries_traffic() {
        assert_eq!(filter_exprs(None)[1], "tcp");
        assert_eq!(filter_exprs(Some(13328))[1], "tcp port 13328");
        assert!(filter_exprs(Some(13328))[0].starts_with("tcp port 13328 or (vlan and"));

        set_filter_port(Some(13328));
        let now = now_ms();
        assert_eq!(wanted_filter_port(now), Some(13328));
        assert_eq!(wanted_filter_port(now + FILTER_PORT_QUIET_MS), None, "quiet port: all TCP again");
        set_filter_port(None);
        assert_eq!(wanted_filter_port(now), None);
    }

    #[test]
    fn linux_skips_pseudo_and_container_devices() {
        for name in ["any", "nflog", "usbmon1", "docker0", "br-1a2b", "veth9", "podman0"] {
            assert!(linux::skip_device(name), "{name}");
        }
        for name in ["lo", "eth0", "enp5s0", "wlan0", "wg0"] {
            assert!(!linux::skip_device(name), "{name}");
        }
    }
}
