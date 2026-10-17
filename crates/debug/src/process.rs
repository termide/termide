//! Starting the processes debugging needs, kept off termide's terminal.

use std::process::Command;

/// Run `command` in a session of its own, with no controlling terminal.
///
/// A process that keeps termide's controlling terminal can open `/dev/tty`
/// — a debugger or the program it runs does — and take termide's terminal
/// out of raw mode, after which every key shows as an escape code. A
/// program meant to read the keyboard gets a terminal panel instead, whose
/// PTY becomes its controlling terminal.
pub fn detach(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no Rust state.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Only meaningful when the test itself runs with a controlling
    /// terminal; without one both answers are `NO_TTY`.
    #[test]
    fn a_detached_process_cannot_open_the_terminal() {
        let out = detach(Command::new("sh").args([
            "-c",
            "if (exec 3</dev/tty) 2>/dev/null; then echo TTY; else echo NO_TTY; fi",
        ]))
        .output()
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "NO_TTY");
    }
}
