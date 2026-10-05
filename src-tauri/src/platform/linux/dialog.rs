//! Running GTK calls on the thread that runs GTK's main loop.
//!
//! GTK may only be touched from that thread, and callers ask from worker
//! threads, so each call is handed to the loop and the caller waits for the
//! answer.

pub(super) fn on_gtk_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    // Runs at once when called from the GTK thread itself, so no deadlock.
    gtk::glib::MainContext::default().invoke(move || {
        let _ = tx.send(f());
    });
    rx.recv().ok()
}
