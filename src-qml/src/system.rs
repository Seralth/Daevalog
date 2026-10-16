// SPDX-License-Identifier: AGPL-3.0-only

//! `System`, the QML singleton for what Settings > Diagnostics and Report a
//! problem need: the version, the data folder, the report info, the packet
//! logs, debug.log and their bug-report copies, the game's Combat Analysis
//! folder, and the clipboard. The rules are the backend's
//! (`daevalog::share::report`); nothing here sends anything.

use core::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use daevalog::share::report;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("daevalog-qml/cpp/shell.h");

        #[cxx_name = "copyText"]
        fn copy_text(text: &QString);

        #[cxx_name = "qtLine"]
        fn qt_line() -> QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, version, READ, CONSTANT)]
        #[qproperty(QString, data_folder, READ, CONSTANT)]
        #[qproperty(QString, record_folder, READ, NOTIFY)]
        #[qproperty(QString, report_info, READ, NOTIFY)]
        #[qproperty(QString, packet_logs, READ, NOTIFY)]
        #[qproperty(bool, preparing, READ, NOTIFY)]
        #[qproperty(QString, prepared, READ, NOTIFY)]
        type System = super::SystemRust;

        /// Reads the report info, the packet logs and the Combat Analysis
        /// folder again; `overlay` is how the meter shows ("layer shell").
        #[qinvokable]
        fn refresh(self: Pin<&mut System>, overlay: QString);

        /// Writes the bug-report copy of the packet log `name` (the newest
        /// when empty) in the data folder, on a thread of its own; the
        /// result lands in `prepared`: {"name", "namesKnown"} or {"error"}.
        #[qinvokable]
        fn prepare_log(self: Pin<&mut System>, name: QString);

        /// Writes `report_debug.log`, debug.log with the character names,
        /// the home folder and the user name taken out, the same way.
        #[qinvokable]
        fn prepare_debug_log(self: Pin<&mut System>);

        /// Puts `text` on the clipboard.
        #[qinvokable]
        fn copy(self: &System, text: QString);
    }

    impl cxx_qt::Threading for System {}
}

/// The 2.0 release line; the revision is the repository's, which the
/// backend's version has too ("1.1 · r727"). Set in the backend at the start,
/// so a report and an upload say 2.0 too.
pub const RELEASE: &str = "2.0";

/// "2.0 · r727", or "2.0" when the build knew no revision. Sets the line
/// too, for a caller that comes before `main` sets it (the tests).
pub fn version() -> String {
    daevalog::version::set_release(RELEASE);
    daevalog::version::display()
}

pub struct SystemRust {
    version: QString,
    /// The data folder, where the logs are; empty when there is none.
    data_folder: QString,
    /// The game's Combat Analysis folder; empty when it was not found.
    record_folder: QString,
    report_info: QString,
    /// The packet logs, newest first, as JSON: [{"name", "bytes"}].
    packet_logs: QString,
    preparing: bool,
    prepared: QString,
}

impl Default for SystemRust {
    fn default() -> Self {
        let folder = crate::settings_store::folder().map(|p| p.display().to_string()).unwrap_or_default();
        Self {
            version: QString::from(&version()),
            data_folder: QString::from(&folder),
            record_folder: QString::default(),
            report_info: QString::default(),
            packet_logs: QString::from("[]"),
            preparing: false,
            prepared: QString::default(),
        }
    }
}

impl qobject::System {
    fn refresh(mut self: Pin<&mut Self>, overlay: QString) {
        let display = format!("{}, {}", qobject::qt_line(), overlay);
        let info = report::report_info(&version(), Some(display));
        let folder = std::path::PathBuf::from(self.data_folder.to_string());
        let logs = serde_json::to_string(&report::packet_logs(&folder)).unwrap_or_else(|_| "[]".into());
        let records = daevalog::game_record::files::record_roots()
            .into_iter()
            .next()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.as_mut().rust_mut().report_info = QString::from(&info);
        self.as_mut().report_info_changed();
        self.as_mut().rust_mut().packet_logs = QString::from(&logs);
        self.as_mut().packet_logs_changed();
        self.as_mut().rust_mut().record_folder = QString::from(&records);
        self.as_mut().record_folder_changed();
    }

    fn prepare_log(self: Pin<&mut Self>, name: QString) {
        let name = name.to_string();
        self.prepare(move |folder| report::prepare_log(folder, (!name.is_empty()).then_some(name.as_str())));
    }

    fn prepare_debug_log(self: Pin<&mut Self>) {
        self.prepare(|folder| {
            // Every name the meter knows now; the log's own name lines
            // cover the ones from before.
            let mut names = vec![crate::capture::saved_name()];
            if let Some(feed) = crate::live::feed() {
                names.extend(daevalog::share::names_from(&feed.storage).into_keys());
                names.extend(feed.storage.local_character_name());
            }
            daevalog::share::report_debug::prepare_debug_log(folder, &names)
        });
    }

    /// Runs `make` on a thread of its own; its result lands in `prepared`.
    fn prepare(
        mut self: Pin<&mut Self>,
        make: impl FnOnce(&std::path::Path) -> Result<report::PreparedLog, String> + Send + 'static,
    ) {
        if self.preparing {
            return;
        }
        self.as_mut().rust_mut().preparing = true;
        self.as_mut().preparing_changed();
        let folder = std::path::PathBuf::from(self.data_folder.to_string());
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let json = match make(&folder) {
                Ok(done) => serde_json::json!({ "name": done.name, "namesKnown": done.names_known }),
                Err(error) => serde_json::json!({ "error": error }),
            };
            let json = json.to_string();
            let _ = qt.queue(move |mut system| {
                system.as_mut().rust_mut().preparing = false;
                system.as_mut().preparing_changed();
                system.as_mut().rust_mut().prepared = QString::from(&json);
                system.as_mut().prepared_changed();
            });
        });
    }

    fn copy(&self, text: QString) {
        qobject::copy_text(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_2_0_with_the_repositorys_revision() {
        let v = version();
        assert!(v.starts_with("2.0"), "{v}");
        match daevalog::version::DISPLAY.split_once(" · ") {
            Some((_, revision)) => assert_eq!(v, format!("2.0 · {revision}")),
            None => assert_eq!(v, "2.0"),
        }
    }
}
