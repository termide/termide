//! The hosted side of a reattach.
//!
//! The daemon signals the termide it hosts with SIGUSR1 when a client
//! attaches. A signal handler can do almost nothing safely, so it only sets a
//! flag; the event loop picks it up on its next tick and does the real work —
//! re-entering terminal modes, refreshing capabilities and repainting.

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{daemon, paths};

static REATTACH_REQUESTED: AtomicBool = AtomicBool::new(false);

/// SIGUSR1 handler.
///
/// Storing to an atomic is async-signal-safe; nothing else here is, which is
/// why the handler does nothing else.
extern "C" fn on_sigusr1(_signal: libc::c_int) {
    REATTACH_REQUESTED.store(true, Ordering::SeqCst);
}

/// Block SIGUSR1 so that it can never act on its default disposition.
///
/// The default disposition of SIGUSR1 is to *terminate the process*. The
/// daemon signals a hosted termide the moment a client attaches, which can be
/// long before that termide has finished starting and installed a handler — so
/// the signal has to be blocked from the very first instruction, not merely
/// handled later. The daemon calls this before spawning, and the mask survives
/// both fork and exec, so the hosted process is born with SIGUSR1 blocked.
///
/// A signal that arrives while blocked stays pending and is delivered the
/// moment [`install_handler`] unblocks it, so an early attach is not lost
/// either: it repaints as soon as the app is able to.
pub fn block_signal() -> Result<()> {
    let mut mask = nix::sys::signal::SigSet::empty();
    mask.add(nix::sys::signal::Signal::SIGUSR1);
    mask.thread_block().context("Failed to block SIGUSR1")
}

/// Start listening for reattach signals. A no-op outside a detached instance.
///
/// Call this as early as possible: until it runs SIGUSR1 stays blocked, and an
/// attach that happened meanwhile is sitting pending, waiting for this call.
pub fn install_handler() -> Result<()> {
    if !daemon::is_hosted() {
        return Ok(());
    }

    use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
    let action = SigAction::new(
        SigHandler::Handler(on_sigusr1),
        // SA_RESTART so the signal does not surface as EINTR in the middle of
        // a read the event loop is doing; the flag is checked on the next tick
        // either way.
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    // SAFETY: the handler only stores to a static atomic.
    unsafe { sigaction(Signal::SIGUSR1, &action) }
        .context("Failed to install the SIGUSR1 handler")?;

    // Only now is it safe to let the signal through: the handler is in place,
    // and any pending attach is delivered to it immediately.
    let mut mask = SigSet::empty();
    mask.add(Signal::SIGUSR1);
    mask.thread_unblock().context("Failed to unblock SIGUSR1")?;
    Ok(())
}

/// Consume a pending reattach request, if one arrived.
pub fn take_request() -> bool {
    REATTACH_REQUESTED.swap(false, Ordering::SeqCst)
}

/// What the terminal on the other end of the current attach looks like.
pub struct ClientTerminal {
    pub term: String,
    /// The client's terminal answered the Kitty keyboard-protocol query.
    pub kitty: bool,
    /// The client is on the far side of an SSH connection.
    pub via_ssh: bool,
    /// The client's terminal widens a text-presentation emoji after U+FE0F.
    pub vs16_wide: bool,
}

/// Adopt the terminal facts reported by the client that just attached.
///
/// The hosted process inherited `$TERM` from whichever terminal started the
/// instance, and it cannot probe for keyboard capabilities at all: the far end
/// of its PTY is the daemon, which answers no capability query, so a probe
/// there always reports "unsupported" and every `Alt+<letter>` binding quietly
/// stops working on macOS. Both facts therefore come from the client.
///
/// Setting `$TERM` here is also what makes every shell spawned afterwards see
/// the right terminal.
pub fn adopt_client_terminal() -> Option<ClientTerminal> {
    let id = daemon::hosted_instance_id()?;
    let path = paths::term_path(&id).ok()?;
    let text = std::fs::read_to_string(path).ok()?;

    let mut lines = text.lines();
    let term = lines.next()?.trim().to_string();
    if term.is_empty() {
        return None;
    }

    let mut kitty = false;
    let mut via_ssh = false;
    let mut vs16_wide = false;
    for line in lines {
        match line.split_once('=') {
            Some(("kitty", v)) => kitty = v.trim() == "1",
            Some(("ssh", v)) => via_ssh = v.trim() == "1",
            Some(("vs16", v)) => vs16_wide = v.trim() == "1",
            _ => {}
        }
    }

    // SAFETY: called from the event loop, before any thread this process
    // spawns reads TERM for a child it is about to exec.
    unsafe { std::env::set_var("TERM", &term) };

    Some(ClientTerminal {
        term,
        kitty,
        via_ssh,
        vs16_wide,
    })
}
