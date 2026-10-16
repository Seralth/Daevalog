// SPDX-License-Identifier: AGPL-3.0-only

//! `Account`, the QML singleton for Settings > Account: signing in
//! to a2tools.app with the device code the site gives (approved in the
//! browser), signing out on this machine, who is signed in, and the
//! auto-upload setting; and the automatic uploads the live feed starts. Every
//! rule and request is the backend's (`daevalog::account`, `daevalog::share`),
//! as 1.x calls them. Nothing is sent but on the player's word: a sign-in they
//! started, an upload they asked for, or auto-upload they switched on.

use core::pin::Pin;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use daevalog::account::{self, AccountState};
use daevalog::entity::fight_record::FightRecord;
use daevalog::history::fight_history::FightHistoryManager;
use daevalog::history::live_save::Saved;
use daevalog::share;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, state, READ, NOTIFY)]
        #[qproperty(QString, name, READ, NOTIFY)]
        #[qproperty(bool, supporter, READ, NOTIFY)]
        #[qproperty(QString, message, READ, NOTIFY)]
        #[qproperty(QString, code, READ, NOTIFY)]
        #[qproperty(QString, link, READ, NOTIFY)]
        #[qproperty(bool, auto_upload, READ, WRITE = change_auto_upload, NOTIFY)]
        #[qproperty(QString, upload_failure, READ, NOTIFY)]
        type Account = super::AccountRust;

        fn change_auto_upload(self: Pin<&mut Account>, value: bool);

        /// Asks a2tools.app who is signed in, and reads the settings again.
        #[qinvokable]
        fn refresh(self: Pin<&mut Account>);

        /// Starts a sign-in: the code to approve comes in `code`, the page
        /// to approve it on in `link`; the outcome in `state`.
        #[qinvokable]
        fn sign_in(self: Pin<&mut Account>);

        /// Forgets the sign-in on this machine. Revoking it everywhere is
        /// done on a2tools.app, where every device is listed.
        #[qinvokable]
        fn sign_out(self: Pin<&mut Account>);
    }

    impl cxx_qt::Threading for Account {}
}

/// What the Account page shows: `state` is "checking", "signedOut",
/// "signedIn", "unavailable" (a sign-in is kept but could not be checked:
/// `message` says why), "waiting" (for the browser: `code`, `link`) or
/// "failed" (`message`). `upload_failure` is why the last automatic upload
/// failed, empty after one that went up.
pub struct AccountRust {
    state: QString,
    name: QString,
    supporter: bool,
    message: QString,
    code: QString,
    link: QString,
    auto_upload: bool,
    upload_failure: QString,
}

impl Default for AccountRust {
    fn default() -> Self {
        Self {
            state: QString::from("checking"),
            name: QString::default(),
            supporter: false,
            message: QString::default(),
            code: QString::default(),
            link: QString::default(),
            auto_upload: auto_upload_on(),
            upload_failure: QString::default(),
        }
    }
}

macro_rules! show {
    ($this:ident, $field:ident, $signal:ident, $value:expr) => {{
        let value = $value;
        if $this.$field != value {
            $this.as_mut().rust_mut().$field = value;
            $this.as_mut().$signal();
        }
    }};
}

/// The data folder, where the sign-in is kept (sealed by the desktop's
/// keyring) and the fights and their uploads are.
fn folder() -> Option<PathBuf> {
    crate::settings_store::folder()
}

/// Runs an account request to its end on this thread.
fn block_on<F: std::future::Future>(request: F) -> Option<F::Output> {
    match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => Some(runtime.block_on(request)),
        Err(error) => {
            eprintln!("daevalog-dps-meter: no runtime for the account: {error}");
            None
        }
    }
}

/// The interface language a2tools.app is told with an upload, as 1.x saves
/// it: it tells Korean servers from Taiwanese ones.
pub fn ui_language() -> String {
    crate::settings_store::store().raw("dpsMeter.language").unwrap_or_default()
}

/// Whether finished boss fights are uploaded on their own.
pub fn auto_upload_on() -> bool {
    crate::settings_store::store().raw(share::AUTO_UPLOAD_KEY).as_deref() == Some("true")
}

/// The account's name as the site sends it: `displayName`, or
/// `display_name`, which the summary keeps among its other fields (1.x's
/// page reads both).
fn display_name(summary: &account::AccountSummary) -> String {
    summary
        .display_name
        .clone()
        .or_else(|| summary.extra.get("display_name").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default()
}

impl qobject::Account {
    fn change_auto_upload(mut self: Pin<&mut Self>, value: bool) {
        crate::settings_store::store().set_raw(share::AUTO_UPLOAD_KEY, if value { "true" } else { "false" });
        show!(self, auto_upload, auto_upload_changed, value);
    }

    /// Who is signed in, as `state` and `name`; a sign-in waiting for the
    /// browser keeps its code until its outcome comes.
    fn show_who(mut self: Pin<&mut Self>, who: AccountState) {
        if self.state.to_string() == "waiting" {
            return;
        }
        let (state, name, supporter, message) = match who {
            AccountState::SignedIn(summary) => ("signedIn", display_name(&summary), summary.supporter, String::new()),
            AccountState::SignedOut => ("signedOut", String::new(), false, String::new()),
            AccountState::Unavailable(why) => ("unavailable", String::new(), false, why),
        };
        show!(self, name, name_changed, QString::from(&name));
        show!(self, supporter, supporter_changed, supporter);
        show!(self, message, message_changed, QString::from(&message));
        show!(self, state, state_changed, QString::from(state));
    }

    fn refresh(mut self: Pin<&mut Self>) {
        show!(self, auto_upload, auto_upload_changed, auto_upload_on());
        show!(self, upload_failure, upload_failure_changed, QString::from(&last_failure(None)));
        let Some(dir) = folder() else { return };
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let Some(who) = block_on(account::whoami(account::client(), &dir)) else { return };
            let _ = qt.queue(move |account| account.show_who(who));
        });
    }

    fn sign_in(mut self: Pin<&mut Self>) {
        let Some(dir) = folder() else { return };
        if self.state.to_string() == "waiting" {
            return;
        }
        show!(self, code, code_changed, QString::default());
        show!(self, link, link_changed, QString::default());
        show!(self, message, message_changed, QString::default());
        show!(self, state, state_changed, QString::from("waiting"));
        let qt = self.qt_thread();
        std::thread::spawn(move || {
            let outcome = block_on(async {
                let grant = account::start(account::client(), &account::device_label()).await?;
                let code = grant.user_code.clone();
                // Only a page on a2tools.app is opened; the code shows either way.
                let link = if account::is_site_url(&grant.verification_uri_complete) {
                    grant.verification_uri_complete.clone()
                } else {
                    String::new()
                };
                let _ = qt.queue(move |mut account| {
                    show!(account, code, code_changed, QString::from(&code));
                    show!(account, link, link_changed, QString::from(&link));
                });
                account::poll_until_decided(account::client(), &grant, &dir).await
            });
            let _ = qt.queue(move |mut account| {
                show!(account, code, code_changed, QString::default());
                show!(account, link, link_changed, QString::default());
                match outcome {
                    Some(Ok(())) => {
                        show!(account, state, state_changed, QString::from("checking"));
                        account.refresh();
                    }
                    Some(Err(why)) => {
                        show!(account, message, message_changed, QString::from(&why));
                        show!(account, state, state_changed, QString::from("failed"));
                    }
                    None => show!(account, state, state_changed, QString::from("failed")),
                }
            });
        });
    }

    fn sign_out(mut self: Pin<&mut Self>) {
        if let Some(dir) = folder() {
            account::secret::clear(&dir);
        }
        self.as_mut().rust_mut().state = QString::default();
        self.show_who(AccountState::SignedOut);
    }
}

/// Why the last automatic upload failed, empty after one that went up, for
/// Settings to show under the switch; `Some` sets it.
fn last_failure(set: Option<String>) -> String {
    static LAST: Mutex<String> = Mutex::new(String::new());
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(text) = set {
        *last = text;
    }
    last.clone()
}

/// Fights with an automatic upload running: one at a time per fight, so a
/// retry cannot start while the first try still waits on the network.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// After an auto-save: uploads the finished boss fights it wrote, and the
/// failed uploads due again, when auto-upload is on, as 1.x's auto-save does.
/// Each upload runs on a thread of its own; a failure is quiet and waits for
/// its retry (`share::note_auto_upload_failure`).
pub fn auto_upload(dir: &Path, history: &'static FightHistoryManager, saved: Option<&Saved>) {
    if !auto_upload_on() {
        return;
    }
    if let Some(saved) = saved {
        for record in saved.records.iter().filter(|r| share::wants_auto_upload(dir, r, saved.finished.contains(&r.id))) {
            upload_in_background(dir.to_path_buf(), record.clone());
        }
    }
    // Fighting or not: a meter left open after the connection came back
    // catches up on its own.
    for id in share::auto_upload_retries_due(dir, daevalog::clock::now_ms()) {
        if let Ok(record) = history.load_fight(&id) {
            upload_in_background(dir.to_path_buf(), record);
        }
    }
}

/// A fight's place among the uploads running, given up on drop: every way
/// out of the upload thread clears it, a panic included.
struct InFlight(String);

impl InFlight {
    fn start(id: &str) -> Option<Self> {
        in_flight().lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string()).then(|| Self(id.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        in_flight().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

fn upload_in_background(dir: PathBuf, record: FightRecord) {
    let Some(held) = InFlight::start(&record.id) else { return };
    std::thread::spawn(move || {
        let _held = held;
        let outcome = block_on(share::upload_detailed(account::client(), &dir, &ui_language(), &record));
        match outcome {
            Some(Ok(result)) => {
                eprintln!("daevalog-dps-meter: uploaded {} -> {}", record.id, result.url);
                last_failure(Some(String::new()));
                crate::history::uploaded_elsewhere();
            }
            Some(Err(failure)) => {
                share::note_auto_upload_failure(&dir, &record.id, &failure, daevalog::clock::now_ms());
                last_failure(Some(failure.message.clone()));
                let again = if failure.retryable { ", will retry" } else { "" };
                eprintln!("daevalog-dps-meter: the upload of {} failed{again}: {}", record.id, failure.message);
            }
            None => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_comes_under_either_spelling() {
        let summary = |json: &str| serde_json::from_str::<account::AccountSummary>(json).unwrap();
        assert_eq!(display_name(&summary(r#"{"displayName":"Ann"}"#)), "Ann");
        assert_eq!(display_name(&summary(r#"{"display_name":"Bo"}"#)), "Bo");
        assert_eq!(display_name(&summary(r#"{"supporter":true}"#)), "");
    }

    #[test]
    fn the_last_auto_upload_failure_is_kept_until_one_goes_up() {
        assert_eq!(last_failure(Some("offline".into())), "offline");
        assert_eq!(last_failure(None), "offline");
        assert_eq!(last_failure(Some(String::new())), "");
    }

    #[test]
    fn one_upload_of_a_fight_at_a_time_and_a_panic_frees_it() {
        let held = InFlight::start("account-test").unwrap();
        assert!(InFlight::start("account-test").is_none());
        drop(held);
        let outcome = std::panic::catch_unwind(|| {
            let _held = InFlight::start("account-test").unwrap();
            panic!("upload thread panicked");
        });
        assert!(outcome.is_err());
        assert!(InFlight::start("account-test").is_some());
    }
}
