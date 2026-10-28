//! The project environment `direnv export json` gives a directory, for the
//! background and report commands.
//!
//! direnv can take seconds — nix-direnv evaluates a flake — so it never runs
//! on the UI thread. A lookup answers from the cache while the answer is
//! younger than [`TTL`]; otherwise it starts direnv on a worker thread and
//! the command waits, queued, until the worker reports back.

use std::collections::HashMap;
use std::path::Path;

/// Variables to set (`Some`) and to unset (`None`).
pub(super) type DirenvEnv = HashMap<String, Option<String>>;

/// Where a lookup stands.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Lookup {
    /// The environment to apply; `None` when direnv has none for the
    /// directory or is not installed.
    Ready(Option<DirenvEnv>),
    /// direnv is running for the directory; ask again later.
    Pending,
}

/// How long an answer is used before direnv is asked again.
#[cfg(unix)]
const TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// The environment for commands run in `cwd`, starting direnv in the
/// background when the cache has no fresh answer.
#[cfg(unix)]
pub(super) fn lookup(cwd: &Path) -> Lookup {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, PoisonError};
    use std::time::Instant;

    #[derive(Default)]
    struct Entry {
        /// The last answer and when it came.
        answer: Option<(Option<DirenvEnv>, Instant)>,
        /// A worker is running direnv for the directory.
        refreshing: bool,
    }

    /// Set once running direnv finds no such program.
    static MISSING: AtomicBool = AtomicBool::new(false);
    static CACHE: Mutex<Option<HashMap<PathBuf, Entry>>> = Mutex::new(None);

    if MISSING.load(Ordering::Relaxed) {
        return Lookup::Ready(None);
    }

    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let entry = cache
        .get_or_insert_with(HashMap::new)
        .entry(cwd.to_path_buf())
        .or_default();
    if let Some((env, at)) = &entry.answer {
        if at.elapsed() < TTL {
            return Lookup::Ready(env.clone());
        }
    }
    if !entry.refreshing {
        entry.refreshing = true;
        let cwd = cwd.to_path_buf();
        std::thread::spawn(move || {
            let output = std::process::Command::new("direnv")
                .args(["export", "json"])
                .current_dir(&cwd)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output();
            let env = match output {
                Ok(output) if output.status.success() => parse_export_json(&output.stdout),
                Ok(_) => None,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        MISSING.store(true, Ordering::Relaxed);
                    }
                    None
                }
            };
            let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = cache
                .get_or_insert_with(HashMap::new)
                .entry(cwd)
                .or_default();
            entry.answer = Some((env, Instant::now()));
            entry.refreshing = false;
        });
    }
    Lookup::Pending
}

#[cfg(not(unix))]
pub(super) fn lookup(_cwd: &Path) -> Lookup {
    Lookup::Ready(None)
}

/// Parse what `direnv export json` prints: an object of strings to set and
/// nulls to unset. Nothing printed — no `.envrc`, or nothing to change — and
/// anything unreadable give `None`.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_export_json(stdout: &[u8]) -> Option<DirenvEnv> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    match serde_json::from_slice(stdout) {
        Ok(env) => Some(env),
        Err(e) => {
            log::warn!("Unreadable `direnv export json` output: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a hand-rolled unescape turned `\\n` (a backslash then
    /// `n`) into a newline and left `\uXXXX` and `\/` as written.
    #[test]
    fn values_are_unescaped_as_json() {
        let out = br#"{
  "WIN_PATH": "C:\\new\\table",
  "GREETING": "line one\nline two\t\"quoted\"",
  "SNOWMAN": "\u2603 \u00e9",
  "URL": "http:\/\/example.com",
  "OLD_VAR": null
}
"#;
        let env = parse_export_json(out).expect("an environment");
        assert_eq!(env["WIN_PATH"].as_deref(), Some(r"C:\new\table"));
        assert_eq!(
            env["GREETING"].as_deref(),
            Some("line one\nline two\t\"quoted\"")
        );
        assert_eq!(env["SNOWMAN"].as_deref(), Some("\u{2603} \u{e9}"));
        assert_eq!(env["URL"].as_deref(), Some("http://example.com"));
        assert_eq!(env["OLD_VAR"], None);
        assert_eq!(env.len(), 5);
    }

    /// A value holding a colon or a comma stays whole: the old parser split
    /// lines on the first colon.
    #[test]
    fn single_line_output_parses_too() {
        let env = parse_export_json(br#"{"A":"x:y,z","B":null}"#).unwrap();
        assert_eq!(env["A"].as_deref(), Some("x:y,z"));
        assert_eq!(env["B"], None);
    }

    /// The lookup never blocks: it answers at once or reports the worker it
    /// started, which answers soon after — direnv installed or not.
    #[test]
    #[cfg(unix)]
    fn a_lookup_answers_without_waiting_for_direnv() {
        let dir = tempfile::tempdir().unwrap();
        let mut answer = lookup(dir.path());
        for _ in 0..500 {
            if answer != Lookup::Pending {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            answer = lookup(dir.path());
        }
        // No `.envrc` there: nothing to apply.
        assert_eq!(answer, Lookup::Ready(None));
        // Fresh now: answered from the cache.
        assert_eq!(lookup(dir.path()), Lookup::Ready(None));
    }

    #[test]
    fn empty_or_unreadable_output_is_no_environment() {
        assert_eq!(parse_export_json(b""), None);
        assert_eq!(parse_export_json(b"  \n"), None);
        assert_eq!(parse_export_json(b"not json"), None);
    }
}
