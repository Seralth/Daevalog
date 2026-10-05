//! The meter's side of the capture helper (`daevalog-capture`, in
//! `capture-helper/`): starts it from the meter's own folder, feeds its packets
//! to the dispatcher, and passes on filter changes. The helper holds the
//! capture permission, so the meter holds none.
//!
//! The helper opens its devices once, at start, and then gives up the
//! permission, so it cannot open a device that shows up later. `Supervisor`
//! starts it again when it dies, and, while no port is locked, when the list
//! of devices changes (Wi-Fi that came up after the meter did).

use std::collections::{BTreeSet, HashMap};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daevalog_capture::wire::{read_frame, Control, Report, Status, MAX_FRAME};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::captured_payload::CapturedPayload;
use super::pcap_capturer::{log_line, to_payload, DropCounter};

/// How long the device list may take.
const DEVICES_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the device list is compared with the one the helper opened.
const DEVICE_POLL: Duration = Duration::from_secs(30);

/// The wait before starting a helper that stopped: doubled after each try
/// that fails, up to the most.
const RESTART_FIRST: Duration = Duration::from_secs(1);
const RESTART_MOST: Duration = Duration::from_secs(60);

/// A helper that captured this long starts the waits over when it stops.
const RAN_WELL: Duration = Duration::from_secs(60);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// What the helper's reader tells the supervisor.
enum Event {
    Status(Status),
    Exited,
}

struct Helper {
    path: PathBuf,
    /// `None` once closed: the helper quits when its input ends.
    input: Mutex<Option<ChildStdin>>,
    status: Mutex<Option<Status>>,
    alive: AtomicBool,
    /// The devices the helper found when it started, from the list it sends
    /// before its status.
    found: Mutex<Option<Vec<String>>>,
    /// One device-list question at a time, and where its answer goes.
    asking: Mutex<()>,
    devices: Mutex<Option<std::sync::mpsc::Sender<Vec<String>>>>,
}

impl Helper {
    /// Start the helper at `path`, with an empty environment.
    fn start(
        path: &Path,
        sender: mpsc::Sender<CapturedPayload>,
        events: std::sync::mpsc::Sender<Event>,
    ) -> Result<Arc<Self>, String> {
        let mut child = Command::new(path)
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {}", path.display(), e))?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err("no pipe to the capture helper".into());
        };
        let helper = Arc::new(Self {
            path: path.to_path_buf(),
            input: Mutex::new(Some(input)),
            status: Mutex::new(None),
            alive: AtomicBool::new(true),
            found: Mutex::new(None),
            asking: Mutex::new(()),
            devices: Mutex::new(None),
        });
        info!("Capture helper started: {}", path.display());
        let reader = helper.clone();
        std::thread::Builder::new()
            .name("capture-helper".into())
            .spawn(move || reader.read(output, child, sender, events))
            .map_err(|e| e.to_string())?;
        Ok(helper)
    }

    fn read(
        &self,
        output: ChildStdout,
        mut child: Child,
        sender: mpsc::Sender<CapturedPayload>,
        events: std::sync::mpsc::Sender<Event>,
    ) {
        let mut output = BufReader::with_capacity(MAX_FRAME, output);
        let mut drops: HashMap<String, DropCounter> = HashMap::new();
        let reason = loop {
            let body = match read_frame(&mut output) {
                Ok(Some(body)) => body,
                Ok(None) => break "it quit".to_string(),
                Err(e) => break e.to_string(),
            };
            match Report::decode(&body) {
                Ok(Report::Packet(segment)) => {
                    let label = segment.device.clone();
                    let counter = drops.entry(label.clone()).or_insert_with(DropCounter::new);
                    if let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(to_payload(segment)) {
                        counter.note_drop(&label, now_ms());
                    } else {
                        counter.report(&label, now_ms());
                    }
                }
                Ok(Report::Log(level, text)) => log_line(level, &text),
                Ok(Report::Status(status)) => {
                    self.note_status(status);
                    *self.status.lock() = Some(status);
                    let _ = events.send(Event::Status(status));
                }
                // Before the status: the devices it found. After: an answer.
                Ok(Report::Devices(labels)) if self.status.lock().is_none() => {
                    *self.found.lock() = Some(labels);
                }
                Ok(Report::Devices(labels)) => {
                    if let Some(answer) = self.devices.lock().take() {
                        let _ = answer.send(labels);
                    }
                }
                Err(e) => break e.to_string(),
            }
        };
        self.alive.store(false, Ordering::SeqCst);
        let _ = child.kill();
        let exit = child.wait().map(|s| s.to_string()).unwrap_or_default();
        if self.input.lock().is_some() {
            warn!("Capture helper stopped ({reason}, {exit})");
        } else {
            info!("Capture helper stopped, as asked ({exit})");
        }
        let _ = events.send(Event::Exited);
    }

    fn note_status(&self, status: Status) {
        if status.opened == 0 {
            error!(
                "Packet capture is off: the capture helper {} opened no device (capture permission: {}). {}",
                self.path.display(),
                if status.capable { "yes" } else { "no" },
                crate::platform::pcap::MISSING_HELP
            );
        } else {
            info!(
                "Capture helper opened {} devices{}",
                status.opened,
                if status.dropped { ", then gave up its capabilities" } else { "" }
            );
        }
    }

    fn send(&self, control: Control) -> Result<(), String> {
        match self.input.lock().as_mut() {
            Some(input) => input.write_all(&control.encode()).map_err(|e| e.to_string()),
            None => Err("the capture helper is stopping".into()),
        }
    }

    /// Ask the helper to quit, then close its input.
    fn stop(&self) {
        let _ = self.send(Control::Stop);
        self.input.lock().take();
    }

    /// Whether the helper runs and has a device open.
    fn can_capture(&self) -> bool {
        self.alive.load(Ordering::SeqCst) && self.status.lock().is_some_and(|s| s.opened > 0)
    }

    fn set_filter_port(&self, port: Option<u16>) {
        let _ = self.send(Control::SetFilterPort(port));
    }

    fn list_device_labels(&self) -> Result<Vec<String>, String> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err("the capture helper is not running".into());
        }
        let _one = self.asking.lock();
        let (answer, labels) = std::sync::mpsc::channel();
        *self.devices.lock() = Some(answer);
        self.send(Control::ListDevices)?;
        labels.recv_timeout(DEVICES_TIMEOUT).map_err(|e| e.to_string())
    }
}

/// Keeps a capture helper running. `notify(false)` when capture is not
/// available (the helper will not start, opens no device, or quits before
/// capturing), `notify(true)` when it is again after that.
pub struct Supervisor {
    path: PathBuf,
    sender: mpsc::Sender<CapturedPayload>,
    notify: Box<dyn Fn(bool) + Send + Sync>,
    /// The locked port, sent to each helper it starts. Locked before `current`.
    port: Mutex<Option<u16>>,
    current: Mutex<Option<Arc<Helper>>>,
    stopping: AtomicBool,
}

impl Supervisor {
    pub fn start(
        path: PathBuf,
        sender: mpsc::Sender<CapturedPayload>,
        notify: impl Fn(bool) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let supervisor = Arc::new(Self {
            path,
            sender,
            notify: Box::new(notify),
            port: Mutex::new(None),
            current: Mutex::new(None),
            stopping: AtomicBool::new(false),
        });
        let runner = supervisor.clone();
        if let Err(e) = std::thread::Builder::new().name("capture-supervisor".into()).spawn(move || runner.run()) {
            error!("Packet capture is off: {e}");
            (supervisor.notify)(false);
        }
        supervisor
    }

    fn run(&self) {
        let mut wait = RESTART_FIRST;
        let mut notified = false;
        while !self.stopping.load(Ordering::SeqCst) {
            let (events, received) = std::sync::mpsc::channel();
            let started = match Helper::start(&self.path, self.sender.clone(), events) {
                Ok(helper) => {
                    let port = self.port.lock();
                    *self.current.lock() = Some(helper.clone());
                    if port.is_some() {
                        helper.set_filter_port(*port);
                    }
                    drop(port);
                    let outcome = self.watch(&helper, &received, &mut notified);
                    self.current.lock().take();
                    outcome
                }
                Err(e) => {
                    error!(
                        "Packet capture is off: the capture helper did not start ({}). {}",
                        e,
                        crate::platform::pcap::MISSING_HELP
                    );
                    Outcome::Failed
                }
            };
            if self.stopping.load(Ordering::SeqCst) {
                break;
            }
            match started {
                Outcome::DevicesChanged => {
                    wait = RESTART_FIRST;
                    continue;
                }
                Outcome::Failed => {
                    if !notified {
                        notified = true;
                        (self.notify)(false);
                    }
                }
                Outcome::Captured(ran) if ran >= RAN_WELL => wait = RESTART_FIRST,
                Outcome::Captured(_) => {}
            }
            info!("Starting the capture helper again in {} s", wait.as_secs());
            std::thread::sleep(wait);
            wait = (wait * 2).min(RESTART_MOST);
        }
    }

    /// Until `helper` stops: say when it captures, and stop it when the
    /// devices change while no port is locked.
    fn watch(&self, helper: &Helper, events: &std::sync::mpsc::Receiver<Event>, notified: &mut bool) -> Outcome {
        let mut capturing: Option<Instant> = None;
        let mut devices_changed = false;
        loop {
            match events.recv_timeout(DEVICE_POLL) {
                Ok(Event::Status(status)) => {
                    if status.opened > 0 {
                        capturing = Some(Instant::now());
                        if *notified {
                            *notified = false;
                            (self.notify)(true);
                        }
                    }
                }
                Ok(Event::Exited) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    if devices_changed || capturing.is_none() || self.port.lock().is_some() {
                        continue;
                    }
                    let found = helper.found.lock().clone();
                    if let (Some(found), Ok(now)) = (found, helper.list_device_labels()) {
                        let before: BTreeSet<_> = found.iter().collect();
                        let after: BTreeSet<_> = now.iter().collect();
                        if before != after {
                            info!("Capture devices changed ({:?} -> {:?}): starting the capture helper again", before, after);
                            devices_changed = true;
                            helper.stop();
                        }
                    }
                }
            }
        }
        match (devices_changed, capturing) {
            (true, _) => Outcome::DevicesChanged,
            (false, Some(since)) => Outcome::Captured(since.elapsed()),
            (false, None) => Outcome::Failed,
        }
    }

    fn helper(&self) -> Option<Arc<Helper>> {
        self.current.lock().clone()
    }

    pub fn set_filter_port(&self, port: Option<u16>) {
        let mut locked = self.port.lock();
        *locked = port;
        if let Some(helper) = self.helper() {
            helper.set_filter_port(port);
        }
    }

    pub fn list_device_labels(&self) -> Result<Vec<String>, String> {
        self.helper().ok_or("the capture helper is not running")?.list_device_labels()
    }

    pub fn can_capture(&self) -> bool {
        self.helper().is_some_and(|helper| helper.can_capture())
    }

    /// The capture state in a few words, for a bug report.
    pub fn state(&self) -> String {
        let Some(helper) = self.helper() else {
            return "unavailable (the capture helper is not running)".into();
        };
        if !helper.alive.load(Ordering::SeqCst) {
            return "unavailable (the capture helper stopped)".into();
        }
        match *helper.status.lock() {
            None => "helper running, no status yet".into(),
            Some(status) if !status.capable => "unavailable (the helper has no capture permission)".into(),
            Some(status) if status.opened == 0 => "unavailable (helper running, no device opened)".into(),
            Some(status) => format!(
                "helper running, {} device{} opened",
                status.opened,
                if status.opened == 1 { "" } else { "s" }
            ),
        }
    }

    /// Stop the helper for good: the meter is quitting.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(helper) = self.helper() {
            helper.stop();
        }
    }
}

enum Outcome {
    /// It never had a device open.
    Failed,
    /// It captured for this long.
    Captured(Duration),
    /// Stopped to open the devices there are now.
    DevicesChanged,
}
