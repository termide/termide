//! macOS process introspection for the terminal panel.
//!
//! Linux answers the panel's three questions about the shell through `/proc`:
//! `cwd` for the working directory, `task/<pid>/children` for the foreground
//! child, `comm` for a process name. macOS has no `/proc`, so the same
//! questions go through libproc's `proc_pidinfo` and `proc_listpids`.
//!
//! Cost matters here: `has_children` is on the key-input path via
//! `captures_escape`, and the other two run on the render path behind a TTL
//! cache. `PROC_PPID_ONLY` keeps the child lookup to a single syscall instead
//! of a scan over every PID on the system.

use std::path::PathBuf;

/// `PROC_PPID_ONLY` from `<sys/proc_info.h>`: restrict `proc_listpids` to the
/// direct children of the pid passed as `typeinfo`. Not re-exported by `libc`;
/// the value is part of the kernel's stable ABI.
const PROC_PPID_ONLY: u32 = 6;

/// How many child pids to ask for when the caller only needs a yes/no answer.
const HAS_CHILDREN_PROBE: usize = 4;

/// The shell's current working directory, read from its current-directory
/// vnode. `None` when the process is gone or refuses introspection.
pub fn shell_cwd(pid: u32) -> Option<PathBuf> {
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;

    // SAFETY: `proc_pidinfo` writes at most `size` bytes into the zeroed
    // struct and returns how many it wrote. The struct is only read after
    // confirming a full-size write, so no field is left uninitialized.
    let info = unsafe {
        let mut info: libc::proc_vnodepathinfo = std::mem::zeroed();
        let written = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast::<libc::c_void>(),
            size,
        );
        if written < size {
            return None;
        }
        info
    };

    vip_path_to_pathbuf(&info.pvi_cdir.vip_path)
}

/// Name of the shell's foreground command: its first child's name, or the
/// shell's own name when nothing is running under it.
///
/// Mirrors the Linux path, which reads the first entry of the kernel's
/// `children` list and falls back to the shell's own `comm`.
pub fn foreground_command(pid: u32) -> Option<String> {
    let target = children(pid, 1).first().copied().unwrap_or(pid);
    let info = bsdinfo(target)?;

    // `pbi_name` holds up to 32 characters against `pbi_comm`'s 16, so a
    // longer command stays readable in the panel title. The kernel leaves it
    // empty for some processes, hence the fallback.
    c_chars_to_string(&info.pbi_name).or_else(|| c_chars_to_string(&info.pbi_comm))
}

/// Whether the shell has any direct child process.
pub fn has_children(pid: u32) -> bool {
    !children(pid, HAS_CHILDREN_PROBE).is_empty()
}

/// Direct children of `pid`, capped at `limit` entries.
///
/// The cap is what keeps this cheap: callers never need the whole list, only
/// the first child or the fact that one exists.
fn children(pid: u32, limit: usize) -> Vec<u32> {
    let mut pids: Vec<libc::c_int> = vec![0; limit];
    let buffer_size = std::mem::size_of_val(pids.as_slice()) as libc::c_int;

    // SAFETY: the buffer is a live allocation of exactly `buffer_size` bytes
    // and `proc_listpids` writes no more than that, returning the byte count.
    let written = unsafe {
        libc::proc_listpids(
            PROC_PPID_ONLY,
            pid,
            pids.as_mut_ptr().cast::<libc::c_void>(),
            buffer_size,
        )
    };
    if written <= 0 {
        return Vec::new();
    }

    let count = (written as usize / std::mem::size_of::<libc::c_int>()).min(limit);
    pids.truncate(count);
    // The kernel pads unused slots with zeroes when fewer children exist than
    // the buffer holds.
    pids.into_iter()
        .filter(|&child| child > 0)
        .map(|child| child as u32)
        .collect()
}

/// BSD-level info (parent pid, names) for a single process.
fn bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;

    // SAFETY: same contract as `shell_cwd` — a zeroed struct, a byte count
    // bounded by `size`, and reads only after a full-size write.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let written = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast::<libc::c_void>(),
            size,
        );
        if written < size {
            None
        } else {
            Some(info)
        }
    }
}

/// Decode libproc's split path buffer.
///
/// `vip_path` is really `[c_char; MAXPATHLEN]`; the `libc` crate declares it
/// as `[[c_char; 32]; 32]` to stay within an older rustc's array limits. The
/// bytes are contiguous either way.
fn vip_path_to_pathbuf(path: &[[libc::c_char; 32]; 32]) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    // SAFETY: `path` is one contiguous 32 * 32 array of `c_char`, reinterpreted
    // as the byte slice it already is. The borrow keeps it alive.
    let bytes = unsafe { std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), 32 * 32) };

    let end = bytes.iter().position(|&b| b == 0)?;
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(bytes[..end].to_vec())))
}

/// Decode a fixed-size, NUL-padded C string field.
fn c_chars_to_string(field: &[libc::c_char]) -> Option<String> {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test process is its own best fixture: its cwd is known, and it is
    /// guaranteed to exist for the duration of the test.
    #[test]
    fn reads_own_cwd() {
        let pid = std::process::id();
        let expected = std::env::current_dir().expect("cwd");
        let actual = shell_cwd(pid).expect("shell_cwd for the current process");

        // `/tmp` and friends are symlinked on macOS, so compare canonical form.
        assert_eq!(
            actual.canonicalize().ok(),
            expected.canonicalize().ok(),
            "shell_cwd disagreed with std::env::current_dir"
        );
    }

    #[test]
    fn reports_own_name() {
        let pid = std::process::id();
        let name = foreground_command(pid).expect("foreground_command for the current process");
        assert!(!name.is_empty(), "process name should not be empty");
    }

    #[test]
    fn detects_a_live_child() {
        let pid = std::process::id();

        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn /bin/sleep");

        assert!(
            has_children(pid),
            "a spawned child should be visible to has_children"
        );
        assert!(
            children(pid, 32).contains(&child.id()),
            "the spawned pid should appear among the direct children"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn unknown_pid_yields_nothing() {
        // Above the kernel's pid ceiling, so it can never name a live process.
        // This is the same "cannot introspect" path a dead shell takes.
        const DEAD_PID: u32 = 999_999;

        assert!(shell_cwd(DEAD_PID).is_none());
        assert!(foreground_command(DEAD_PID).is_none());
        assert!(!has_children(DEAD_PID));
    }
}
