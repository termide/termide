//! The debug adapters termide drives over DAP, and how each is started and
//! told what to debug.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

/// A debug adapter that speaks DAP over its standard input and output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adapter {
    /// LLVM's `lldb-dap` (formerly `lldb-vscode`): Rust, C, C++, Swift.
    LldbDap,
    /// GDB 14 and later, `gdb -i dap`.
    GdbDap,
    /// Microsoft's debugpy, for Python.
    Debugpy,
    /// Delve, for Go: `dlv dap`, which talks over TCP.
    Delve,
}

impl Adapter {
    pub const ALL: [Adapter; 4] = [Self::LldbDap, Self::GdbDap, Self::Debugpy, Self::Delve];

    /// The spelling `commands.toml` uses for `debugger`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LldbDap => "lldb-dap",
            Self::GdbDap => "gdb-dap",
            Self::Debugpy => "debugpy",
            Self::Delve => "dlv-dap",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|a| a.as_str().eq_ignore_ascii_case(name.trim()))
    }

    /// Whether a breakpoint in `path` is one this adapter can stop at.
    pub fn accepts(self, path: &Path) -> bool {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let python = matches!(ext.as_deref(), Some("py" | "pyw"));
        let go = ext.as_deref() == Some("go");
        match self {
            Self::Debugpy => python,
            Self::Delve => go,
            Self::LldbDap | Self::GdbDap => !python && !go,
        }
    }

    /// The command that starts the adapter, when it can be found.
    pub fn command(self) -> Option<(PathBuf, Vec<String>)> {
        match self {
            Self::LldbDap => find_in_path("lldb-dap")
                .or_else(xcrun_lldb_dap)
                .or_else(|| find_in_path("lldb-vscode"))
                .map(|p| (p, Vec::new())),
            Self::GdbDap => find_in_path("gdb").map(|p| (p, vec!["-i".into(), "dap".into()])),
            Self::Debugpy => find_in_path("python3")
                .or_else(|| find_in_path("python"))
                .map(|p| (p, vec!["-m".into(), "debugpy.adapter".into()])),
            Self::Delve => {
                find_in_path("dlv").map(|p| (p, vec!["dap".into(), "--listen=127.0.0.1:0".into()]))
            }
        }
    }

    /// Whether the adapter listens on a socket rather than talking over its
    /// standard input and output.
    pub fn uses_tcp(self) -> bool {
        self == Self::Delve
    }

    /// The `adapterID` of `initialize`.
    pub fn id(self) -> &'static str {
        match self {
            Self::LldbDap => "lldb-dap",
            Self::GdbDap => "gdb",
            Self::Debugpy => "debugpy",
            Self::Delve => "go",
        }
    }

    /// The arguments of `launch` for `program` run with `args` in `cwd`.
    pub fn launch_arguments(self, program: &str, args: &[String], cwd: &Path) -> Value {
        let cwd = cwd.to_string_lossy();
        match self {
            Self::LldbDap | Self::GdbDap => json!({
                "program": program,
                "args": args,
                "cwd": cwd,
                "stopOnEntry": false,
            }),
            Self::Debugpy => json!({
                "program": program,
                "args": args,
                "cwd": cwd,
                "console": "internalConsole",
                "justMyCode": true,
                "redirectOutput": true,
            }),
            // `program` is the package to build, `.` or `./cmd/app`.
            Self::Delve => json!({
                "mode": "debug",
                "program": program,
                "args": args,
                "cwd": cwd,
                "outputMode": "remote",
            }),
        }
    }
}

/// Whether debugpy can be imported by the Python on `PATH`.
pub fn debugpy_available() -> bool {
    let Some((python, _)) = Adapter::Debugpy.command() else {
        return false;
    };
    Command::new(python)
        .args(["-c", "import debugpy"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn xcrun_lldb_dap() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("xcrun")
        .args(["--find", "lldb-dap"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    (out.status.success() && path.is_file()).then_some(path)
}

/// `name` in a directory of `PATH`.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_names_parse() {
        assert_eq!(Adapter::parse("LLDB-DAP"), Some(Adapter::LldbDap));
        assert_eq!(Adapter::parse("debugpy"), Some(Adapter::Debugpy));
        assert_eq!(Adapter::parse("lldb"), None);
    }

    #[test]
    fn debugpy_launches_into_its_internal_console() {
        let args = Adapter::Debugpy.launch_arguments("t.py", &["x".into()], Path::new("/p"));
        assert_eq!(args["program"], "t.py");
        assert_eq!(args["args"], json!(["x"]));
        assert_eq!(args["console"], "internalConsole");
    }
}
