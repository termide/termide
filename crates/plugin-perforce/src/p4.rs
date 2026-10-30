//! Running `p4` and reading what it prints.
//!
//! `p4` takes its server, user and workspace from the environment and the
//! P4CONFIG files as it does in a shell: it runs in the directory of the
//! file, or in the project root, with `PWD` set to it, since p4 takes `PWD`
//! over the real directory.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long one `p4` may run.
const TIMEOUT: Duration = Duration::from_secs(60);

/// p4 said no: the message as p4 wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P4Error(pub String);

impl std::fmt::Display for P4Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One `p4 -ztag` record: `... key value` lines.
pub type Record = BTreeMap<String, String>;

/// The workspace the project is in, as `p4 info` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Workspace {
    /// The root with symbolic links resolved, as termide gives paths.
    real: PathBuf,
    /// The root as p4 has it, which may go through a symbolic link.
    spelled: PathBuf,
}

/// The `p4` program and the workspace it serves.
pub struct P4 {
    program: PathBuf,
    project_root: PathBuf,
    /// `None` until asked; `Some(None)` when the project is in no workspace.
    workspace: Mutex<Option<Option<Workspace>>>,
}

impl P4 {
    #[must_use]
    pub fn new(program: PathBuf, project_root: PathBuf) -> Self {
        Self {
            program,
            project_root,
            workspace: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Run p4 in `cwd`, the project root without one: its standard output,
    /// or what it wrote to standard error. Absolute paths among `args` are
    /// written as p4 spells them (see [`Self::as_p4_spells`]).
    ///
    /// # Errors
    ///
    /// When p4 cannot run, runs out of time, or fails.
    pub fn run(&self, args: &[&str], cwd: Option<&Path>) -> Result<String, P4Error> {
        let cwd = cwd.unwrap_or(&self.project_root);
        let args: Vec<String> = args
            .iter()
            .map(|arg| {
                if Path::new(arg).is_absolute() {
                    self.as_p4_spells(Path::new(arg))
                        .to_string_lossy()
                        .into_owned()
                } else {
                    (*arg).to_string()
                }
            })
            .collect();
        let mut child = Command::new(&self.program)
            .args(&args)
            .current_dir(cwd)
            .env("PWD", cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    P4Error(format!(
                        "{} is not installed or not on PATH",
                        self.program.display()
                    ))
                } else {
                    P4Error(format!("cannot run {}: {e}", self.program.display()))
                }
            })?;
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(P4Error(format!(
                        "p4 {} gave no answer in {} s",
                        args.first().map_or("", String::as_str),
                        TIMEOUT.as_secs()
                    )));
                }
                Err(e) => return Err(P4Error(format!("p4: {e}"))),
            }
        };
        let out = stdout.join().unwrap_or_default();
        let err = stderr.join().unwrap_or_default();
        let err = err.trim();
        if !err.is_empty() && (!status.success() || out.trim().is_empty()) {
            return Err(P4Error(err.to_string()));
        }
        if !status.success() {
            return Err(P4Error(format!("p4 {} failed ({status})", args.join(" "))));
        }
        Ok(out)
    }

    /// The file's record from `p4 fstat`, or `None` when the depot does not
    /// have it (not added yet, or outside the workspace).
    ///
    /// # Errors
    ///
    /// When p4 fails otherwise.
    pub fn fstat(&self, path: &Path) -> Result<Option<Record>, P4Error> {
        let arg = path.to_string_lossy();
        match self.run(&["-ztag", "fstat", &arg], path.parent()) {
            Ok(out) => Ok(parse_ztag(&out).into_iter().next()),
            Err(error) if not_in_depot(&error.0) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The workspace root, with symbolic links resolved, or `None` when the
    /// project is in no workspace. Asked again after p4 failed, so a server
    /// that was down at the start is found later.
    pub fn client_root(&self) -> Option<PathBuf> {
        if let Some(known) = self.known_workspace() {
            return known.map(|ws| ws.real);
        }
        // Not under the lock: `run` takes it to spell paths.
        let info = match self.run(&["-ztag", "info"], None) {
            Ok(out) => parse_ztag(&out),
            Err(error) => {
                eprintln!("perforce: info: {error}");
                return None;
            }
        };
        let record = info.into_iter().next().unwrap_or_default();
        let unknown = record
            .get("clientName")
            .is_none_or(|name| name == "*unknown*");
        let workspace = match record.get("clientRoot") {
            Some(root) if !unknown && !root.is_empty() => Some(Workspace {
                real: real_path(Path::new(root)),
                spelled: PathBuf::from(root),
            }),
            _ => None,
        };
        *self.workspace_slot() = Some(workspace.clone());
        workspace.map(|ws| ws.real)
    }

    fn workspace_slot(&self) -> std::sync::MutexGuard<'_, Option<Option<Workspace>>> {
        self.workspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn known_workspace(&self) -> Option<Option<Workspace>> {
        self.workspace_slot().clone()
    }

    /// Whether `path` lies in the workspace.
    pub fn in_workspace(&self, path: &Path) -> bool {
        self.client_root()
            .is_some_and(|root| real_path(path).starts_with(root))
    }

    /// `path` written under the workspace root as p4 has it. termide gives
    /// real paths (`/private/tmp/ws/a.c` on macOS), and p4 refuses one whose
    /// root it knows through a symbolic link (`/tmp/ws`) as not in the
    /// workspace.
    fn as_p4_spells(&self, path: &Path) -> PathBuf {
        let Some(Some(ws)) = self.known_workspace() else {
            return path.to_path_buf();
        };
        match real_path(path).strip_prefix(&ws.real) {
            Ok(rest) if rest.as_os_str().is_empty() => ws.spelled.clone(),
            Ok(rest) => ws.spelled.join(rest),
            Err(_) => path.to_path_buf(),
        }
    }

    /// `path`, however p4 spells it, written under the project root as
    /// termide has it; `None` outside the project.
    #[must_use]
    pub fn in_project(&self, path: &Path) -> Option<PathBuf> {
        let root = real_path(&self.project_root);
        let rest = real_path(path).strip_prefix(&root).ok()?.to_path_buf();
        Some(self.project_root.join(rest))
    }

    /// `path` from the project root, however either is spelled.
    #[must_use]
    pub fn relative(&self, path: &Path) -> String {
        let root = real_path(&self.project_root);
        let real = real_path(path);
        match real.strip_prefix(&root) {
            Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
            _ => path.to_string_lossy().into_owned(),
        }
    }
}

/// Whether p4's complaint means the depot has no such file.
fn not_in_depot(text: &str) -> bool {
    text.contains("no such file")
        || text.contains("not in client view")
        || text.contains("not under client")
}

/// A pipe read to the end on a thread of its own, so a full stderr cannot
/// stall the process while stdout is read.
fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// `path` with symbolic links resolved; for a file that does not exist yet,
/// its directory resolved and the name kept.
#[must_use]
pub fn real_path(path: &Path) -> PathBuf {
    if let Ok(real) = dunce::canonicalize(path) {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => dunce::canonicalize(parent)
            .map_or_else(|_| path.to_path_buf(), |parent| parent.join(name)),
        _ => path.to_path_buf(),
    }
}

/// `p4 -ztag` output as records: `... key value` lines (nested fields with
/// more dots), a blank line between records.
#[must_use]
pub fn parse_ztag(text: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut record = Record::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !record.is_empty() {
                records.push(std::mem::take(&mut record));
            }
            continue;
        }
        if !line.starts_with("... ") {
            continue;
        }
        let mut body = line;
        while let Some(rest) = body.strip_prefix("... ") {
            body = rest;
        }
        let (key, value) = body.split_once(' ').unwrap_or((body, ""));
        record.insert(key.to_string(), value.to_string());
    }
    if !record.is_empty() {
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ztag_records_split_on_blank_lines_and_nested_fields_flatten() {
        let records = parse_ztag(
            "... depotFile //depot/a.c\n... headRev 5\n... ... otherOpen0 bob@ws\n\n\
             ... depotFile //depot/b c.txt\n... desc two words\n... flag\n",
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["otherOpen0"], "bob@ws");
        assert_eq!(records[1]["depotFile"], "//depot/b c.txt");
        assert_eq!(records[1]["desc"], "two words");
        assert_eq!(records[1]["flag"], "");
    }

    #[test]
    fn a_missing_file_resolves_through_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let real = real_path(dir.path());
        assert_eq!(real_path(&dir.path().join("new.c")), real.join("new.c"));
    }
}
