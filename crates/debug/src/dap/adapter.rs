//! The debug adapters termide drives over DAP, and how each is started and
//! told what to debug.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use termide_config::DebuggerSettings;

/// Where the program's input and output go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Console {
    /// A terminal panel: the program reads the keyboard there.
    #[default]
    Terminal,
    /// The debug panel's output; the program gets no input.
    Panel,
}

impl Console {
    /// `terminal` or `panel`, as `commands.toml` spells it.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "terminal" => Some(Self::Terminal),
            "panel" => Some(Self::Panel),
            _ => None,
        }
    }
}

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

    /// The command that starts the adapter with its default flags, when it
    /// can be found.
    pub fn command(self) -> Option<(PathBuf, Vec<String>)> {
        self.command_with(&DebuggerSettings::default())
    }

    /// The command that starts the adapter as `settings` say: their program
    /// in place of the one found on `PATH`, their flags in place of the
    /// default ones. The arguments termide needs to talk to it come first
    /// and cannot be changed.
    pub fn command_with(self, settings: &DebuggerSettings) -> Option<(PathBuf, Vec<String>)> {
        let program = match settings.command.as_deref() {
            Some(command) => resolve_program(command)?,
            None => self.find()?,
        };
        let mut args = self.fixed_args();
        args.extend(settings.args.clone().unwrap_or_else(|| self.default_args()));
        Some((program, args))
    }

    /// The program found where it is usually installed.
    fn find(self) -> Option<PathBuf> {
        match self {
            Self::LldbDap => find_in_path("lldb-dap")
                .or_else(xcrun_lldb_dap)
                .or_else(|| find_in_path("lldb-vscode")),
            Self::GdbDap => find_in_path("gdb"),
            Self::Debugpy => find_in_path("python3").or_else(|| find_in_path("python")),
            Self::Delve => find_in_path("dlv"),
        }
    }

    /// The arguments termide starts the adapter with to talk DAP to it.
    fn fixed_args(self) -> Vec<String> {
        let args: &[&str] = match self {
            Self::LldbDap => &[],
            Self::GdbDap => &["-i", "dap"],
            Self::Debugpy => &["-m", "debugpy.adapter"],
            Self::Delve => &["dap", "--listen=127.0.0.1:0"],
        };
        args.iter().map(|a| a.to_string()).collect()
    }

    /// The flags the adapter gets unless the settings name others. Delve,
    /// where it cannot tell which user connects, gets its same-user check
    /// turned off, as it would refuse termide too.
    pub fn default_args(self) -> Vec<String> {
        match self {
            Self::Delve if !delve_can_check_user() => vec!["--only-same-user=false".to_string()],
            _ => Vec::new(),
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

    /// Whether the program can run in a terminal panel, where it reads the
    /// keyboard: debugpy and Delve ask termide to start it there
    /// (`runInTerminal`), lldb-dap gets the terminal's device to use. GDB's
    /// adapter has no such mode.
    pub fn supports_terminal(self) -> bool {
        !matches!(self, Self::GdbDap)
    }

    /// Whether the adapter asks termide to start the program in a terminal
    /// (`runInTerminal`) rather than being handed a terminal's device.
    pub fn asks_for_terminal(self) -> bool {
        matches!(self, Self::Debugpy | Self::Delve)
    }

    /// The arguments of `launch` for `program` run with `args` in `cwd`.
    /// With `console` [`Console::Terminal`] the program's input and output
    /// go to a terminal: one the adapter asks for, or the device `tty`.
    pub fn launch_arguments(
        self,
        program: &str,
        args: &[String],
        cwd: &Path,
        console: Console,
        tty: Option<&Path>,
    ) -> Value {
        let cwd = cwd.to_string_lossy();
        let terminal = console == Console::Terminal && self.supports_terminal();
        match self {
            Self::LldbDap | Self::GdbDap => {
                let mut arguments = json!({
                    "program": program,
                    "args": args,
                    "cwd": cwd,
                    "stopOnEntry": false,
                });
                // lldb-dap's own `runInTerminal` launcher fails to reach it
                // on macOS (LLVM 21), and its `stdio` option is newer still;
                // lldb's target settings point the program at the terminal
                // in every version.
                if let (true, Self::LldbDap, Some(tty)) = (terminal, self, tty) {
                    let tty = tty.to_string_lossy();
                    arguments["preRunCommands"] = json!([
                        format!("settings set target.input-path {tty}"),
                        format!("settings set target.output-path {tty}"),
                        format!("settings set target.error-path {tty}"),
                    ]);
                }
                arguments
            }
            Self::Debugpy => json!({
                "program": program,
                "args": args,
                "cwd": cwd,
                "console": if terminal { "integratedTerminal" } else { "internalConsole" },
                "justMyCode": true,
                "redirectOutput": !terminal,
            }),
            // `program` is the package to build, `.` or `./cmd/app`.
            Self::Delve => {
                let mut arguments = json!({
                    "mode": "debug",
                    "program": program,
                    "args": args,
                    "cwd": cwd,
                    "outputMode": "remote",
                });
                if terminal {
                    arguments["console"] = json!("integratedTerminal");
                }
                arguments
            }
        }
    }
}

/// Whether debugpy can be imported by the Python on `PATH`.
pub fn debugpy_available() -> bool {
    debugpy_available_with(&DebuggerSettings::default())
}

/// Whether debugpy can be imported by the Python `settings` name, or the
/// one on `PATH`.
pub fn debugpy_available_with(settings: &DebuggerSettings) -> bool {
    let Some((python, _)) = Adapter::Debugpy.command_with(settings) else {
        return false;
    };
    crate::detach(&mut Command::new(python))
        .args(["-c", "import debugpy"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Whether Delve can check that a local connection comes from the user who
/// started it. On Linux it reads `/proc/net/tcp` for that, which Android 10
/// and later deny to apps (Termux among them, whatever the binary was built
/// for); the check then fails and Delve closes every connection with
/// "closing connection from different user" (go-delve/delve#3202). Asked
/// once; the answer does not change while termide runs.
fn delve_can_check_user() -> bool {
    static CAN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CAN.get_or_init(|| {
        if !cfg!(any(target_os = "linux", target_os = "android")) {
            return true;
        }
        match std::fs::File::open("/proc/net/tcp") {
            Ok(_) => true,
            Err(e) => {
                log::info!(
                    "Delve: /proc/net/tcp cannot be read ({e}), so its same-user check \
                     would refuse termide; starting it with --only-same-user=false"
                );
                false
            }
        }
    })
}

fn xcrun_lldb_dap() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = crate::detach(&mut Command::new("xcrun"))
        .args(["--find", "lldb-dap"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    (out.status.success() && path.is_file()).then_some(path)
}

/// The program a setting names: a path (absolute, `~/…` under the home
/// directory, or with a `/`, taken from the current directory) that exists,
/// or a name looked up on `PATH`.
pub fn resolve_program(command: &str) -> Option<PathBuf> {
    resolve_program_in(command, std::env::var_os("HOME").map(PathBuf::from))
}

fn resolve_program_in(command: &str, home: Option<PathBuf>) -> Option<PathBuf> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let path = match command.strip_prefix("~/") {
        Some(rest) => home?.join(rest),
        None => PathBuf::from(command),
    };
    if path.components().count() > 1 {
        return path.is_file().then_some(path);
    }
    find_in_path(command)
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
    fn a_program_under_the_home_directory_is_found() {
        let home = tempfile::tempdir().unwrap();
        let program = home.path().join("go/bin/dlv");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "").unwrap();
        let home = Some(home.path().to_path_buf());
        assert_eq!(
            resolve_program_in("~/go/bin/dlv", home.clone()),
            Some(program)
        );
        assert_eq!(resolve_program_in("~/go/bin/gdb", home), None);
        assert_eq!(resolve_program_in("~/go/bin/dlv", None), None);
    }

    #[test]
    fn settings_replace_the_default_flags_but_not_termides_own() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("dlv");
        std::fs::write(&program, "").unwrap();
        let program_text = program.to_string_lossy().into_owned();
        let with = |args: Option<Vec<String>>| DebuggerSettings {
            command: Some(program_text.clone()),
            args,
        };
        let (found, args) = Adapter::Delve
            .command_with(&with(Some(vec!["--log".into()])))
            .expect("the named program exists");
        assert_eq!(found, program);
        assert_eq!(args, ["dap", "--listen=127.0.0.1:0", "--log"]);
        let (_, args) = Adapter::Delve
            .command_with(&with(Some(Vec::new())))
            .unwrap();
        assert_eq!(args, ["dap", "--listen=127.0.0.1:0"]);
        // Unset keeps the default flags, whatever this system needs.
        let (_, args) = Adapter::Delve.command_with(&with(None)).unwrap();
        assert_eq!(args[2..], Adapter::Delve.default_args()[..]);
        // A named program that is not there is not replaced by another.
        let missing = DebuggerSettings {
            command: Some(dir.path().join("nope").to_string_lossy().into_owned()),
            args: None,
        };
        assert_eq!(Adapter::Delve.command_with(&missing), None);
    }

    #[test]
    fn debugpy_launches_into_its_internal_console() {
        let args = Adapter::Debugpy.launch_arguments(
            "t.py",
            &["x".into()],
            Path::new("/p"),
            Console::Panel,
            None,
        );
        assert_eq!(args["program"], "t.py");
        assert_eq!(args["args"], json!(["x"]));
        assert_eq!(args["console"], "internalConsole");
        let args = Adapter::Debugpy.launch_arguments(
            "t.py",
            &[],
            Path::new("/p"),
            Console::Terminal,
            None,
        );
        assert_eq!(args["console"], "integratedTerminal");
    }

    #[test]
    fn lldb_points_the_program_at_the_terminals_device() {
        let args = Adapter::LldbDap.launch_arguments(
            "./m",
            &[],
            Path::new("/p"),
            Console::Terminal,
            Some(Path::new("/dev/ttys004")),
        );
        assert_eq!(
            args["preRunCommands"],
            json!([
                "settings set target.input-path /dev/ttys004",
                "settings set target.output-path /dev/ttys004",
                "settings set target.error-path /dev/ttys004"
            ])
        );
        let args = Adapter::GdbDap.launch_arguments(
            "./m",
            &[],
            Path::new("/p"),
            Console::Terminal,
            Some(Path::new("/dev/ttys004")),
        );
        assert!(
            args.get("preRunCommands").is_none(),
            "gdb has no terminal mode"
        );
    }
}
