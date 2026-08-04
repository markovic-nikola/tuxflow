//! Run blocking work (ssh, git, disk) off the GTK main thread.

use gtk4::glib;

/// Run `work` on a worker thread and hand its result to `done` back on the
/// GTK main thread. Must be called from the main thread.
///
/// This is the one sanctioned bridge for blocking calls: the async receiver
/// suspends until the result arrives, unlike `idle_add_local` + `try_recv`
/// polling, which busy-spins the main loop for the whole wait.
pub fn run<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    glib::spawn_future_local(async move {
        if let Ok(result) = rx.await {
            done(result);
        }
    });
}
