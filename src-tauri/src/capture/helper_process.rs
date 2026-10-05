//! The meter's side of the capture helper (`daevalog-capture`, in
//! `capture-helper/`): starts it from the meter's own folder, feeds its packets
//! to the dispatcher, and passes on filter changes. The helper holds the
//! capture permission, so the meter holds none.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daevalog_capture::wire::{read_frame, Control, Report, Status, MAX_FRAME};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::{error, info};

use super::captured_payload::CapturedPayload;
use super::pcap_capturer::{log_line, to_payload, DropCounter};

/// How long the device list may take.
const DEVICES_TIMEOUT: Duration = Duration::from_secs(3);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub struct Helper {
    path: PathBuf,
    input: Mutex<ChildStdin>,
    status: Mutex<Option<Status>>,
    alive: AtomicBool,
    /// One device-list question at a time, and where its answer goes.
    asking: Mutex<()>,
    devices: Mutex<Option<std::sync::mpsc::Sender<Vec<String>>>>,
}

impl Helper {
    /// Start the helper at `path`, with an empty environment.
    pub fn start(path: &Path, sender: mpsc::Sender<CapturedPayload>) -> Result<Arc<Self>, String> {
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
            input: Mutex::new(input),
            status: Mutex::new(None),
            alive: AtomicBool::new(true),
            asking: Mutex::new(()),
            devices: Mutex::new(None),
        });
        info!("Capture helper started: {}", path.display());
        let reader = helper.clone();
        std::thread::Builder::new()
            .name("capture-helper".into())
            .spawn(move || reader.read(output, child, sender))
            .map_err(|e| e.to_string())?;
        Ok(helper)
    }

    fn read(&self, output: ChildStdout, mut child: Child, sender: mpsc::Sender<CapturedPayload>) {
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
        error!("Capture helper stopped ({reason}, {exit}); no more packets until the meter restarts");
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
        self.input.lock().write_all(&control.encode()).map_err(|e| e.to_string())
    }

    /// Whether the helper runs and has a device open.
    pub fn can_capture(&self) -> bool {
        self.alive.load(Ordering::SeqCst) && self.status.lock().is_some_and(|s| s.opened > 0)
    }

    pub fn set_filter_port(&self, port: Option<u16>) {
        let _ = self.send(Control::SetFilterPort(port));
    }

    pub fn list_device_labels(&self) -> Result<Vec<String>, String> {
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
