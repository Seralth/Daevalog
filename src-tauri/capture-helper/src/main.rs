//! `daevalog-capture`, the capture helper. On Linux the meter starts it from
//! its own folder. It holds cap_net_raw, so the meter (GTK, WebKit) holds no
//! capability.
//!
//! It opens every capture device, gives up its capabilities, and sends each
//! TCP payload to the meter on standard output (see `wire`). It takes
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

    use daevalog_capture::pcap::{self, linux, PcapLib, Sink};
    use daevalog_capture::wire::{read_frame, Control, Report, Status};
    use daevalog_capture::{Level, Segment};

    const HELP: &str = "Install libpcap (e.g. `sudo apt install libpcap0.8`).";

    /// With a loopback or tunnel device present, physical devices start this
    /// much later, so a ping reducer's loopback flow is seen first.
    const PHYSICAL_DELAY_MS: u64 = 1500;

    /// Standard output, unbuffered: one write per frame, under the lock.
    static OUT: OnceLock<Mutex<File>> = OnceLock::new();

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
            send(&Report::Packet(segment));
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

        // Every device is opened now, while the capability is held; an open
        // socket keeps capturing without it.
        let mut virtual_devices = Vec::new();
        let mut physical_devices = Vec::new();
        for device in &devices {
            match pcap.open(device) {
                Ok(live) if device.is_virtual() => virtual_devices.push(live),
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

        let delay = !virtual_devices.is_empty();
        for live in virtual_devices {
            let (pcap, running) = (pcap.clone(), running.clone());
            std::thread::spawn(move || pcap.run(live, &running, i64::MIN, &mut Pipe));
        }
        for live in physical_devices {
            let (pcap, running) = (pcap.clone(), running.clone());
            std::thread::spawn(move || {
                // Opened with the rest; what arrived before its turn is dropped,
                // as if it had been opened now.
                let mut since = i64::MIN;
                if delay {
                    std::thread::sleep(std::time::Duration::from_millis(PHYSICAL_DELAY_MS));
                    say(Level::Info, &format!("Starting capture on physical device: {}", live.label()));
                    since = now_ms();
                }
                pcap.run(live, &running, since, &mut Pipe);
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
