//! Ending a server's or an agent's process along with what it started.
//!
//! A stdio MCP server or an ACP agent is often a launcher (`npx`, `uvx`, a
//! shell script) whose real work runs in a child of it: killing the process
//! termide started leaves that child behind. So the process is started as
//! the leader of a group of its own (see [`own_group`]), and [`retire`] ends
//! the group.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// How long a process whose stdin was closed has to exit on its own before
/// its group is killed.
pub const GRACE: Duration = Duration::from_secs(1);

/// Start `command`'s process as the leader of a new process group, so
/// [`retire`] can reach what it starts. A command already put in a session of
/// its own (`setsid`) leads a group anyway and needs no call.
pub fn own_group(command: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Close `child`'s stdin, give it `grace` to exit, then kill its process
/// group and reap it. The waiting happens on a thread of its own: this runs
/// when a panel closes, usually on the UI thread, which must not wait for a
/// server. The child must lead its group ([`own_group`], or `setsid`).
pub fn retire(mut child: Child, grace: Duration) {
    drop(child.stdin.take());
    let reaper = move || {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                _ => break,
            }
        }
        // Also when the leader has exited: what it started may still run.
        kill_group(&child);
        let _ = child.kill();
        let _ = child.wait();
    };
    if let Err(error) = std::thread::Builder::new()
        .name("termide-reaper".into())
        .spawn(reaper)
    {
        log::warn!("cannot start a thread to end a process: {error}");
    }
}

/// SIGKILL the process group `child` leads.
fn kill_group(child: &Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: plain syscall on the group the child leads.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = child;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;

    #[test]
    fn retiring_ends_what_the_process_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!(
                "(sleep 2; touch '{}') & cat >/dev/null; sleep 30",
                marker.display()
            ))
            .stdin(Stdio::piped());
        own_group(&mut command);
        let child = command.spawn().unwrap();
        let started = Instant::now();
        retire(child, Duration::from_millis(200));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "retire waited"
        );
        std::thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists(), "the background job outlived its server");
    }

    #[test]
    fn a_process_that_exits_on_eof_is_left_to_exit() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("cat >/dev/null")
            .stdin(Stdio::piped());
        own_group(&mut command);
        let child = command.spawn().unwrap();
        let pid = child.id();
        retire(child, Duration::from_secs(5));
        let deadline = Instant::now() + Duration::from_secs(3);
        // SAFETY: signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            assert!(Instant::now() < deadline, "the process did not exit on EOF");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
