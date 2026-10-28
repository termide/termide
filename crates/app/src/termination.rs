//! Ending termide from outside, by a signal.
//!
//! - SIGHUP: the terminal went away — its window closed, the SSH connection
//!   dropped. A detached instance gets it only if its host daemon dies; a
//!   client detaching leaves the host's PTY open, so no hangup reaches it.
//! - SIGTERM: `termide --kill` (which sends SIGKILL after a grace period),
//!   `kill`, a service manager or a logout.
//! - SIGINT: from another process. In raw mode Ctrl+C is a key, not a signal.
//!
//! Each would otherwise kill the process on the spot. Here the signal is only
//! recorded; the event loop sees it within a tick and ends through the
//! ordinary shutdown, except that nobody is asked anything: unsaved changes
//! are kept rather than discarded (see `App::run`). A repeat of the signal
//! changes nothing — a closing terminal can deliver SIGHUP twice, once from
//! the shell and once from the kernel, and the second must not cut the first
//! short.
//!
//! On Unix a watchdog thread receives the signals and sees the loop out. A
//! terminal that hung up reads as end-of-file forever, and crossterm's reader
//! retries such a read without end (only `WouldBlock` stops it), so the loop
//! would never get to look at the flag. If the loop has not stopped shortly
//! after the signal, the watchdog puts an empty, non-blocking pipe in place
//! of standard input, which is what crossterm reads from; its next read
//! reports `WouldBlock` and the loop gets its tick. Should the loop still not
//! stop, the process exits as the signal would have ended it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

/// The signal that asked termide to end, 0 while none has.
fn slot() -> &'static Arc<AtomicUsize> {
    static SLOT: OnceLock<Arc<AtomicUsize>> = OnceLock::new();
    SLOT.get_or_init(|| Arc::new(AtomicUsize::new(0)))
}

/// Set once the event loop has stopped, whatever stopped it.
static LOOP_STOPPED: AtomicBool = AtomicBool::new(false);

/// The signal that had come when the loop stopped, 0 for none: one arriving
/// during the shutdown after a quit did not end the run.
static ENDED_BY: AtomicUsize = AtomicUsize::new(0);

/// The signals handled, by number and name.
#[cfg(unix)]
const SIGNALS: [(i32, &str); 3] = [
    (signal_hook::consts::SIGHUP, "SIGHUP"),
    (signal_hook::consts::SIGTERM, "SIGTERM"),
    (signal_hook::consts::SIGINT, "SIGINT"),
];
#[cfg(not(unix))]
const SIGNALS: [(i32, &str); 2] = [
    (signal_hook::consts::SIGTERM, "SIGTERM"),
    (signal_hook::consts::SIGINT, "SIGINT"),
];

/// How long the loop has to notice a signal on its own: a few ticks.
#[cfg(unix)]
const LOOP_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// How long after the signal the loop may still not have stopped before the
/// process exits without its shutdown. Below the 3 s `termide --kill` gives.
#[cfg(unix)]
const EXIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Catch the signals that end termide from outside. Call once, before the
/// event loop starts; until then they keep their default, fatal action.
///
/// # Errors
///
/// When a handler cannot be registered.
#[cfg(unix)]
pub fn install_handlers() -> std::io::Result<()> {
    let numbers: Vec<i32> = SIGNALS.iter().map(|(number, _)| *number).collect();
    let mut signals = signal_hook::iterator::Signals::new(&numbers)?;
    // crossterm reads standard input when it is a terminal; otherwise it
    // opens /dev/tty, whose descriptor is out of reach.
    // SAFETY: isatty only inspects the descriptor.
    let stdin_is_tty = unsafe { libc::isatty(libc::STDIN_FILENO) } == 1;
    std::thread::Builder::new()
        .name("termination".into())
        .spawn(move || {
            let Some(signal) = signals.forever().next() else {
                return;
            };
            record(signal);
            watch_the_loop_out(signal, stdin_is_tty);
        })?;
    Ok(())
}

/// Catch the signals that end termide from outside. Call once, before the
/// event loop starts; until then they keep their default, fatal action.
///
/// # Errors
///
/// When a handler cannot be registered.
#[cfg(not(unix))]
pub fn install_handlers() -> std::io::Result<()> {
    for (signal, _) in SIGNALS {
        let value = usize::try_from(signal).unwrap_or(usize::MAX);
        signal_hook::flag::register_usize(signal, Arc::clone(slot()), value)?;
    }
    Ok(())
}

fn record(signal: i32) {
    let value = usize::try_from(signal).unwrap_or(usize::MAX);
    slot().store(value, Ordering::SeqCst);
}

/// Give the loop [`LOOP_GRACE`] to stop, free it from a hung-up terminal if
/// it has not, and end the process at [`EXIT_DEADLINE`] if even that failed.
#[cfg(unix)]
fn watch_the_loop_out(signal: i32, stdin_is_tty: bool) {
    let started = std::time::Instant::now();
    std::thread::sleep(LOOP_GRACE);
    if LOOP_STOPPED.load(Ordering::SeqCst) {
        return;
    }
    if stdin_is_tty {
        match detach_stdin() {
            Ok(()) => log::info!("Standard input replaced to let the event loop see the signal"),
            Err(e) => log::warn!("Could not replace standard input: {e}"),
        }
    }
    std::thread::sleep(EXIT_DEADLINE.saturating_sub(started.elapsed()));
    if !LOOP_STOPPED.load(Ordering::SeqCst) {
        log::error!(
            "The event loop did not stop after {}; exiting without the shutdown",
            name(signal)
        );
        // SAFETY: `_exit` ends the process at once, which is the point: the
        // main thread is stuck and nothing it owns can be relied upon.
        unsafe { libc::_exit(exit_status(signal)) };
    }
}

/// Put the read end of a new, empty, non-blocking pipe in place of standard
/// input. Its write end is never closed, so a read reports `WouldBlock`
/// rather than end-of-file.
#[cfg(unix)]
fn detach_stdin() -> std::io::Result<()> {
    let mut fds = [0; 2];
    // SAFETY: plain descriptor calls on descriptors created right here;
    // `dup2` atomically replaces standard input, which nothing else needs
    // once the terminal is gone.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let flags = libc::fcntl(fds[0], libc::F_GETFL);
        if flags < 0 || libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::dup2(fds[0], libc::STDIN_FILENO) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        libc::close(fds[0]);
    }
    // fds[1] stays open for the rest of the process on purpose.
    Ok(())
}

/// The signal that asked termide to end, if one has.
#[must_use]
pub fn received() -> Option<i32> {
    match slot().load(Ordering::SeqCst) {
        0 => None,
        signal => i32::try_from(signal).ok(),
    }
}

/// Tell the watchdog that the event loop has stopped and the shutdown is
/// under way, which it must not interrupt.
pub fn loop_stopped() {
    ENDED_BY.store(slot().load(Ordering::SeqCst), Ordering::SeqCst);
    LOOP_STOPPED.store(true, Ordering::SeqCst);
}

/// The signal that ended the run: one received before the event loop
/// stopped.
#[must_use]
pub fn ended_by() -> Option<i32> {
    match ENDED_BY.load(Ordering::SeqCst) {
        0 => None,
        signal => i32::try_from(signal).ok(),
    }
}

/// A signal's name for the journal, `signal 12` for one not handled here.
#[must_use]
pub fn name(signal: i32) -> String {
    SIGNALS
        .iter()
        .find(|(number, _)| *number == signal)
        .map_or_else(
            || format!("signal {signal}"),
            |(_, name)| (*name).to_string(),
        )
}

/// The exit status of a process that ends because of `signal`, as shells
/// report one killed by it: 128 + its number. Never 0, so that a caller
/// such as `git commit` with termide as its editor does not take an edit
/// cut short for a finished one.
#[must_use]
pub fn exit_status(signal: i32) -> i32 {
    128 + signal
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A signal that would kill termide is recorded instead, and reported
    /// by name and with the status a shell gives a process it killed.
    #[test]
    fn a_hangup_is_recorded_not_fatal() {
        // The watchdog must not end the test process.
        loop_stopped();
        install_handlers().unwrap();
        signal_hook::low_level::raise(signal_hook::consts::SIGHUP).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while received().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let signal = received().expect("the hangup was recorded");
        assert_eq!(signal, signal_hook::consts::SIGHUP);
        assert_eq!(name(signal), "SIGHUP");
        assert_eq!(exit_status(signal), 129);
    }

    /// Standard input replaced by the watchdog reads as "nothing yet", not
    /// as end-of-file, which is what lets crossterm's reader return.
    #[test]
    fn a_replaced_stdin_would_block() {
        // SAFETY: the test's standard input is saved and put back around it.
        let saved = unsafe { libc::dup(libc::STDIN_FILENO) };
        detach_stdin().unwrap();
        let mut byte = [0u8; 1];
        let read = unsafe { libc::read(libc::STDIN_FILENO, byte.as_mut_ptr().cast(), 1) };
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::dup2(saved, libc::STDIN_FILENO);
            libc::close(saved);
        }
        assert_eq!(read, -1);
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }
}
