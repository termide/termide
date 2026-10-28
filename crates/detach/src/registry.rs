//! The list of detached instances, kept as one `<id>.info` sidecar per socket.
//!
//! There is no central index file: an instance is whatever has a live socket in
//! the runtime directory. That keeps the registry self-healing — a daemon that
//! dies without cleaning up leaves a socket whose daemon no longer exists, and
//! [`prune_dead`] removes it on the next listing.
//!
//! A pid alone does not identify the daemon: on macOS the runtime directory
//! survives a reboot, and the recorded pid may since belong to an unrelated
//! process. [`is_running`] checks the process start time recorded with it.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;

/// What `--list-instances` shows for one detached instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceInfo {
    pub id: String,
    /// Pid of the daemon, not of the hosted termide: the daemon is what owns
    /// the socket, so its liveness is what decides whether the instance exists.
    pub pid: i32,
    /// When the daemon process started, as the kernel tells it (an opaque
    /// value, compared only for equality): with the pid, what identifies the
    /// daemon. `None` in a sidecar written by an older termide, or where the
    /// platform does not report it.
    pub pid_start: Option<u64>,
    pub project: PathBuf,
    /// Seconds since the Unix epoch.
    pub started: u64,
    pub attached: bool,
}

impl InstanceInfo {
    /// How long the instance has been up, as a compact `3d 4h` / `5m` string.
    pub fn uptime(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(self.started);
        let secs = now.saturating_sub(self.started);

        let days = secs / 86_400;
        let hours = (secs % 86_400) / 3_600;
        let minutes = (secs % 3_600) / 60;

        if days > 0 {
            format!("{days}d {hours}h")
        } else if hours > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{minutes}m")
        }
    }

    fn serialise(&self) -> String {
        let mut text = format!("pid={}\n", self.pid);
        if let Some(start) = self.pid_start {
            text.push_str(&format!("pid_start={start}\n"));
        }
        text.push_str(&format!(
            "project={}\nstarted={}\nattached={}\n",
            self.project.display(),
            self.started,
            u8::from(self.attached)
        ));
        text
    }

    fn parse(id: &str, text: &str) -> Option<Self> {
        let mut pid = None;
        let mut pid_start = None;
        let mut project = None;
        let mut started = None;
        let mut attached = false;

        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "pid" => pid = value.parse().ok(),
                "pid_start" => pid_start = value.parse().ok(),
                "project" => project = Some(PathBuf::from(value)),
                "started" => started = value.parse().ok(),
                "attached" => attached = value == "1",
                _ => {}
            }
        }

        Some(InstanceInfo {
            id: id.to_string(),
            pid: pid?,
            pid_start,
            project: project?,
            started: started.unwrap_or(0),
            attached,
        })
    }
}

/// Current wall-clock time as seconds since the epoch.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write (or overwrite) the sidecar for an instance.
pub fn write_info(info: &InstanceInfo) -> Result<()> {
    let path = paths::info_path(&info.id)?;
    std::fs::write(&path, info.serialise())
        .with_context(|| format!("Failed to write {}", path.display()))
}

/// Read one instance's sidecar, if it is present and parsable.
pub fn read_info(id: &str) -> Option<InstanceInfo> {
    let path = paths::info_path(id).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    InstanceInfo::parse(id, &text)
}

/// Mark an instance attached or detached, leaving the rest of the sidecar alone.
pub fn set_attached(id: &str, attached: bool) -> Result<()> {
    if let Some(mut info) = read_info(id) {
        info.attached = attached;
        write_info(&info)?;
    }
    Ok(())
}

/// Delete every file belonging to an instance.
///
/// All three of them: a leftover `.term` is small but permanent, and since
/// `prune_dead` routes through here, anything this function forgets accretes
/// in the runtime directory for as long as the machine lives.
pub fn remove(id: &str) {
    for path in [
        paths::socket_path(id),
        paths::info_path(id),
        paths::term_path(id),
    ]
    .into_iter()
    .flatten()
    {
        let _ = std::fs::remove_file(path);
    }
}

/// Whether a process of ours with this pid exists.
///
/// `kill(pid, 0)` asks the kernel without touching the process. EPERM means
/// the pid belongs to another user, which a daemon started by this user,
/// listed in this user's runtime directory, never does.
#[cfg(unix)]
fn pid_is_ours(pid: i32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    pid > 0 && kill(Pid::from_raw(pid), None).is_ok()
}

/// When process `pid` started, as an opaque value that differs between two
/// processes that held the same pid.
#[cfg(target_os = "linux")]
pub(crate) fn process_start(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name in parentheses may hold spaces; the fields after it
    // start with the state (field 3), so the start time (field 22) is the
    // 20th of them.
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(target_os = "macos")]
pub(crate) fn process_start(pid: i32) -> Option<u64> {
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `proc_pidinfo` writes at most `size` bytes into the zeroed
    // struct it is given, and reports how many it wrote.
    let info = unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let written = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast::<libc::c_void>(),
            size,
        );
        (written == size).then_some(info)
    }?;
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn process_start(_pid: i32) -> Option<u64> {
    None
}

/// Whether the daemon of `info` is still running.
///
/// The pid must be a live process of ours, and the same process: its start
/// time must match the recorded one. A sidecar without a start time (older
/// termide, or a platform that reports none) is taken as running only when
/// its socket accepts a connection, which a socket left over from a reboot
/// or a crash does not; a live daemon sees the probe as a client that says
/// nothing, and drops it.
#[cfg(unix)]
pub fn is_running(info: &InstanceInfo) -> bool {
    if !pid_is_ours(info.pid) {
        return false;
    }
    match (info.pid_start, process_start(info.pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        _ => paths::socket_path(&info.id)
            .is_ok_and(|socket| std::os::unix::net::UnixStream::connect(socket).is_ok()),
    }
}

#[cfg(not(unix))]
pub fn is_running(_info: &InstanceInfo) -> bool {
    false
}

/// Drop sockets whose daemon is gone. Returns the ids that were removed.
pub fn prune_dead() -> Result<Vec<String>> {
    let mut pruned = Vec::new();
    for info in scan()? {
        if !is_running(&info) {
            remove(&info.id);
            pruned.push(info.id);
        }
    }
    Ok(pruned)
}

/// Every instance with a socket in the runtime directory, dead ones included.
fn scan() -> Result<Vec<InstanceInfo>> {
    let dir = paths::runtime_dir()?;
    let mut found = Vec::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", dir.display())),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match read_info(id) {
            Some(info) => found.push(info),
            // A socket with no readable sidecar is unusable — nothing can be
            // reported about it and nothing can attach — so treat it as dead.
            None => remove(id),
        }
    }

    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

/// Live instances, with dead entries pruned as a side effect.
pub fn list() -> Result<Vec<InstanceInfo>> {
    prune_dead()?;
    scan()
}

/// The instance `--attach` should pick when the user names none.
///
/// The most recently started one: with a single instance it is unambiguous,
/// and with several it matches "the one I just detached from".
pub fn most_recent() -> Result<Option<InstanceInfo>> {
    Ok(list()?.into_iter().max_by_key(|s| s.started))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InstanceInfo {
        InstanceInfo {
            id: "termide".to_string(),
            pid: 4242,
            pid_start: Some(1_700_000_000_123_456),
            project: PathBuf::from("/home/u/termide"),
            started: 1_700_000_000,
            attached: true,
        }
    }

    // Regression: `remove` used to delete the socket and the sidecar but not
    // the `.term` handover file, so every instance that ever ran left one
    // behind — for ever, since pruning goes through this same function.
    #[test]
    fn remove_deletes_every_file_an_instance_owns() {
        let id = "test-remove-all-files";
        let paths = [
            paths::socket_path(id).unwrap(),
            paths::info_path(id).unwrap(),
            paths::term_path(id).unwrap(),
        ];
        for path in &paths {
            std::fs::write(path, b"x").unwrap();
        }

        remove(id);

        for path in &paths {
            assert!(!path.exists(), "{} should be gone", path.display());
        }
    }

    #[test]
    fn info_round_trips_through_the_sidecar_format() {
        let mut info = sample();
        let parsed = InstanceInfo::parse("termide", &info.serialise()).unwrap();
        assert_eq!(parsed, info);
        info.pid_start = None;
        let parsed = InstanceInfo::parse("termide", &info.serialise()).unwrap();
        assert_eq!(parsed, info);
    }

    fn own_process(id: &str) -> InstanceInfo {
        let pid = std::process::id() as i32;
        InstanceInfo {
            id: id.to_string(),
            pid,
            pid_start: process_start(pid),
            ..sample()
        }
    }

    // Regression: a sidecar that survived a reboot (macOS keeps the runtime
    // directory) names a pid that now belongs to some other process. That
    // process is not the daemon: never pruned, it could be killed by
    // `--kill`.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_reused_pid_is_not_the_daemon() {
        let mut info = own_process("test-reused-pid");
        assert!(info.pid_start.is_some());
        assert!(is_running(&info), "the recorded process itself");
        info.pid_start = info.pid_start.map(|start| start + 1);
        assert!(!is_running(&info), "another process under the same pid");
    }

    // A pid of another user (here init/launchd, unless the tests run as
    // root) cannot be a daemon from this user's runtime directory.
    #[cfg(unix)]
    #[test]
    fn a_process_of_another_user_is_not_the_daemon() {
        // SAFETY: `geteuid` only reads the process credentials.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let info = InstanceInfo {
            pid: 1,
            pid_start: None,
            ..own_process("test-other-user")
        };
        assert!(!is_running(&info));
    }

    // A sidecar from an older termide records no start time: the daemon is
    // the one listening on the socket, and a socket nobody listens on is a
    // leftover.
    #[cfg(unix)]
    #[test]
    fn without_a_start_time_the_socket_must_answer() {
        let id = "test-legacy-sidecar";
        let socket = paths::socket_path(id).unwrap();
        let _ = std::fs::remove_file(&socket);
        let info = InstanceInfo {
            pid_start: None,
            ..own_process(id)
        };
        assert!(!is_running(&info), "no socket at all");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(is_running(&info), "a daemon listening");
        drop(listener);
        assert!(!is_running(&info), "a socket left behind");
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn parsing_needs_a_pid_and_a_project() {
        assert!(InstanceInfo::parse("x", "pid=1\n").is_none());
        assert!(InstanceInfo::parse("x", "project=/tmp\n").is_none());
        assert!(InstanceInfo::parse("x", "pid=1\nproject=/tmp\n").is_some());
    }

    #[test]
    fn unknown_keys_are_ignored_so_the_format_can_grow() {
        let text = "pid=7\nproject=/tmp\nstarted=5\nattached=1\nfuture=yes\n";
        let parsed = InstanceInfo::parse("x", text).unwrap();
        assert_eq!(parsed.pid, 7);
        assert!(parsed.attached);
    }

    #[test]
    fn uptime_is_reported_in_the_largest_useful_unit() {
        let mut info = sample();
        info.started = now_unix().saturating_sub(90);
        assert_eq!(info.uptime(), "1m");

        info.started = now_unix().saturating_sub(3 * 3_600 + 300);
        assert_eq!(info.uptime(), "3h 5m");

        info.started = now_unix().saturating_sub(2 * 86_400 + 4 * 3_600);
        assert_eq!(info.uptime(), "2d 4h");
    }
}
