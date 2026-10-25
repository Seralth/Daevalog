//! The meter's side of the capture helper (`daevalog-capture`, in
//! `capture-helper/`): starts it from the meter's own folder, feeds its packets
//! to the dispatcher, and passes on filter changes. The helper holds the
//! capture permission, so the meter holds none.
//!
//! The helper opens its devices once, at start, and then gives up the
//! permission, so it cannot open a device that shows up later, or one that
//! went away and came back. `Supervisor` starts it again when it dies; and
//! for devices that are listed but read by no helper (`to_open`: Wi-Fi that
//! came up after the meter did, an Ethernet cable plugged in, a VPN that
//! switched servers) it starts one more helper that reads only those, so
//! the devices already read, the game's among them, never stop.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::{Duration, Instant};

use daevalog_capture::wire::{read_frame, Control, Report, Status, LOCKED_ARG, MAX_FRAME, ONLY_ARG};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::tcp_chunk::TcpChunk;
use super::in_process::{log_line, to_payload, DropCounter};

/// How long the device list may take.
const DEVICES_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the device list is compared with the one the helper opened:
/// a VPN that switches servers is back in a few seconds, and capture should
/// be too. The list is a few system calls.
const DEVICE_POLL: Duration = Duration::from_secs(2);

/// While the game's device or the picked one is gone, how often to look for
/// it, and for how long. During a VPN blip the game waits on the server, and
/// TCP brings the backlog in a burst right after the device is back: capture
/// must be open again by then.
const LOST_POLL: Duration = Duration::from_millis(200);
const LOST_POLL_FOR: Duration = Duration::from_secs(60);

/// A device whose capture fails this soon after it was opened is left alone
/// until it leaves the list, so a device that cannot be read does not start
/// a helper at every look.
const FAILS_AT_ONCE: Duration = Duration::from_secs(1);

/// The wait before starting a helper that stopped: doubled after each try
/// that fails, up to the most.
const RESTART_FIRST: Duration = Duration::from_secs(1);
const RESTART_MOST: Duration = Duration::from_secs(60);

/// A helper that captured this long starts the waits over when it stops.
const RAN_WELL: Duration = Duration::from_secs(60);

/// What a helper's reader tells the supervisor, with the helper's number:
/// 0 is the one that opens every device, the others read only some.
enum Event {
    Status(u32, Status),
    /// A device's capture failed.
    Lost(String),
    Exited(u32),
}

/// The locked port, and the device a packet on it last came on; shared
/// with each helper's reader.
#[derive(Default)]
struct PortSeen {
    /// 0 when none is locked.
    port: AtomicU32,
    device: Mutex<String>,
}

impl PortSeen {
    fn note(&self, segment: &daevalog_capture::Segment) {
        let port = self.port.load(Ordering::Relaxed);
        if port != 0 && (u32::from(segment.src_port) == port || u32::from(segment.dst_port) == port) {
            let mut device = self.device.lock();
            if *device != segment.device {
                device.clone_from(&segment.device);
            }
        }
    }

    /// The device the locked port's packets came on; `None` with no lock.
    fn device(&self) -> Option<String> {
        (self.port.load(Ordering::Relaxed) != 0).then(|| self.device.lock().clone()).filter(|d| !d.is_empty())
    }

}

/// The listed devices (`now`) that no helper reads (`reading`) and that did
/// not fail at once before (`refused`), to open in a helper of their own: at
/// once when one of them is a key device (the one the game's port was on,
/// the picked one), so a VPN that went down and up is read again before
/// the game's backlog comes; else once the list held still for a look
/// (`settled`), so a device that flaps (a phone's USB tether) does not start
/// a helper at each flap.
fn to_open(now: &[String], reading: &BTreeSet<String>, refused: &BTreeSet<String>, key: &[&str], settled: bool) -> Vec<String> {
    let missing: Vec<String> = now.iter().filter(|d| !reading.contains(*d) && !refused.contains(*d)).cloned().collect();
    let has_key = missing.iter().any(|d| key.iter().any(|k| k.trim().eq_ignore_ascii_case(d.trim())));
    if has_key || settled {
        missing
    } else {
        Vec::new()
    }
}

struct Helper {
    id: u32,
    path: PathBuf,
    /// The devices it reads; empty: all.
    only: Vec<String>,
    /// `None` once closed: the helper quits when its input ends.
    input: Mutex<Option<ChildStdin>>,
    status: Mutex<Option<Status>>,
    alive: AtomicBool,
    /// The devices the helper found when it started, from the list it sends
    /// before its status.
    found: Mutex<Option<Vec<String>>>,
    /// One device-list question at a time, and where its answer goes.
    asking: Mutex<()>,
    devices: Mutex<DeviceAnswers>,
}

/// Device-list questions the helper has not answered yet. Its answers carry
/// no id but come in order, so only the answer to the newest one is passed
/// on; an answer that came too late is dropped.
#[derive(Default)]
struct DeviceAnswers {
    unanswered: usize,
    newest: Option<std::sync::mpsc::Sender<Vec<String>>>,
}

impl DeviceAnswers {
    fn ask(&mut self, answer: std::sync::mpsc::Sender<Vec<String>>) {
        self.unanswered += 1;
        self.newest = Some(answer);
    }

    /// The question never reached the helper.
    fn unask(&mut self) {
        self.unanswered = self.unanswered.saturating_sub(1);
        self.newest = None;
    }

    fn answer(&mut self, labels: Vec<String>) {
        if self.unanswered == 0 {
            return;
        }
        self.unanswered -= 1;
        if self.unanswered == 0 {
            if let Some(answer) = self.newest.take() {
                let _ = answer.send(labels);
            }
        }
    }
}

impl Helper {
    /// Start the helper at `path`, with an empty environment, reading the
    /// devices in `only`, or all of them.
    fn start(
        id: u32,
        path: &Path,
        only: &[String],
        sender: mpsc::Sender<TcpChunk>,
        events: std::sync::mpsc::Sender<Event>,
        seen: Arc<PortSeen>,
    ) -> Result<Arc<Self>, String> {
        let mut command = Command::new(path);
        if seen.port.load(Ordering::Relaxed) != 0 {
            command.arg(LOCKED_ARG);
        }
        for device in only {
            command.arg(ONLY_ARG).arg(device);
        }
        let mut child = command
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
            id,
            path: path.to_path_buf(),
            only: only.to_vec(),
            input: Mutex::new(Some(input)),
            status: Mutex::new(None),
            alive: AtomicBool::new(true),
            found: Mutex::new(None),
            asking: Mutex::new(()),
            devices: Mutex::new(DeviceAnswers::default()),
        });
        if only.is_empty() {
            info!("Capture helper started: {}", path.display());
        } else {
            info!("Capture helper {id} started for {}", only.join(", "));
        }
        let reader = helper.clone();
        std::thread::Builder::new()
            .name("capture-helper".into())
            .spawn(move || reader.read(output, child, sender, events, &seen))
            .map_err(|e| e.to_string())?;
        Ok(helper)
    }

    fn read(
        &self,
        output: ChildStdout,
        mut child: Child,
        sender: mpsc::Sender<TcpChunk>,
        events: std::sync::mpsc::Sender<Event>,
        seen: &PortSeen,
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
                    seen.note(&segment);
                    let label = segment.device.clone();
                    let counter = drops.entry(label.clone()).or_insert_with(DropCounter::new);
                    if let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(to_payload(segment)) {
                        counter.note_drop(&label, crate::clock::now_ms());
                    } else {
                        counter.report(&label, crate::clock::now_ms());
                    }
                }
                Ok(Report::Log(level, text)) => log_line(level, &text),
                Ok(Report::Status(status)) => {
                    self.note_status(status);
                    *self.status.lock() = Some(status);
                    let _ = events.send(Event::Status(self.id, status));
                }
                // Before the status: the devices it found. After: an answer.
                Ok(Report::Devices(labels)) if self.status.lock().is_none() => {
                    *self.found.lock() = Some(labels);
                }
                Ok(Report::Devices(labels)) => self.devices.lock().answer(labels),
                Ok(Report::Lost(label)) => {
                    let _ = events.send(Event::Lost(label));
                }
                Err(e) => break e.to_string(),
            }
        };
        self.alive.store(false, Ordering::SeqCst);
        let _ = child.kill();
        let exit = child.wait().map(|s| s.to_string()).unwrap_or_default();
        if !self.only.is_empty() && reason == "it quit" {
            info!("Capture helper {} stopped: {} gone ({exit})", self.id, self.only.join(", "));
        } else if self.input.lock().is_some() {
            warn!("Capture helper stopped ({reason}, {exit})");
        } else {
            info!("Capture helper stopped, as asked ({exit})");
        }
        let _ = events.send(Event::Exited(self.id));
    }

    fn note_status(&self, status: Status) {
        if status.opened == 0 && !self.only.is_empty() {
            warn!("Capture helper {} opened none of {}", self.id, self.only.join(", "));
        } else if status.opened == 0 {
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

    fn set_filter_port(&self, port: Option<u16>) {
        let _ = self.send(Control::SetFilterPort(port));
    }

    fn list_device_labels(&self) -> Result<Vec<String>, String> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err("the capture helper is not running".into());
        }
        let _one = self.asking.lock();
        let (answer, labels) = std::sync::mpsc::channel();
        self.devices.lock().ask(answer);
        if let Err(e) = self.send(Control::ListDevices) {
            self.devices.lock().unask();
            return Err(e);
        }
        labels.recv_timeout(DEVICES_TIMEOUT).map_err(|e| e.to_string())
    }
}

/// Keeps a capture helper running. `notify(false)` when capture is not
/// available (the helper will not start, opens no device, or quits before
/// capturing), `notify(true)` when it is again after that.
pub struct Supervisor {
    path: PathBuf,
    sender: mpsc::Sender<TcpChunk>,
    notify: Box<dyn Fn(bool) + Send + Sync>,
    /// The locked port, sent to each helper it starts. Locked before `current`.
    port: Mutex<Option<u16>>,
    seen: Arc<PortSeen>,
    current: Mutex<Option<Arc<Helper>>>,
    /// The helpers for devices that came up or back since `current` started.
    extras: Mutex<Vec<Arc<Helper>>>,
    stopping: AtomicBool,
    /// The devices listed last; `None` before the first list.
    listed: Mutex<Option<Vec<String>>>,
}

impl Supervisor {
    pub fn start(
        path: PathBuf,
        sender: mpsc::Sender<TcpChunk>,
        notify: impl Fn(bool) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let supervisor = Arc::new(Self {
            path,
            sender,
            notify: Box::new(notify),
            port: Mutex::new(None),
            seen: Arc::new(PortSeen::default()),
            current: Mutex::new(None),
            extras: Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
            listed: Mutex::new(None),
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
            let started = match Helper::start(0, &self.path, &[], self.sender.clone(), events.clone(), self.seen.clone()) {
                Ok(helper) => {
                    let port = self.port.lock();
                    *self.current.lock() = Some(helper.clone());
                    if port.is_some() {
                        helper.set_filter_port(*port);
                    }
                    drop(port);
                    let outcome = self.watch(&helper, &events, &received, &mut notified);
                    self.current.lock().take();
                    // The next one opens every device again.
                    for extra in self.extras.lock().drain(..) {
                        extra.stop();
                    }
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

    /// Until `helper` stops: say when it captures, and open the devices that
    /// come up or back in helpers of their own (`to_open`).
    fn watch(
        &self,
        helper: &Helper,
        events: &std::sync::mpsc::Sender<Event>,
        received: &std::sync::mpsc::Receiver<Event>,
        notified: &mut bool,
    ) -> Outcome {
        let mut capturing: Option<Instant> = None;
        // The devices some helper reads, and when each was opened.
        let mut reading: HashMap<String, Instant> = HashMap::new();
        let mut refused = BTreeSet::new();
        let mut next_id = 1;
        // The list at the look before, for `to_open`'s `settled`.
        let mut before: Option<BTreeSet<String>> = None;
        // When the game's device or the picked one was lost.
        let mut key_lost: Option<Instant> = None;
        let key = || -> Vec<String> { [self.seen.device(), super::live::wanted_device()].into_iter().flatten().collect() };
        loop {
            let poll = if key_lost.is_some_and(|at| at.elapsed() < LOST_POLL_FOR) { LOST_POLL } else { DEVICE_POLL };
            match received.recv_timeout(poll) {
                Ok(Event::Status(0, status)) => {
                    if status.opened > 0 {
                        capturing = Some(Instant::now());
                        if *notified {
                            *notified = false;
                            (self.notify)(true);
                        }
                    }
                    let found = helper.found.lock().clone();
                    reading = found.iter().flatten().map(|d| (d.clone(), Instant::now())).collect();
                    *self.listed.lock() = found;
                }
                Ok(Event::Status(id, status)) if status.opened == 0 => {
                    for device in self.extra_devices(id) {
                        reading.remove(&device);
                        refused.insert(device);
                    }
                }
                Ok(Event::Status(..)) => {}
                Ok(Event::Lost(label)) => {
                    warn!("Capture on {label} stopped: the device went down or away");
                    if key().iter().any(|k| k.trim().eq_ignore_ascii_case(label.trim())) {
                        key_lost = Some(Instant::now());
                    }
                    if reading.remove(&label).is_some_and(|opened| opened.elapsed() < FAILS_AT_ONCE) {
                        refused.insert(label);
                    }
                }
                Ok(Event::Exited(0)) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Event::Exited(id)) => {
                    // A helper that read only devices that are gone quits.
                    for device in self.extra_devices(id) {
                        reading.remove(&device);
                    }
                    self.extras.lock().retain(|extra| extra.id != id);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if capturing.is_none() {
                continue;
            }
            let Ok(now) = helper.list_device_labels() else { continue };
            refused.retain(|d| now.contains(d));
            let listed: BTreeSet<String> = now.iter().cloned().collect();
            let settled = before.as_ref() == Some(&listed);
            before = Some(listed);
            let read: BTreeSet<String> = reading.keys().cloned().collect();
            let key = key();
            let key: Vec<&str> = key.iter().map(String::as_str).collect();
            let open = to_open(&now, &read, &refused, &key, settled);
            if !open.is_empty() {
                info!("Capture devices came up or back: {}", open.join(", "));
                let id = next_id;
                next_id += 1;
                match Helper::start(id, &self.path, &open, self.sender.clone(), events.clone(), self.seen.clone()) {
                    Ok(extra) => {
                        let port = self.port.lock();
                        if port.is_some() {
                            extra.set_filter_port(*port);
                        }
                        self.extras.lock().push(extra);
                        drop(port);
                        for device in open {
                            reading.insert(device, Instant::now());
                        }
                    }
                    Err(e) => {
                        warn!("Could not start a capture helper for {}: {e}", open.join(", "));
                        refused.extend(open);
                    }
                }
            }
            *self.listed.lock() = Some(now);
        }
        match capturing {
            Some(since) => Outcome::Captured(since.elapsed()),
            None => Outcome::Failed,
        }
    }

    fn extra_devices(&self, id: u32) -> Vec<String> {
        self.extras.lock().iter().find(|extra| extra.id == id).map(|extra| extra.only.clone()).unwrap_or_default()
    }

    fn helper(&self) -> Option<Arc<Helper>> {
        self.current.lock().clone()
    }

    pub fn set_filter_port(&self, port: Option<u16>) {
        let mut locked = self.port.lock();
        *locked = port;
        self.seen.port.store(port.map_or(0, u32::from), Ordering::Relaxed);
        if let Some(helper) = self.helper() {
            helper.set_filter_port(port);
        }
        for extra in self.extras.lock().iter() {
            extra.set_filter_port(port);
        }
    }

    pub fn list_device_labels(&self) -> Result<Vec<String>, String> {
        self.helper().ok_or("the capture helper is not running")?.list_device_labels()
    }

    /// The devices as last listed, every `DEVICE_POLL`; `None` before the
    /// helper's first list.
    pub fn listed_devices(&self) -> Option<Vec<String>> {
        self.listed.lock().clone()
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
        for extra in self.extras.lock().iter() {
            extra.stop();
        }
    }
}

enum Outcome {
    /// It never had a device open.
    Failed,
    /// It captured for this long.
    Captured(Duration),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_device_that_went_away_and_came_back_is_opened_at_once_when_it_matters() {
        // The game's VPN went down and up within a look, its connection
        // alive: the list as it was, but no helper reads vpn0 any more.
        let now = list(&["lo", "eth0", "vpn0"]);
        let reading = set(&["lo", "eth0"]);
        let none = BTreeSet::new();
        assert_eq!(to_open(&now, &reading, &none, &["vpn0"], false), list(&["vpn0"]), "the game's device: at once");
        assert_eq!(to_open(&now, &reading, &none, &["eth0", "VPN0"], false), list(&["vpn0"]), "the picked one too");
        assert!(to_open(&now, &reading, &none, &[], false).is_empty(), "any other: once the list holds still");
        assert_eq!(to_open(&now, &reading, &none, &[], true), list(&["vpn0"]));
        // Failed at once before: not again until it leaves the list.
        assert!(to_open(&now, &reading, &set(&["vpn0"]), &["vpn0"], true).is_empty());
        // Read already: nothing to do.
        assert!(to_open(&now, &set(&["lo", "eth0", "vpn0"]), &none, &["vpn0"], true).is_empty());
    }

    #[test]
    fn a_new_device_is_opened_once_the_list_holds_still() {
        // An Ethernet cable on a laptop on Wi-Fi; the game is on wlan0.
        let reading = set(&["lo", "wlan0"]);
        let now = list(&["lo", "wlan0", "eth0"]);
        let none = BTreeSet::new();
        assert!(to_open(&now, &reading, &none, &["wlan0"], false).is_empty(), "first look");
        assert_eq!(to_open(&now, &reading, &none, &["wlan0"], true), list(&["eth0"]), "next look, the same list");
        assert!(to_open(&list(&["lo", "wlan0"]), &reading, &none, &["wlan0"], true).is_empty(), "nothing new");
    }

    #[test]
    fn a_late_device_list_goes_to_no_one() {
        let mut answers = DeviceAnswers::default();
        let (first, first_got) = std::sync::mpsc::channel();
        answers.ask(first);
        // The first question timed out; a second is asked.
        let (second, second_got) = std::sync::mpsc::channel();
        answers.ask(second);
        answers.answer(list(&["lo"]));
        assert!(second_got.try_recv().is_err(), "the first answer is not the second's");
        answers.answer(list(&["lo", "vpn0"]));
        assert_eq!(second_got.try_recv().unwrap(), list(&["lo", "vpn0"]));
        assert!(first_got.try_recv().is_err());
        answers.answer(list(&["lo"]));
        assert_eq!(answers.unanswered, 0, "an answer no one asked for is dropped");
    }

    #[test]
    fn the_device_the_game_port_was_on_is_remembered() {
        let seen = PortSeen::default();
        let segment = |device: &str, port: u16| daevalog_capture::Segment {
            device: device.into(),
            captured_at_ms: 0,
            src_ip: [10, 2, 0, 1],
            dst_ip: [10, 2, 0, 2],
            src_port: port,
            dst_port: 51174,
            seq: 0,
            ack: 0,
            data: vec![],
        };
        seen.note(&segment("vpn0", 13328));
        assert_eq!(seen.device(), None, "no lock");
        seen.port.store(13328, Ordering::Relaxed);
        seen.note(&segment("eth0", 443));
        seen.note(&segment("vpn0", 13328));
        assert_eq!(seen.device().as_deref(), Some("vpn0"));
    }
}
