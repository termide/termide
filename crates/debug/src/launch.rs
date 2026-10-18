//! Command lines that start a command-line debugger with breakpoints set.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};

/// A breakpoint: a file and a 1-based line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Breakpoint {
    pub path: PathBuf,
    pub line: usize,
}

/// A command-line debugger termide can start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Debugger {
    Lldb,
    Gdb,
    /// Delve, for Go.
    Dlv,
    /// Python's built-in debugger.
    Pdb,
}

impl Debugger {
    /// Every debugger, in the order they are offered.
    pub const ALL: [Debugger; 4] = [Self::Lldb, Self::Gdb, Self::Dlv, Self::Pdb];

    /// The spelling `commands.toml` uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lldb => "lldb",
            Self::Gdb => "gdb",
            Self::Dlv => "dlv",
            Self::Pdb => "pdb",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|d| d.as_str().eq_ignore_ascii_case(name.trim()))
    }

    /// Whether a breakpoint in `path` is one this debugger can stop at: pdb
    /// takes Python files, Delve Go files, lldb and gdb the rest.
    pub fn accepts(self, path: &Path) -> bool {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let python = matches!(ext.as_deref(), Some("py" | "pyw"));
        let go = ext.as_deref() == Some("go");
        match self {
            Self::Pdb => python,
            Self::Dlv => go,
            Self::Lldb | Self::Gdb => !python && !go,
        }
    }

    fn script_extension(self) -> &'static str {
        match self {
            Self::Lldb => "lldb",
            Self::Gdb => "gdb",
            Self::Dlv => "dlv",
            Self::Pdb => "pdb",
        }
    }
}

/// What to debug and with which debugger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub debugger: Debugger,
    /// The program and its arguments, as shell text — for Delve, its
    /// subcommand and arguments (`debug ./cmd/app`), for pdb the script.
    pub target: String,
    /// A shell command run first; the debugger starts only if it succeeds.
    pub build: Option<String>,
    /// Flags of the debugger itself (the settings' and the launch's own
    /// `debugger_args`), put where it reads its options, before the program.
    pub debugger_args: Vec<String>,
    /// The debugger's program from the settings, in place of `lldb`, `gdb`,
    /// `dlv` or (for pdb) `python3` found on `PATH`.
    pub program: Option<String>,
}

impl Launch {
    /// The launch that needs no configuration for `file`: a Python script
    /// under pdb, the Go package in the file's directory under Delve.
    pub fn for_file(file: &Path) -> Option<Self> {
        let ext = file.extension()?.to_str()?.to_ascii_lowercase();
        let (debugger, target) = match ext.as_str() {
            "py" | "pyw" => (Debugger::Pdb, shell_quote(&file.to_string_lossy())),
            "go" => (Debugger::Dlv, "debug".to_string()),
            _ => return None,
        };
        Some(Self {
            debugger,
            target,
            build: None,
            debugger_args: Vec::new(),
            program: None,
        })
    }

    /// The shell line that starts the debugger on the target with
    /// `breakpoints` set and runs it to the first one. lldb, gdb and Delve
    /// read their commands from a script written into `script_dir`.
    pub fn command_line(&self, breakpoints: &[Breakpoint], script_dir: &Path) -> Result<String> {
        let breakpoints: Vec<&Breakpoint> = breakpoints
            .iter()
            .filter(|b| self.debugger.accepts(&b.path))
            .collect();
        let flags: String = self
            .debugger_args
            .iter()
            .map(|arg| format!(" {}", shell_quote(arg)))
            .collect();
        let program = |default: &str| match self.program.as_deref() {
            Some(program) => shell_quote(program),
            None => default.to_string(),
        };
        let debug = match self.debugger {
            Debugger::Pdb => {
                let mut line = format!("{} -m pdb{flags}", program("python3"));
                for b in &breakpoints {
                    let command = format!("break {}:{}", b.path.display(), b.line);
                    line.push_str(" -c ");
                    line.push_str(&shell_quote(&command));
                }
                line.push_str(" -c continue ");
                line.push_str(&self.target);
                line
            }
            debugger => {
                let script = write_script(debugger, &script(debugger, &breakpoints), script_dir)?;
                let script = shell_quote(&script.to_string_lossy());
                match debugger {
                    Debugger::Lldb => {
                        format!("{}{flags} -s {script} -- {}", program("lldb"), self.target)
                    }
                    Debugger::Gdb => format!(
                        "{} -q{flags} -x {script} --args {}",
                        program("gdb"),
                        self.target
                    ),
                    Debugger::Dlv => {
                        format!("{}{flags} --init {script} {}", program("dlv"), self.target)
                    }
                    Debugger::Pdb => unreachable!(),
                }
            }
        };
        Ok(match self.build.as_deref().map(str::trim) {
            Some(build) if !build.is_empty() => format!("{build} && {debug}"),
            _ => debug,
        })
    }
}

/// The debugger's own commands: set each breakpoint, then run.
fn script(debugger: Debugger, breakpoints: &[&Breakpoint]) -> String {
    let mut out = String::new();
    if debugger == Debugger::Gdb {
        // The program's shared libraries are not loaded yet.
        out.push_str("set breakpoint pending on\n");
    }
    for b in breakpoints {
        let path = b.path.to_string_lossy();
        let line = match debugger {
            Debugger::Lldb => {
                format!("breakpoint set --file {} --line {}", dquote(&path), b.line)
            }
            Debugger::Gdb if path.contains(' ') => format!("break {}:{}", dquote(&path), b.line),
            Debugger::Gdb | Debugger::Dlv => format!("break {}:{}", path, b.line),
            Debugger::Pdb => unreachable!("pdb takes its breakpoints as arguments"),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(match debugger {
        Debugger::Lldb | Debugger::Gdb => "run\n",
        _ => "continue\n",
    });
    out
}

fn write_script(debugger: Debugger, text: &str, dir: &Path) -> Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    std::fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let name = format!(
        "{}-{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        debugger.script_extension()
    );
    let path = dir.join(name);
    std::fs::write(&path, text).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(path)
}

fn dquote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `s` as one shell word: as it is when it holds only characters no shell
/// treats specially, in single quotes otherwise.
pub fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || "/._-+:=,@%".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bp(path: &str, line: usize) -> Breakpoint {
        Breakpoint {
            path: path.into(),
            line,
        }
    }

    fn launch(debugger: Debugger, target: &str) -> Launch {
        Launch {
            debugger,
            target: target.into(),
            build: None,
            debugger_args: Vec::new(),
            program: None,
        }
    }

    #[test]
    fn the_settings_program_replaces_the_debuggers_name() {
        let dir = tempfile::tempdir().unwrap();
        let line = Launch {
            program: Some("/opt/my gdb/bin/gdb".into()),
            ..launch(Debugger::Gdb, "./app")
        }
        .command_line(&[], dir.path())
        .unwrap();
        assert!(line.starts_with("'/opt/my gdb/bin/gdb' -q -x "), "{line}");
    }

    #[test]
    fn debugger_flags_go_before_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let with = |debugger, target: &str| Launch {
            debugger_args: vec!["--nx".into(), "a b".into()],
            ..launch(debugger, target)
        };
        let line = with(Debugger::Gdb, "./app -v")
            .command_line(&[], dir.path())
            .unwrap();
        assert!(line.starts_with("gdb -q --nx 'a b' -x "), "{line}");
        assert!(line.ends_with(" --args ./app -v"), "{line}");
        let line = with(Debugger::Lldb, "./app")
            .command_line(&[], dir.path())
            .unwrap();
        assert!(line.starts_with("lldb --nx 'a b' -s "), "{line}");
        let line = with(Debugger::Pdb, "t.py")
            .command_line(&[], dir.path())
            .unwrap();
        assert!(line.starts_with("python3 -m pdb --nx 'a b' -c "), "{line}");
        let line = with(Debugger::Dlv, "debug .")
            .command_line(&[], dir.path())
            .unwrap();
        assert!(line.starts_with("dlv --nx 'a b' --init "), "{line}");
    }

    fn script_of(line: &str) -> String {
        let path = line
            .split_whitespace()
            .skip_while(|w| !matches!(*w, "-s" | "-x" | "--init"))
            .nth(1)
            .unwrap();
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn lldb_sets_the_breakpoints_and_runs() {
        let dir = tempfile::tempdir().unwrap();
        let line = launch(Debugger::Lldb, "target/debug/app --flag")
            .command_line(&[bp("/p/src/main.rs", 10), bp("/p/tool.py", 3)], dir.path())
            .unwrap();
        assert!(line.starts_with("lldb -s "), "{line}");
        assert!(line.ends_with(" -- target/debug/app --flag"), "{line}");
        assert_eq!(
            script_of(&line),
            "breakpoint set --file \"/p/src/main.rs\" --line 10\nrun\n"
        );
    }

    #[test]
    fn gdb_quotes_a_path_with_a_space() {
        let dir = tempfile::tempdir().unwrap();
        let line = launch(Debugger::Gdb, "./a.out")
            .command_line(&[bp("/my dir/a.c", 4)], dir.path())
            .unwrap();
        assert!(line.contains("--args ./a.out"), "{line}");
        assert_eq!(
            script_of(&line),
            "set breakpoint pending on\nbreak \"/my dir/a.c\":4\nrun\n"
        );
    }

    #[test]
    fn delve_takes_only_go_files() {
        let dir = tempfile::tempdir().unwrap();
        let line = launch(Debugger::Dlv, "debug ./cmd/app")
            .command_line(&[bp("/p/main.go", 7), bp("/p/x.rs", 1)], dir.path())
            .unwrap();
        assert!(line.ends_with(" debug ./cmd/app"), "{line}");
        assert_eq!(script_of(&line), "break /p/main.go:7\ncontinue\n");
    }

    #[test]
    fn pdb_gets_its_breakpoints_as_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let line = launch(Debugger::Pdb, "tool.py arg")
            .command_line(&[bp("/p/my tool.py", 3), bp("/p/a.rs", 1)], dir.path())
            .unwrap();
        assert_eq!(
            line,
            "python3 -m pdb -c 'break /p/my tool.py:3' -c continue tool.py arg"
        );
    }

    #[test]
    fn a_build_runs_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = launch(Debugger::Pdb, "t.py");
        l.build = Some("make".into());
        let line = l.command_line(&[], dir.path()).unwrap();
        assert_eq!(line, "make && python3 -m pdb -c continue t.py");
    }

    #[test]
    fn a_file_alone_picks_pdb_or_delve() {
        let py = Launch::for_file(Path::new("/p/a b.py")).unwrap();
        assert_eq!(
            (py.debugger, py.target.as_str()),
            (Debugger::Pdb, "'/p/a b.py'")
        );
        let go = Launch::for_file(Path::new("/p/main.go")).unwrap();
        assert_eq!((go.debugger, go.target.as_str()), (Debugger::Dlv, "debug"));
        assert!(Launch::for_file(Path::new("/p/main.rs")).is_none());
    }

    #[test]
    fn debugger_names_parse_case_insensitively() {
        assert_eq!(Debugger::parse(" LLDB "), Some(Debugger::Lldb));
        assert_eq!(Debugger::parse("delve"), None);
    }

    #[test]
    fn shell_quote_leaves_plain_words_alone() {
        assert_eq!(shell_quote("/p/a.py"), "/p/a.py");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
    }
}
