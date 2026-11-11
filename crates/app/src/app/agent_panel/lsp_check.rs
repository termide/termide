//! The language server's errors for the file an agent's `edit` or `write`
//! just changed. The call runs on the agent's thread and the servers live
//! with the app, so the call sends the path down a process-wide channel and
//! waits. The app tells the server on its tick, as an editor would: the
//! editor that has the file follows it from disk and sends the change, and
//! a file no editor has is opened in its server for the check alone (the
//! server started first if need be) and closed once answered. The answer
//! goes out once the server's reports stop coming for a moment, or when the
//! time runs out.
//!
//! The errors a server last reported for a file while it was open are kept,
//! so the answer can say which errors the change brought.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use termide_agent_core::CancelToken;
use termide_agent_tools::{set_lsp_check, FileError, LspReport};
use termide_lsp::LspManager;

use crate::app::App;
use crate::PanelExt;

/// How long a check waits for the server, its start included.
const WAIT: Duration = Duration::from_secs(5);
/// How long the reports have to stop coming for the answer to go out: a
/// server may report a file more than once for one change.
const SETTLE: Duration = Duration::from_millis(300);

struct Request {
    path: PathBuf,
    reply: Sender<Option<LspReport>>,
}

/// The agents' checks under way, and what the servers said of the files.
#[derive(Default)]
pub(in crate::app) struct LspChecks {
    waiting: Vec<Waiter>,
    /// Documents opened in their server for a check, by their version.
    opened: HashMap<PathBuf, i32>,
    /// The errors last reported for a file while it was open.
    known: HashMap<PathBuf, Vec<FileError>>,
}

struct Waiter {
    path: PathBuf,
    lang: String,
    before: Option<Vec<FileError>>,
    /// The errors of the latest report since the check began.
    errors: Option<Vec<FileError>>,
    reported: Option<Instant>,
    deadline: Instant,
    reply: Sender<Option<LspReport>>,
}

fn port() -> &'static (Sender<Request>, Mutex<Receiver<Request>>) {
    static PORT: OnceLock<(Sender<Request>, Mutex<Receiver<Request>>)> = OnceLock::new();
    PORT.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        (sender, Mutex::new(receiver))
    })
}

/// When an app last served the checks.
static SERVED: Mutex<Option<Instant>> = Mutex::new(None);

/// The check `edit` and `write` run, installed once an app serves it.
fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        set_lsp_check(Arc::new(|path: &Path, cancel: &CancelToken| {
            // No app has served lately: none would answer.
            let served = *SERVED.lock().unwrap_or_else(PoisonError::into_inner);
            if served.is_none_or(|at| at.elapsed() > Duration::from_secs(1)) {
                return None;
            }
            let (reply, answer) = mpsc::channel();
            port()
                .0
                .send(Request {
                    path: path.to_path_buf(),
                    reply,
                })
                .ok()?;
            // The app answers by its deadline; this one only guards against
            // an app that went away.
            let give_up = Instant::now() + WAIT + Duration::from_secs(2);
            loop {
                match answer.recv_timeout(Duration::from_millis(100)) {
                    Ok(report) => return report,
                    Err(RecvTimeoutError::Disconnected) => return None,
                    Err(RecvTimeoutError::Timeout) => {
                        if cancel.is_cancelled() || Instant::now() > give_up {
                            return None;
                        }
                    }
                }
            }
        }));
    });
}

/// The errors among `diagnostics`; one with no severity counts as one.
fn errors(diagnostics: &[Diagnostic]) -> Vec<FileError> {
    diagnostics
        .iter()
        .filter(|d| d.severity.is_none_or(|s| s == DiagnosticSeverity::ERROR))
        .map(|d| FileError {
            line: d.range.start.line + 1,
            column: d.range.start.character + 1,
            message: d.message.clone(),
            source: d.source.clone(),
            code: d.code.as_ref().map(|code| match code {
                NumberOrString::Number(n) => n.to_string(),
                NumberOrString::String(s) => s.clone(),
            }),
        })
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl App {
    /// Take the agents' checks and answer the ones whose time has come.
    /// Called on every tick.
    pub(in crate::app) fn serve_lsp_check(&mut self) {
        install();
        *SERVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        let requests: Vec<Request> = port()
            .1
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_iter()
            .collect();
        for request in requests {
            match self.begin_lsp_check(request.path, &request.reply) {
                Some(waiter) => self.lsp_checks.waiting.push(waiter),
                None => {
                    let _ = request.reply.send(None);
                }
            }
        }
        self.answer_lsp_checks();
    }

    /// Tell the server of `path` it changed; `None` when no server is there
    /// to answer for it.
    fn begin_lsp_check(
        &mut self,
        path: PathBuf,
        reply: &Sender<Option<LspReport>>,
    ) -> Option<Waiter> {
        let path = canonical(&path);
        let lang = LspManager::detect_language(&path)?;
        let manager = self.state.lsp_manager.as_mut()?;
        manager.ensure_server(&lang, &path).ok()?;
        let before = self.lsp_checks.known.get(&path).cloned();
        let manager = self.state.lsp_manager.as_ref()?;
        let parked = self
            .open_projects
            .parked_mut()
            .flat_map(|(_, project)| project.panels_mut());
        let mut editors = self
            .layout_manager
            .iter_all_panels_mut()
            .chain(parked)
            .filter_map(|panel| panel.as_editor_mut());
        match editors.find(|editor| editor.file_path().is_some_and(|p| canonical(p) == path)) {
            // The server reads the editor's unsaved text, not the file.
            Some(editor) if editor.buffer().is_modified() => return None,
            Some(editor) => {
                // The editor follows the file now rather than when the
                // watcher gets to it, and sends the server the change.
                editor.check_external_modification();
                editor.flush_lsp_changes(manager);
                let opened = editor.file_path().map(Path::to_path_buf)?;
                manager.did_save(&lang, &opened, None);
            }
            None => {
                let text = std::fs::read_to_string(&path).ok()?;
                match self.lsp_checks.opened.get_mut(&path) {
                    Some(version) => {
                        *version += 1;
                        manager.did_change_full(&lang, &path, *version, &text);
                    }
                    None => {
                        manager.did_open(&lang, &path, 1, &text);
                        self.lsp_checks.opened.insert(path.clone(), 1);
                    }
                }
                manager.did_save(&lang, &path, None);
            }
        }
        Some(Waiter {
            path,
            lang,
            before,
            errors: None,
            reported: None,
            deadline: Instant::now() + WAIT,
            reply: reply.clone(),
        })
    }

    /// Note what a server reported for `path`: for the checks waiting on
    /// it, and as what the file has while it is open somewhere.
    pub(in crate::app) fn note_lsp_report(&mut self, path: &Path, diagnostics: &[Diagnostic]) {
        let parked = self
            .open_projects
            .parked_mut()
            .flat_map(|(_, project)| project.panels_mut());
        let open_in_editor = self
            .layout_manager
            .iter_all_panels_mut()
            .chain(parked)
            .filter_map(|panel| panel.as_editor_mut())
            .any(|editor| editor.file_path() == Some(path));
        let checks = &self.lsp_checks;
        if checks.waiting.is_empty() && checks.opened.is_empty() && !open_in_editor {
            // Reported after it was closed, or a file never opened.
            return;
        }
        let path = canonical(path);
        let found = errors(diagnostics);
        let now = Instant::now();
        let mut waited = false;
        for waiter in &mut self.lsp_checks.waiting {
            if waiter.path == path {
                waiter.errors = Some(found.clone());
                waiter.reported = Some(now);
                waited = true;
            }
        }
        if waited || open_in_editor || self.lsp_checks.opened.contains_key(&path) {
            self.lsp_checks.known.insert(path, found);
        }
    }

    fn answer_lsp_checks(&mut self) {
        if self.lsp_checks.waiting.is_empty() {
            return;
        }
        let now = Instant::now();
        let waiting = std::mem::take(&mut self.lsp_checks.waiting);
        let mut done = HashSet::new();
        for waiter in waiting {
            let settled = waiter
                .reported
                .is_some_and(|at| now.duration_since(at) >= SETTLE);
            if !settled && now < waiter.deadline {
                self.lsp_checks.waiting.push(waiter);
                continue;
            }
            let report = waiter.errors.map(|errors| LspReport {
                errors,
                before: waiter.before,
            });
            let _ = waiter.reply.send(report);
            done.insert((waiter.path, waiter.lang));
        }
        // A document opened for the checks alone is closed with the last.
        for (path, lang) in done {
            let still = self.lsp_checks.waiting.iter().any(|w| w.path == path);
            if !still && self.lsp_checks.opened.remove(&path).is_some() {
                if let Some(manager) = self.state.lsp_manager.as_ref() {
                    manager.did_close(&lang, &path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    /// Errors, and what reports no severity, reach the agent as 1-based
    /// places; warnings and hints do not.
    #[test]
    fn only_errors_are_taken_with_their_place_and_code() {
        let diagnostic = |severity, message: &str| Diagnostic {
            range: Range::new(Position::new(4, 8), Position::new(4, 12)),
            severity,
            code: Some(NumberOrString::String("E0308".into())),
            source: Some("rustc".into()),
            message: message.into(),
            ..Diagnostic::default()
        };
        let found = errors(&[
            diagnostic(Some(DiagnosticSeverity::ERROR), "mismatched types"),
            diagnostic(Some(DiagnosticSeverity::WARNING), "unused variable"),
            diagnostic(Some(DiagnosticSeverity::HINT), "consider this"),
            diagnostic(None, "no severity"),
        ]);
        assert_eq!(
            found,
            [
                FileError {
                    line: 5,
                    column: 9,
                    message: "mismatched types".into(),
                    source: Some("rustc".into()),
                    code: Some("E0308".into()),
                },
                FileError {
                    line: 5,
                    column: 9,
                    message: "no severity".into(),
                    source: Some("rustc".into()),
                    code: Some("E0308".into()),
                },
            ]
        );
    }
}
