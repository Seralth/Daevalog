//! `daevalog-capture`, the capture helper. On Linux the meter starts it from
//! its own folder. It holds cap_net_raw, so the meter (GTK, WebKit) holds no
//! capability.
//!
//! It opens every capture device, gives up its capabilities, and sends each
//! TCP payload of the starting user's own connections to the meter on
//! standard output (see `wire`, `owner`). It takes
//! `Control` messages on standard input and quits when that input ends.

#[cfg(target_os = "linux")]
fn main() {
    helper::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("daevalog-capture runs on Linux only. Elsewhere the meter captures by itself.");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
mod helper {
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, OnceLock};

    use daevalog_capture::owner::{Gate, Proc};
    use daevalog_capture::pcap::{self, linux, PcapLib, Sink};
    use daevalog_capture::wire::{read_frame, Control, Report, Status};
    use daevalog_capture::{Level, Segment};

    const HELP: &str = "Install libpcap (e.g. `sudo apt install libpcap0.8`).";

    /// With a loopback or tunnel device present, physical devices start this
    /// much later, so a ping reducer's loopback flow is seen first.
    const PHYSICAL_DELAY_MS: u64 = 1500;

    /// Standard output, unbuffered: one write per frame, under the lock.
    static OUT: OnceLock<Mutex<File>> = OnceLock::new();

    /// The sockets of the user who started the helper. Only their packets
    /// go to the meter.
    static GATE: OnceLock<Gate> = OnceLock::new();

    /// How often packets left out as other users' are logged.
    const STATS_SECS: u64 = 60;

    fn send(report: &Report) {
        let bytes = report.encode();
        let Some(out) = OUT.get() else { return };
        let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
        // The meter is gone when its pipe cannot be written.
        if out.write_all(&bytes).is_err() {
            std::process::exit(0);
        }
    }

    fn say(level: Level, message: &str) {
        send(&Report::Log(level, message.to_string()));
    }

    struct Pipe;

    impl Sink for Pipe {
        fn packet(&mut self, segment: Segment) {
            let Some(gate) = GATE.get() else { return };
            if let Some(segment) = gate.packet(segment, now_ms()) {
                send(&Report::Packet(segment));
            }
        }
    }

    fn log_left_out() {
        let (mut logged, mut logged_overflow) = (0, 0);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(STATS_SECS));
            let Some(gate) = GATE.get() else { continue };
            let (total, overflow) = gate.counts();
            if total > logged {
                say(Level::Info, &format!("Left out {} packets of other users' connections in the last minute", total - logged));
                logged = total;
            }
            if overflow > logged_overflow {
                say(
                    Level::Warn,
                    &format!("Dropped {} packets in the last minute: too many were waiting for a socket lookup", overflow - logged_overflow),
                );
                logged_overflow = overflow;
            }
        }
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    pub fn run() {
        // Before any thread starts: nothing from the caller's environment
        // reaches libpcap or what it loads.
        for (name, _) in std::env::vars_os() {
            // SAFETY: no other thread exists yet.
            unsafe { std::env::remove_var(name) };
        }
        let Ok(out) = std::io::stdout().as_fd().try_clone_to_owned() else { std::process::exit(1) };
        let _ = OUT.set(Mutex::new(File::from(out)));
        daevalog_capture::set_logger(say);
        // The real uid: the user who ran the helper. It has file
        // capabilities, not setuid, so this is never someone else.
        // SAFETY: getuid cannot fail.
        let _ = GATE.set(Gate::new(unsafe { libc::getuid() }));

        let capable = capabilities().is_some_and(|(effective, _)| effective & (1 << CAP_NET_RAW) != 0);
        let fail = |message: &str| -> ! {
            say(Level::Error, message);
            send(&Report::Status(Status { capable, opened: 0, dropped: false }));
            std::process::exit(1);
        };

        let pcap = match PcapLib::load(linux::LIBRARIES, HELP) {
            Ok(p) => Arc::new(p),
            Err(e) => fail(&e),
        };
        let devices = match pcap.capture_devices(linux::skip_device) {
            Ok(d) if d.is_empty() => fail("No capture devices found with addresses"),
            Ok(d) => d,
            Err(e) => fail(&e),
        };
        pcap::log_devices(&devices);
        // The devices found, before the status: the meter starts the helper
        // again when the list changes.
        send(&Report::Devices(devices.iter().map(|d| d.label().to_string()).collect()));

        // Every device is opened now, while the capability is held; an open
        // socket keeps capturing without it.
        let mut virtual_devices = Vec::new();
        let mut physical_devices = Vec::new();
        for device in &devices {
            match pcap.open(device, Some(linux::BUFFER_BYTES)) {
                Ok(live) if live.reads_first() => virtual_devices.push(live),
                Ok(live) => physical_devices.push(live),
                Err(e) => say(Level::Warn, &format!("Failed to open capture on {}: {}", device.label(), e)),
            }
        }
        let opened = (virtual_devices.len() + physical_devices.len()) as u16;
        if opened == 0 {
            fail("No capture device could be opened");
        }
        let dropped = match drop_capabilities() {
            Ok(()) => true,
            Err(e) => {
                say(Level::Warn, &format!("Could not give up capabilities: {e}"));
                false
            }
        };
        send(&Report::Status(Status { capable, opened, dropped }));

        let running = Arc::new(AtomicBool::new(true));
        pcap::start_filter_watcher(pcap.clone(), running.clone());
        std::thread::spawn(log_left_out);
        std::thread::spawn(|| {
            if let Some(gate) = GATE.get() {
                gate.look_up(&mut Proc, |segment| send(&Report::Packet(segment)));
            }
        });

        let delay = !virtual_devices.is_empty();
        for live in virtual_devices {
            let (pcap, running) = (pcap.clone(), running.clone());
            std::thread::spawn(move || pcap.run(live, &running, i64::MIN, &mut Pipe));
        }
        // Opened with the rest, and read from now on too: an unread capture
        // buffer fills in under a second, and the kernel drops what no longer
        // fits. What arrives before their turn is skipped, as if they had
        // been opened then.
        let since = if delay { now_ms() + PHYSICAL_DELAY_MS as i64 } else { i64::MIN };
        let labels: Vec<String> = physical_devices.iter().map(|live| live.label().to_string()).collect();
        for live in physical_devices {
            let (pcap, running) = (pcap.clone(), running.clone());
            std::thread::spawn(move || pcap.run(live, &running, since, &mut Pipe));
        }
        if delay {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(PHYSICAL_DELAY_MS));
                for label in labels {
                    say(Level::Info, &format!("Starting capture on physical device: {label}"));
                }
            });
        }

        let mut input = std::io::stdin().lock();
        loop {
            let body = match read_frame(&mut input) {
                Ok(Some(body)) => body,
                Ok(None) => break,
                Err(e) => {
                    say(Level::Error, &format!("Capture helper input: {e}"));
                    break;
                }
            };
            match Control::decode(&body) {
                Ok(Control::SetFilterPort(port)) => pcap::set_filter_port(port),
                Ok(Control::ListDevices) => {
                    let labels = pcap
                        .capture_devices(linux::skip_device)
                        .map(|d| d.iter().map(|d| d.label().to_string()).collect())
                        .unwrap_or_default();
                    send(&Report::Devices(labels));
                }
                Ok(Control::Stop) => break,
                Err(e) => {
                    say(Level::Error, &format!("Capture helper input: {e}"));
                    break;
                }
            }
        }
        std::process::exit(0);
    }

    const CAP_NET_RAW: u32 = 13;

    /// Effective and permitted capabilities, from /proc/self/status.
    fn capabilities() -> Option<(u64, u64)> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let field = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        };
        Some((field("CapEff:")?, field("CapPrm:")?))
    }

    /// Give up every capability, and with no-new-privs never gain one again.
    /// Only this thread, which is the only one so far.
    fn drop_capabilities() -> std::io::Result<()> {
        #[repr(C)]
        struct Header {
            version: u32,
            pid: i32,
        }
        #[repr(C)]
        struct Data {
            effective: u32,
            permitted: u32,
            inheritable: u32,
        }
        const VERSION_3: u32 = 0x2008_0522;
        let header = Header { version: VERSION_3, pid: 0 };
        let data = [Data { effective: 0, permitted: 0, inheritable: 0 }, Data { effective: 0, permitted: 0, inheritable: 0 }];
        // SAFETY: plain syscalls on memory that outlives them.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::syscall(libc::SYS_capset, &header, data.as_ptr()) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        match capabilities() {
            Some((0, 0)) => Ok(()),
            _ => Err(std::io::Error::other("capabilities still set")),
        }
    }
}
