//! Terminals being closed: the wait for their programs to end, run off the
//! UI thread, and joined before the process exits so that a program which
//! ignores the hang-up still gets the later signals.

use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static CLOSING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Run `end` — the signals and the wait that end a closed terminal's
/// program — on a thread of its own.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn spawn(end: impl FnOnce() + Send + 'static) {
    let Ok(handle) = std::thread::Builder::new()
        .name("terminal-close".into())
        .spawn(end)
    else {
        return;
    };
    let mut closing = CLOSING.lock().unwrap_or_else(|e| e.into_inner());
    closing.retain(|handle| !handle.is_finished());
    closing.push(handle);
}

/// Wait, at most `limit`, for the terminals being closed to have ended
/// their programs: for the end of the process, after the panels are
/// dropped. A program stuck in the kernel is not waited for past it.
pub fn wait_for_closing_terminals(limit: Duration) {
    let deadline = Instant::now() + limit;
    let handles = std::mem::take(&mut *CLOSING.lock().unwrap_or_else(|e| e.into_inner()));
    for handle in handles {
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = handle.join();
    }
}
