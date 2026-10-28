//! The attach client: a thin byte pump between this terminal and an instance
//! daemon.
//!
//! It deliberately knows nothing about termide's rendering. Output arrives as
//! raw PTY bytes and goes straight to stdout; keystrokes go the other way
//! untouched, so every escape sequence, mouse report and paste that the hosted
//! termide negotiated keeps working without the client having to understand it.

use anyhow::{Context, Result};
use crossterm::terminal::enable_raw_mode;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::protocol::{ClientCaps, ClientFrame, ServerFrame};
use crate::registry;

/// How often the client re-reads its terminal size.
///
/// Polling rather than handling SIGWINCH keeps the client free of signal
/// handlers and unsafe code; a resize is rare and a tenth of a second of lag
/// on it is not perceptible.
const RESIZE_POLL: Duration = Duration::from_millis(100);

/// Emergency detach: `Ctrl+\` three times in a row.
///
/// The supported way to detach is the in-app action, which leaves termide's
/// own keybindings untouched. This escape hatch exists only for a hosted
/// process that has stopped responding, and is deliberately a sequence no
/// editing instance produces by accident.
const EMERGENCY_BYTE: u8 = 0x1c;
const EMERGENCY_REPEATS: usize = 3;

/// Attach to a detached instance, returning when the client detaches or the
/// instance ends.
///
/// Returns the exit code the caller should use: the hosted termide's own code
/// when the instance ended, and 0 when this client merely detached. Tools that
/// run termide and wait for it — `git commit`, `crontab -e` — decide what to
/// do from that code, so swallowing it would make a failed edit look
/// successful.
///
/// With `takeover`, a client that is already attached is let go rather than
/// this one being refused.
pub fn attach(id: Option<String>, takeover: bool) -> Result<i32> {
    let instance = match id {
        Some(id) => registry::read_info(&id)
            .with_context(|| format!("No detached instance named '{id}'"))?,
        None => registry::most_recent()?
            .context("No detached instances. Start one with `termide --detached`.")?,
    };

    let socket = crate::paths::socket_path(&instance.id)?;
    let stream = UnixStream::connect(&socket).with_context(|| {
        format!(
            "Instance '{}' is not reachable; its daemon may have died",
            instance.id
        )
    })?;

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string());
    let caps = probe_terminal();

    let mut writer = stream.try_clone()?;
    ClientFrame::Attach {
        cols,
        rows,
        term,
        caps,
        takeover,
    }
    .write_to(&mut writer)?;

    // Raw mode only after the daemon has accepted us: a `Busy` reply should
    // print as an ordinary error on an untouched terminal.
    let mut reader = stream.try_clone()?;
    match ServerFrame::read_from(&mut reader)? {
        Some(ServerFrame::Attached) => {}
        Some(ServerFrame::Busy) => {
            anyhow::bail!(
                "Instance '{id}' already has a client attached; \
                 take it over with `termide --attach {id} --force`",
                id = instance.id
            );
        }
        Some(ServerFrame::Exited(code)) => {
            anyhow::bail!("Instance '{}' exited with code {code}", instance.id);
        }
        // A host started by an older termide does not know the takeover frame
        // and closes the connection on it.
        None if takeover => anyhow::bail!(
            "Instance '{id}' did not accept the takeover; its host may predate \
             `--force`. Detach the other client, or end the instance with \
             `termide --kill {id}`",
            id = instance.id
        ),
        _ => anyhow::bail!("Instance '{}' did not accept the attach", instance.id),
    }

    enable_raw_mode().context("Failed to put the terminal into raw mode")?;
    // The hosted termide re-enters its terminal modes only on the tick after
    // it learns of the attach, and may paint a whole frame before that. Taken
    // here, before any output, the alternate screen catches that frame too,
    // so it is wiped on detach instead of staying on this shell's screen.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen);

    let running = Arc::new(AtomicBool::new(true));
    spawn_input_pump(&stream, Arc::clone(&running))?;
    spawn_resize_watcher(&stream, Arc::clone(&running), cols, rows)?;

    let outcome = output_loop(&mut reader);
    running.store(false, Ordering::Relaxed);

    restore_terminal(caps);

    match outcome {
        Outcome::Detached => {
            println!("Detached from instance '{}'.", instance.id);
            println!("Attach again with: termide --attach {}", instance.id);
            Ok(0)
        }
        Outcome::TakenOver => {
            println!(
                "Instance '{}' was taken over by another client.",
                instance.id
            );
            Ok(0)
        }
        Outcome::Exited(code) => {
            if code == 0 {
                println!("Instance '{}' ended.", instance.id);
            } else {
                println!("Instance '{}' ended with code {code}.", instance.id);
            }
            Ok(code)
        }
    }
}

/// End a detached instance, and every shell, LSP server and job inside it.
///
/// The daemon is asked first: it stops the hosted termide with SIGTERM, then
/// SIGKILL, and goes with it. A daemon that does not go — wedged itself, or
/// started by a termide that predates `--kill` — is killed from here, which
/// closes the PTY and hangs the hosted termide up.
pub fn kill(id: &str) -> Result<()> {
    let instance =
        registry::read_info(id).with_context(|| format!("No detached instance named '{id}'"))?;

    // Only the daemon itself is signalled: a stale sidecar's pid may belong
    // to an unrelated process by now.
    if registry::is_running(&instance) {
        if let Ok(mut stream) = UnixStream::connect(crate::paths::socket_path(id)?) {
            let _ = ClientFrame::Kill.write_to(&mut stream);
        }
        if !wait_for_exit(&instance, crate::daemon::KILL_GRACE + KILL_MARGIN) {
            use nix::sys::signal::{kill, Signal};
            let _ = kill(nix::unistd::Pid::from_raw(instance.pid), Signal::SIGKILL);
        }
    }

    // The daemon removes its own files on a clean exit; after a SIGKILL, or
    // for one that was already dead, nobody else will.
    registry::remove(id);
    println!("Instance '{id}' ended.");
    Ok(())
}

/// Extra time `kill` gives the daemon beyond its own SIGKILL escalation.
const KILL_MARGIN: Duration = Duration::from_secs(2);

/// Wait for an instance's daemon, not our child, to go, up to `timeout`;
/// `false` while it is still the same process.
fn wait_for_exit(instance: &registry::InstanceInfo, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while registry::is_running(instance) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Ask this terminal what it can do, on the hosted instance's behalf.
///
/// Must run before raw mode: the Kitty query is a request/response handshake
/// that needs cooked-mode readiness. Over SSH the probe is skipped entirely —
/// a terminal that never answers would hang the attach — which matches what a
/// local termide does in the same situation.
fn probe_terminal() -> ClientCaps {
    let via_ssh =
        std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
    // The width probe has a bounded wait of its own and is answered by any
    // terminal that answers cursor-position reports, SSH or not.
    let vs16_wide = termide_core::probe_variation_selector_width().unwrap_or(false);
    if via_ssh {
        return ClientCaps {
            kitty: false,
            via_ssh: true,
            vs16_wide,
        };
    }
    ClientCaps {
        kitty: crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false),
        via_ssh: false,
        vs16_wide,
    }
}

enum Outcome {
    Detached,
    TakenOver,
    Exited(i32),
}

/// Forward stdin to the daemon, watching for the emergency detach sequence.
fn spawn_input_pump(stream: &UnixStream, running: Arc<AtomicBool>) -> Result<()> {
    let mut writer = stream.try_clone()?;
    let shutdown = stream.try_clone()?;

    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        let mut streak = 0usize;

        while running.load(Ordering::Relaxed) {
            let n = match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };

            for &byte in &buf[..n] {
                streak = if byte == EMERGENCY_BYTE {
                    streak + 1
                } else {
                    0
                };
            }

            if streak >= EMERGENCY_REPEATS {
                let _ = ClientFrame::Detach.write_to(&mut writer);
                // Unblock the output loop, which is parked on the socket.
                let _ = shutdown.shutdown(std::net::Shutdown::Both);
                break;
            }

            if ClientFrame::Input(buf[..n].to_vec())
                .write_to(&mut writer)
                .is_err()
            {
                break;
            }
        }
    });
    Ok(())
}

/// Tell the daemon when this terminal changes size.
fn spawn_resize_watcher(
    stream: &UnixStream,
    running: Arc<AtomicBool>,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let mut writer = stream.try_clone()?;
    std::thread::spawn(move || {
        let mut last = (cols, rows);
        while running.load(Ordering::Relaxed) {
            std::thread::sleep(RESIZE_POLL);
            let Ok(size) = crossterm::terminal::size() else {
                continue;
            };
            if size != last {
                last = size;
                let frame = ClientFrame::Resize {
                    cols: size.0,
                    rows: size.1,
                };
                if frame.write_to(&mut writer).is_err() {
                    break;
                }
            }
        }
    });
    Ok(())
}

/// Copy daemon output to stdout until the instance ends or we detach.
fn output_loop(reader: &mut UnixStream) -> Outcome {
    let mut stdout = std::io::stdout();
    loop {
        match ServerFrame::read_from(reader) {
            Ok(Some(ServerFrame::Output(bytes))) => {
                if stdout.write_all(&bytes).is_err() || stdout.flush().is_err() {
                    return Outcome::Detached;
                }
            }
            Ok(Some(ServerFrame::Exited(code))) => return Outcome::Exited(code),
            Ok(Some(ServerFrame::TakenOver)) => return Outcome::TakenOver,
            // The daemon closing the socket is how an in-app detach reaches
            // the client: the instance lives on, this client does not.
            Ok(None) => return Outcome::Detached,
            Ok(Some(_)) => {}
            Err(_) => return Outcome::Detached,
        }
    }
}

/// Undo everything the hosted termide switched on in this terminal.
///
/// The client never enabled these itself — the hosted process did, through the
/// PTY — but it is the one holding the terminal when the connection ends, so
/// it has to put it back. Shared with startup/exit so the two can never drift.
///
/// The keyboard-enhancement stack is popped separately because only the client
/// knows whether anything was pushed: the hosted process pushes on reattach
/// only when this terminal answered the capability probe.
fn restore_terminal(caps: ClientCaps) {
    // Wipe the alternate screen before leaving it. Without this the last
    // frame the instance painted stays on screen in terminals that restore
    // the primary buffer lazily, so a detach looks like a frozen termide.
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::MoveTo(0, 0)
    );

    if caps.kitty {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
    }
    termide_core::leave_terminal_modes();
}
