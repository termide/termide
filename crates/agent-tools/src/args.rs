//! Argument extraction shared by the tools. Every failure is a plain message
//! the model can act on.

use std::path::{Path, PathBuf};

use termide_agent_core::{ToolCall, ToolContext};

pub(crate) fn required_str<'a>(call: &'a ToolCall, key: &str) -> Result<&'a str, String> {
    match call.arguments.get(key) {
        Some(value) => value
            .as_str()
            .ok_or_else(|| format!("`{key}` must be a string")),
        None => Err(format!("missing required argument `{key}`")),
    }
}

pub(crate) fn optional_str<'a>(call: &'a ToolCall, key: &str) -> Result<Option<&'a str>, String> {
    match call.arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a string")),
    }
}

pub(crate) fn optional_u64(call: &ToolCall, key: &str) -> Result<Option<u64>, String> {
    match call.arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

pub(crate) fn optional_bool(call: &ToolCall, key: &str) -> Result<bool, String> {
    match call.arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("`{key}` must be a boolean")),
    }
}

/// Refuse a path that names neither a regular file nor a directory: a
/// terminal, a FIFO, a socket, a device. The tools run inside termide, so
/// reading `/dev/stdin` or `/dev/tty` blocks the tool's thread on termide's
/// own terminal for good, and that thread then takes keys and halves of
/// escape sequences from the interface; writing one sends the bytes to the
/// device. A missing path and a directory pass, for the caller to report.
pub(crate) fn ensure_not_special(path: &Path) -> Result<(), String> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(());
    };
    let kind = meta.file_type();
    if kind.is_file() || kind.is_dir() {
        return Ok(());
    }
    Err(format!(
        "{} is not a regular file but {}; the file tools only handle regular files",
        path.display(),
        special_kind(&kind)
    ))
}

#[cfg(unix)]
fn special_kind(kind: &std::fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    if kind.is_char_device() {
        "a character device (such as a terminal)"
    } else if kind.is_block_device() {
        "a block device"
    } else if kind.is_fifo() {
        "a named pipe"
    } else if kind.is_socket() {
        "a socket"
    } else {
        "a special file"
    }
}

#[cfg(not(unix))]
fn special_kind(_kind: &std::fs::FileType) -> &'static str {
    "a special file"
}

/// Resolve a tool path argument against the working directory.
pub(crate) fn resolve_path(ctx: &ToolContext, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("`path` must not be empty".to_string());
    }
    let path = Path::new(raw);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    })
}

#[cfg(all(test, unix))]
pub(crate) mod test_support {
    use std::path::Path;
    use std::time::Duration;

    /// Make a named pipe at `path`: with no writer, reading it blocks, the
    /// way reading a terminal does.
    pub(crate) fn make_fifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    }

    /// Run `f` on a thread of its own and fail, rather than hang, when it
    /// blocks.
    pub(crate) fn within_seconds<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the tool blocked on a special file")
    }
}
