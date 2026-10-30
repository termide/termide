//! What the plugin does: the state of the file in the editor in the status
//! bar, open for edit or add, revert, diff, a panel of the opened files with
//! submit and sync, and tools for the agent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_plugins::protocol::{self, events};
use termide_plugins::rpc::RpcError;

use crate::p4::{parse_ztag, P4Error, Record, P4};
use crate::termide::{unknown_method, Handler, Termide};

/// The status bar item's id.
const STATUS: &str = "p4";

/// The answer to `initialize`.
#[must_use]
pub fn contributions() -> Value {
    json!({
        "subscriptions": [
            events::FILE_OPENED, events::EDITOR_FOCUSED, events::FILE_CLOSED, events::FILE_SAVED,
            events::FILE_WILL_EDIT,
        ],
        "commands": [
            { "id": "edit", "title": "Open the file for edit or add" },
            { "id": "revert", "title": "Revert the file" },
            { "id": "diff", "title": "Show the file's changes" },
            { "id": "opened", "title": "Opened files", "replaces": "open_git_status" },
            { "id": "sync", "title": "Sync the workspace" },
        ],
        "tools": [
            {
                "name": "opened",
                "description": "List the files opened in Perforce in this project: action, changelist and path.",
                "inputSchema": { "type": "object", "properties": {} },
                "readOnly": true,
            },
            {
                "name": "edit",
                "description": "Open files in Perforce so they can be written: files the depot has are opened for edit (they are read-only until then), files it does not have are opened for add.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "paths": { "type": "array", "items": { "type": "string" } } },
                    "required": ["paths"],
                },
                "readOnly": false,
            },
            {
                "name": "diff",
                "description": "Show the unified diff of an opened file against the depot.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                },
                "readOnly": true,
            },
        ],
        "providers": {},
    })
}

/// The Perforce plugin of one project.
pub struct Perforce {
    termide: Termide,
    p4: P4,
    /// The panel of opened files, one action at a time.
    panel: Mutex<OpenedPanel>,
    /// What termide was told of the files.
    marks: Mutex<Marks>,
}

/// What termide was last told of the files, so only changes are sent.
#[derive(Default)]
struct Marks {
    /// The file manager's colour of each opened file.
    statuses: BTreeMap<PathBuf, &'static str>,
    /// For each editor's file given a base, which revision it is.
    bases: BTreeMap<PathBuf, String>,
    /// When the opened files were last asked for.
    asked: Option<Instant>,
}

/// How often focusing editors may ask p4 for the opened files again.
const STATUS_INTERVAL: Duration = Duration::from_secs(2);

/// The file manager's colour for a file opened for `action`.
fn status_of(action: &str) -> &'static str {
    match action {
        "add" | "move/add" | "branch" | "import" => "added",
        "delete" | "move/delete" | "purge" | "archive" => "deleted",
        _ => "modified",
    }
}

#[derive(Default)]
struct OpenedPanel {
    id: Option<u64>,
    /// The files the panel lists, by the index its links name.
    files: Vec<Record>,
}

impl Handler for Perforce {
    fn handle(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        let path = || params["path"].as_str().map(PathBuf::from);
        match method {
            protocol::INITIALIZED => self.refresh_statuses(true),
            events::FILE_OPENED | events::EDITOR_FOCUSED => {
                if let Some(path) = path() {
                    self.show_status(&path);
                }
                self.refresh_statuses(false);
            }
            events::FILE_SAVED => self.refresh_statuses(false),
            events::FILE_CLOSED => {
                if self.termide.active().is_null() {
                    self.termide.status(STATUS, "");
                }
            }
            events::FILE_WILL_EDIT => {
                let Some(path) = path() else {
                    return Ok(json!({ "handled": false }));
                };
                if params["edit"].as_bool().unwrap_or(true) {
                    return Ok(json!({ "handled": self.will_edit(&path) }));
                }
                let modified = params["modified"].as_bool().unwrap_or(false);
                return Ok(self.will_stop_editing(&path, modified));
            }
            protocol::COMMAND_EXECUTE => self.command(params["id"].as_str().unwrap_or_default()),
            protocol::TOOL_CALL => {
                let name = params["name"].as_str().unwrap_or_default();
                let outcome = self.tool(name, &params["arguments"]);
                if name == "edit" {
                    self.after_change();
                }
                return Ok(match outcome {
                    Ok(text) => json!({ "text": text, "isError": false }),
                    Err(error) => json!({ "text": error.0, "isError": true }),
                });
            }
            protocol::PANEL_ACTION => {
                let action = params["action"].as_str().unwrap_or_default();
                if let Err(error) = self.panel_action(params["panel"].as_u64(), action) {
                    self.report(&error);
                }
                if action.starts_with("revert")
                    || action.starts_with("submit")
                    || action == "refresh"
                {
                    self.after_change();
                }
            }
            protocol::PANEL_CLOSED => {
                let mut panel = self.lock_panel();
                if panel.id == params["panel"].as_u64() {
                    *panel = OpenedPanel::default();
                }
            }
            other => return Err(unknown_method(other)),
        }
        Ok(Value::Null)
    }
}

impl Perforce {
    #[must_use]
    pub fn new(termide: Termide, p4: P4) -> Self {
        Self {
            termide,
            p4,
            panel: Mutex::new(OpenedPanel::default()),
            marks: Mutex::new(Marks::default()),
        }
    }

    fn lock_marks(&self) -> std::sync::MutexGuard<'_, Marks> {
        self.marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_panel(&self) -> std::sync::MutexGuard<'_, OpenedPanel> {
        self.panel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn root(&self) -> &Path {
        self.p4.project_root()
    }

    // ------------------------------------------------------------ status

    /// Show the state of `path`, or nothing when p4 cannot tell.
    fn show_status(&self, path: &Path) {
        if !self.p4.in_workspace(path) {
            self.termide.status(STATUS, "");
            return;
        }
        let text = match self.p4.fstat(path) {
            Ok(record) => {
                self.refresh_base(path, record.as_ref());
                describe(record.as_ref())
            }
            Err(error) => {
                eprintln!("perforce: fstat {}: {error}", path.display());
                String::new()
            }
        };
        self.termide.status(STATUS, &text);
    }

    /// Tell the file manager which files are opened, and how; `force`
    /// asks p4 even if it was asked a moment ago.
    fn refresh_statuses(&self, force: bool) {
        {
            let mut marks = self.lock_marks();
            let recent = marks.asked.is_some_and(|at| at.elapsed() < STATUS_INTERVAL);
            if recent && !force {
                return;
            }
            marks.asked = Some(Instant::now());
        }
        if !self.p4.in_workspace(self.root()) {
            return;
        }
        let files = match self.opened_files() {
            Ok(files) => files,
            Err(error) => {
                eprintln!("perforce: opened: {error}");
                return;
            }
        };
        let now: BTreeMap<PathBuf, &'static str> = files
            .iter()
            .filter_map(|r| {
                let path = self.p4.in_project(Path::new(&r["clientFile"]))?;
                Some((path, status_of(r.get("action").map_or("", String::as_str))))
            })
            .collect();
        let mut marks = self.lock_marks();
        let mut items: Vec<(PathBuf, Option<&str>)> = marks
            .statuses
            .keys()
            .filter(|path| !now.contains_key(*path))
            .map(|path| (path.clone(), None))
            .collect();
        items.extend(
            now.iter()
                .filter(|(path, status)| marks.statuses.get(*path) != Some(status))
                .map(|(path, status)| (path.clone(), Some(*status))),
        );
        marks.statuses = now;
        drop(marks);
        self.termide.file_status(&items);
    }

    /// Give the editor of `path` the depot's text to mark changes against:
    /// the revision the workspace has, nothing for a file opened for add
    /// (every line is new), and none for a file the depot does not have or
    /// a binary one. Asks p4 only when the revision changed.
    fn refresh_base(&self, path: &Path, record: Option<&Record>) {
        let binary = record
            .and_then(|r| r.get("headType").or_else(|| r.get("type")))
            .is_some_and(|t| t.contains("binary") || t.contains("apple") || t.contains("resource"));
        let wanted = match record {
            _ if binary => None,
            Some(r) if r.get("action").map(String::as_str) == Some("add") => {
                Some("add".to_string())
            }
            Some(r) => r.get("haveRev").map(|have| format!("#{have}")),
            None => None,
        };
        let sent = self.lock_marks().bases.get(path).cloned();
        if wanted == sent {
            return;
        }
        let text = match wanted.as_deref() {
            None => None,
            Some("add") => Some(String::new()),
            Some(rev) => {
                let at = format!("{}{rev}", path.to_string_lossy());
                match self.p4.run(&["print", "-q", &at], path.parent()) {
                    Ok(text) => Some(text),
                    Err(error) => {
                        eprintln!("perforce: print {at}: {error}");
                        return;
                    }
                }
            }
        };
        {
            let mut marks = self.lock_marks();
            match &wanted {
                Some(rev) => marks.bases.insert(path.to_path_buf(), rev.clone()),
                None => marks.bases.remove(path),
            };
        }
        self.termide.base(path, text.as_deref());
    }

    /// The files given bases looked at again: after a submit or a revert
    /// their revision may have moved.
    fn refresh_bases(&self) {
        let paths: Vec<PathBuf> = self.lock_marks().bases.keys().cloned().collect();
        for path in paths {
            match self.p4.fstat(&path) {
                Ok(record) => self.refresh_base(&path, record.as_ref()),
                Err(error) => eprintln!("perforce: fstat {}: {error}", path.display()),
            }
        }
    }

    /// After the user or the agent changed what is opened.
    fn after_change(&self) {
        self.refresh_statuses(true);
        self.refresh_bases();
    }

    /// Tell the user what p4 said; an expired login is offered a terminal.
    fn report(&self, error: &P4Error) {
        let text = &error.0;
        let lower = text.to_lowercase();
        if lower.contains("login")
            && (text.contains("expired") || text.contains("invalid") || text.contains("P4PASSWD"))
        {
            let question = format!("{text}\n\nRun `p4 login` in a terminal?");
            if self.termide.confirm("Login", &question) {
                self.termide.run_in_terminal(
                    &format!("{} login", shell_word(&self.p4.program().to_string_lossy())),
                    self.root(),
                );
            }
            return;
        }
        self.termide.notify(text, "error");
    }

    /// The active editor's file, when it is in the workspace.
    fn active_path(&self) -> Option<PathBuf> {
        let active = self.termide.active();
        let Some(path) = active["path"].as_str().filter(|p| !p.is_empty()) else {
            self.termide
                .notify("Open a file in the editor first", "info");
            return None;
        };
        let path = PathBuf::from(path);
        if !self.p4.in_workspace(&path) {
            let text = format!("{} is not in a Perforce workspace", self.p4.relative(&path));
            self.termide.notify(&text, "error");
            return None;
        }
        Some(path)
    }

    fn project_in_workspace(&self) -> bool {
        if self.p4.in_workspace(self.root()) {
            return true;
        }
        self.termide
            .notify("This project is not in a Perforce workspace", "error");
        false
    }

    fn sync_line(&self) -> String {
        format!("{} sync", shell_word(&self.p4.program().to_string_lossy()))
    }

    // ---------------------------------------------------------- commands

    fn command(&self, id: &str) {
        let outcome = match id {
            "edit" => self.edit_command(),
            "revert" => self.revert_command(),
            "diff" => self.diff_command(),
            "opened" => self.opened_command(),
            "sync" => {
                if self.project_in_workspace() {
                    self.termide.run_in_terminal(&self.sync_line(), self.root());
                }
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = outcome {
            self.report(&error);
        }
        if matches!(id, "edit" | "revert") {
            self.after_change();
        }
    }

    /// Editing was turned on (`Ctrl+E`) for a file that is read-only on
    /// disk: open it for edit, which makes it writable, and the editor
    /// follows. Whether it was the plugin's to deal with: a file the depot
    /// does not have, or one already opened, is left to termide, since
    /// `p4` would not make it writable.
    fn will_edit(&self, path: &Path) -> bool {
        if !self.p4.in_workspace(path) {
            return false;
        }
        match self.p4.fstat(path) {
            Ok(Some(record)) if !record.contains_key("action") => {}
            Ok(_) => return false,
            Err(error) => {
                self.report(&error);
                return true;
            }
        }
        match self.open_for_edit(path) {
            Ok(Some(_)) => self.show_status(path),
            Ok(None) => {}
            Err(error) => self.report(&error),
        }
        self.after_change();
        true
    }

    /// Editing was turned off (`Ctrl+E`) for a file that is writable: for
    /// one opened in Perforce, ask whether to revert it too, and what to do
    /// with unsaved work. The answer to `file/will_edit`: only a dismissed
    /// question, or a revert p4 refused, keeps editing on.
    fn will_stop_editing(&self, path: &Path, modified: bool) -> Value {
        let not_ours = json!({ "handled": false });
        if !self.p4.in_workspace(path) {
            return not_ours;
        }
        let action = match self.p4.fstat(path) {
            Ok(Some(mut record)) => match record.remove("action") {
                Some(action) => action,
                None => return not_ours,
            },
            Ok(None) => return not_ours,
            Err(error) => {
                eprintln!("perforce: fstat {}: {error}", path.display());
                return not_ours;
            }
        };
        let name = self.p4.relative(path);
        let losing = if modified {
            "losing its saved and unsaved changes"
        } else {
            "losing its changes"
        };
        let revert = if action == "add" {
            format!("Revert it in Perforce: no longer add it, {losing}")
        } else {
            format!("Revert it in Perforce, {losing}")
        };
        // The harmless choices first, where `Enter` lands.
        let (title, mut items) = if modified {
            (
                format!("Stop editing {name}: it has unsaved changes"),
                vec![
                    format!("Save, then stop editing: keep it opened for {action}"),
                    format!("Discard the unsaved changes: keep it opened for {action}"),
                ],
            )
        } else {
            (
                format!("Stop editing {name}"),
                vec![format!("Only stop editing: keep it opened for {action}")],
            )
        };
        items.push(revert);
        let items: Vec<&str> = items.iter().map(String::as_str).collect();
        let picked = self.termide.choose(&title, &items);
        match picked {
            None => return json!({ "handled": true }),
            Some(0) if modified => return json!({ "buffer": "save" }),
            Some(index) if index + 1 < items.len() => {
                return if modified {
                    json!({ "buffer": "discard" })
                } else {
                    not_ours
                };
            }
            Some(_) => {}
        }
        let outcome = self
            .p4
            .run(&["revert", &path.to_string_lossy()], path.parent());
        self.after_change();
        match outcome {
            Ok(_) => {
                self.show_status(path);
                // The file on disk is the depot's again: the buffer follows.
                json!({ "buffer": "discard" })
            }
            Err(error) => {
                self.report(&error);
                json!({ "handled": true })
            }
        }
    }

    fn edit_command(&self) -> Result<(), P4Error> {
        let Some(path) = self.active_path() else {
            return Ok(());
        };
        if self.open_for_edit(&path)?.is_some() {
            self.show_status(&path);
        }
        Ok(())
    }

    /// `p4 edit`, or `p4 add` for a file the depot does not have; asks
    /// first when someone else holds the file. What was done, or `None`.
    fn open_for_edit(&self, path: &Path) -> Result<Option<&'static str>, P4Error> {
        let record = self.p4.fstat(path)?;
        let name = self.p4.relative(path);
        let arg = path.to_string_lossy();
        let Some(record) = record else {
            let question = format!("{name} is not in the depot. Open it for add?");
            if !self.termide.confirm("Open for add", &question) {
                return Ok(None);
            }
            self.p4.run(&["add", &arg], path.parent())?;
            return Ok(Some("add"));
        };
        if let Some(action) = record.get("action") {
            self.termide
                .notify(&format!("{name} is already opened for {action}"), "info");
            return Ok(None);
        }
        if let Some(locker) = record.get("otherLock0") {
            self.termide
                .notify(&format!("{name} is locked by {locker}"), "error");
            return Ok(None);
        }
        if let Some(other) = record.get("otherOpen0") {
            let question = format!("{name} is also opened by {other}. Open it for edit too?");
            if !self.termide.confirm("Open for edit", &question) {
                return Ok(None);
            }
        }
        self.p4.run(&["edit", &arg], path.parent())?;
        Ok(Some("edit"))
    }

    fn revert_command(&self) -> Result<(), P4Error> {
        let Some(path) = self.active_path() else {
            return Ok(());
        };
        let opened = self
            .p4
            .fstat(&path)?
            .is_some_and(|record| record.contains_key("action"));
        if !opened {
            let text = format!("{} is not opened", self.p4.relative(&path));
            self.termide.notify(&text, "info");
            return Ok(());
        }
        if self.revert(&path)? {
            self.show_status(&path);
        }
        Ok(())
    }

    /// `p4 revert`, asked about first; whether it was done.
    fn revert(&self, path: &Path) -> Result<bool, P4Error> {
        let question = format!(
            "Revert {}? The changes made to it since it was opened are lost.",
            self.p4.relative(path)
        );
        if !self.termide.confirm("Revert", &question) {
            return Ok(false);
        }
        self.p4
            .run(&["revert", &path.to_string_lossy()], path.parent())?;
        Ok(true)
    }

    fn diff(&self, path: &Path) -> Result<String, P4Error> {
        self.p4
            .run(&["diff", "-du", &path.to_string_lossy()], path.parent())
    }

    /// The file's changes as a unified diff named by its path in the
    /// project; only the headers when it has none.
    fn diff_unified(&self, path: &Path) -> Result<String, P4Error> {
        let out = self.diff(path)?;
        let name = self.p4.relative(path);
        // p4 names the sides by depot and local path; the panel wants the
        // file's place in the project, and the hunks keep their own lines.
        let hunks: Vec<&str> = out
            .lines()
            .skip_while(|line| !line.starts_with("@@"))
            .collect();
        let mut diff = format!("--- a/{name}\n+++ b/{name}\n");
        for line in hunks {
            diff.push_str(line);
            diff.push('\n');
        }
        Ok(diff)
    }

    fn show_diff(&self, path: &Path) -> Result<(), P4Error> {
        let diff = self.diff_unified(path)?;
        let file = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.termide
            .open_diff(&format!("diff {file}"), &diff)
            .map_err(P4Error)?;
        Ok(())
    }

    fn diff_command(&self) -> Result<(), P4Error> {
        match self.active_path() {
            Some(path) => self.show_diff(&path),
            None => Ok(()),
        }
    }

    fn opened_command(&self) -> Result<(), P4Error> {
        if !self.project_in_workspace() {
            return Ok(());
        }
        self.show_panel(&mut self.lock_panel())
    }

    // ---------------------------------------------------- opened files

    /// The project's opened files: records with `clientFile` a local path,
    /// `action` and `change`.
    fn opened_files(&self) -> Result<Vec<Record>, P4Error> {
        let all = self.root().join("...");
        match self
            .p4
            .run(&["-ztag", "fstat", "-Ro", &all.to_string_lossy()], None)
        {
            Ok(out) => Ok(parse_ztag(&out)
                .into_iter()
                .filter(|r| r.contains_key("clientFile"))
                .collect()),
            Err(error) if error.0.contains("not opened") || error.0.contains("no such file") => {
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    /// Descriptions of the workspace's numbered pending changelists.
    fn pending_descriptions(&self) -> Result<BTreeMap<String, String>, P4Error> {
        let info = parse_ztag(&self.p4.run(&["-ztag", "info"], None)?);
        let client = info
            .first()
            .and_then(|r| r.get("clientName"))
            .filter(|c| *c != "*unknown*");
        let Some(client) = client else {
            return Ok(BTreeMap::new());
        };
        let out = self
            .p4
            .run(&["-ztag", "changes", "-s", "pending", "-c", client], None)?;
        Ok(parse_ztag(&out)
            .into_iter()
            .filter_map(|r| {
                let change = r.get("change")?.clone();
                Some((change, r.get("desc").map_or("", |d| d.trim()).to_string()))
            })
            .collect())
    }

    /// The opened files as a list panel shows them: a group by changelist,
    /// a row by file.
    fn panel_list(&self, panel: &mut OpenedPanel) -> Result<Value, P4Error> {
        panel.files = self.opened_files()?;
        let descriptions = if panel.files.is_empty() {
            BTreeMap::new()
        } else {
            self.pending_descriptions()?
        };
        let mut groups: BTreeMap<(bool, String), Vec<usize>> = BTreeMap::new();
        for (index, record) in panel.files.iter().enumerate() {
            let change = record
                .get("change")
                .cloned()
                .unwrap_or_else(|| "default".to_string());
            groups
                .entry((change != "default", change))
                .or_default()
                .push(index);
        }
        let groups: Vec<Value> = groups
            .iter()
            .map(|((_, change), indices)| {
                let label = if change == "default" {
                    "Default changelist".to_string()
                } else {
                    format!("Change {change}")
                };
                let detail = descriptions.get(change).map_or("", String::as_str);
                let items: Vec<Value> = indices
                    .iter()
                    .map(|&index| {
                        let record = &panel.files[index];
                        let action = record.get("action").map_or("?", String::as_str);
                        let name = self.p4.relative(Path::new(&record["clientFile"]));
                        json!({
                            "label": name,
                            "path": name,
                            "detail": action,
                            "status": status_of(action),
                            "actions": [
                                { "label": "Open", "action": format!("open-{index}"), "key": "edit" },
                                { "label": "Diff", "action": format!("diff-{index}"), "key": "view" },
                                { "label": "Revert", "action": format!("revert-{index}"), "key": "revert" },
                            ],
                        })
                    })
                    .collect();
                json!({
                    "label": label,
                    "detail": detail,
                    "actions": [{ "label": "Submit", "action": format!("submit-{change}") }],
                    "items": items,
                })
            })
            .collect();
        Ok(json!({
            "groups": groups,
            "empty": "No files are opened in this project.",
            "refresh": "refresh",
            "buttons": [
                { "label": "Refresh", "action": "refresh" },
                { "label": "Sync the workspace", "action": "sync" },
            ],
        }))
    }

    fn show_panel(&self, panel: &mut OpenedPanel) -> Result<(), P4Error> {
        let list = self.panel_list(panel)?;
        match panel.id {
            Some(id) => self.termide.update_list(id, list).map_err(P4Error),
            None => {
                let id = self
                    .termide
                    .open_list("Opened files", list)
                    .map_err(P4Error)?;
                panel.id = Some(id);
                Ok(())
            }
        }
    }

    fn panel_action(&self, id: Option<u64>, action: &str) -> Result<(), P4Error> {
        let mut panel = self.lock_panel();
        if panel.id.is_none() || panel.id != id {
            return Ok(());
        }
        let (kind, arg) = action.split_once('-').unwrap_or((action, ""));
        let file = |panel: &OpenedPanel| {
            arg.parse::<usize>()
                .ok()
                .and_then(|i| panel.files.get(i))
                .map(|r| PathBuf::from(&r["clientFile"]))
        };
        match kind {
            "refresh" => self.show_panel(&mut panel)?,
            "sync" => self.termide.run_in_terminal(&self.sync_line(), self.root()),
            "open" => {
                if let Some(path) = file(&panel) {
                    self.termide.open_file(&path.to_string_lossy());
                }
            }
            "diff" => {
                if let Some(path) = file(&panel) {
                    self.show_diff(&path)?;
                }
            }
            "revert" => {
                if let Some(path) = file(&panel) {
                    if self.revert(&path)? {
                        self.show_panel(&mut panel)?;
                    }
                }
            }
            "submit" => {
                // A failed submit still moves the default changelist's
                // files into a numbered one: the panel shows where they are.
                let submitted = self.submit(arg);
                let shown = self.show_panel(&mut panel);
                submitted?;
                shown?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Submit a changelist: the default one with a description and the
    /// files to go asked for, a numbered one with its own. Shows what p4
    /// answered.
    fn submit(&self, change: &str) -> Result<(), P4Error> {
        let out = if change == "default" {
            let Some(out) = self.submit_default()? else {
                return Ok(());
            };
            out
        } else {
            if !self
                .termide
                .confirm("Submit", &format!("Submit change {change}?"))
            {
                return Ok(());
            }
            self.p4.run(&["submit", "-c", change], None)?
        };
        let out = out.trim();
        self.termide
            .message("Submitted", if out.is_empty() { "Done." } else { out });
        Ok(())
    }

    /// Ask for the default changelist's description and which of its files
    /// go, then submit them: all of them as they are, a part of them moved
    /// to a changelist of their own first. What p4 answered, or `None` when
    /// nothing was submitted.
    fn submit_default(&self) -> Result<Option<String>, P4Error> {
        // Asked anew: files may have been opened since the panel was shown.
        let opened = self.opened_files()?;
        let files: Vec<&Record> = opened
            .iter()
            .filter(|r| r.get("change").is_none_or(|c| c == "default"))
            .collect();
        let items: Vec<Value> = files
            .iter()
            .map(|r| {
                json!({
                    "label": self.p4.relative(Path::new(&r["clientFile"])),
                    "detail": r.get("action").map_or("", String::as_str),
                    "checked": true,
                })
            })
            .collect();
        let answer = self.termide.dialog(
            "Submit the default changelist",
            json!({
                "input": { "label": "Description of the change", "lines": 4 },
                "list": { "items": items, "pick": "many" },
                "buttons": ["Submit", "Cancel"],
            }),
        );
        let Some(answer) = answer.filter(|a| a["button"] == 0) else {
            return Ok(None);
        };
        let description = answer["text"].as_str().unwrap_or_default().trim();
        if description.is_empty() {
            self.termide
                .message("Not submitted", "A change needs a description.");
            return Ok(None);
        }
        let picked: Vec<&str> = answer["checked"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .filter_map(|i| files.get(usize::try_from(i).ok()?))
            .filter_map(|r| r.get("depotFile").map(String::as_str))
            .collect();
        if picked.is_empty() {
            self.termide
                .message("Not submitted", "No files were picked.");
            return Ok(None);
        }
        if picked.len() == files.len() {
            return self.p4.run(&["submit", "-d", description], None).map(Some);
        }
        let form = self.p4.run(&["change", "-o"], None)?;
        let created = self
            .p4
            .run_with_input(&["change", "-i"], &change_spec(&form, description, &picked))?;
        let change = created_change(&created)
            .ok_or_else(|| P4Error(format!("p4 change: {}", created.trim())))?;
        self.p4.run(&["submit", "-c", &change], None).map(Some)
    }

    // ------------------------------------------------------------- tools

    fn tool(&self, name: &str, arguments: &Value) -> Result<String, P4Error> {
        match name {
            "opened" => {
                if !self.p4.in_workspace(self.root()) {
                    return Err(P4Error(
                        "This project is not in a Perforce workspace".to_string(),
                    ));
                }
                let files = self.opened_files()?;
                if files.is_empty() {
                    return Ok("No files are opened.".to_string());
                }
                Ok(files
                    .iter()
                    .map(|r| {
                        format!(
                            "{}\t{}\t{}",
                            r.get("action").map_or("?", String::as_str),
                            r.get("change").map_or("default", String::as_str),
                            self.p4.relative(Path::new(&r["clientFile"]))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "edit" => {
                let mut results = Vec::new();
                for path in arguments["paths"].as_array().into_iter().flatten() {
                    let Some(path) = path.as_str() else { continue };
                    let path = self.root().join(path);
                    let name = self.p4.relative(&path);
                    if !self.p4.in_workspace(&path) {
                        results.push(format!("{name}: not in a Perforce workspace"));
                        continue;
                    }
                    let record = self.p4.fstat(&path)?;
                    if let Some(action) = record.as_ref().and_then(|r| r.get("action")) {
                        results.push(format!("{name}: already opened for {action}"));
                        continue;
                    }
                    let verb = if record.is_none() { "add" } else { "edit" };
                    let out = self
                        .p4
                        .run(&[verb, &path.to_string_lossy()], path.parent())?;
                    let out = out.trim();
                    results.push(if out.is_empty() {
                        format!("{name}: opened for {verb}")
                    } else {
                        out.to_string()
                    });
                }
                if results.is_empty() {
                    return Ok("No paths given.".to_string());
                }
                Ok(results.join("\n"))
            }
            "diff" => {
                let path = self
                    .root()
                    .join(arguments["path"].as_str().unwrap_or_default());
                if !self.p4.in_workspace(&path) {
                    return Err(P4Error(format!(
                        "{} is not in a Perforce workspace",
                        self.p4.relative(&path)
                    )));
                }
                let out = self.diff(&path)?;
                let out = out.trim();
                Ok(if out.is_empty() {
                    "No changes against the depot.".to_string()
                } else {
                    out.to_string()
                })
            }
            other => Err(P4Error(format!("no tool {other}"))),
        }
    }
}

/// The status bar text for a file's record; `None` for a file the depot
/// does not have.
#[must_use]
pub fn describe(record: Option<&Record>) -> String {
    let Some(record) = record else {
        return "p4: not in depot".to_string();
    };
    let action = record.get("action");
    let mut parts = Vec::new();
    match (
        action.map(String::as_str),
        record.get("haveRev"),
        record.get("headRev"),
    ) {
        (Some("add"), _, _) => parts.push("add".to_string()),
        (_, Some(have), Some(head)) => {
            let mut rev = format!("#{have}/{head}");
            if let Some(action) = action {
                rev = format!("{action} {rev}");
            }
            if have != head {
                rev.push_str(" (out of date)");
            }
            parts.push(rev);
        }
        (Some(action), _, _) => parts.push(action.to_string()),
        _ => {}
    }
    if action.is_some() {
        if let Some(change) = record.get("change").filter(|c| *c != "default") {
            parts.push(format!("change {change}"));
        }
    }
    if let Some(locker) = record.get("otherLock0") {
        parts.push(format!("locked by {locker}"));
    } else if let Some(other) = record.get("otherOpen0") {
        let others: usize = record
            .get("otherOpen")
            .and_then(|n| n.parse().ok())
            .unwrap_or(1);
        let more = if others > 1 {
            format!(" +{}", others - 1)
        } else {
            String::new()
        };
        parts.push(format!("also opened by {other}{more}"));
    }
    if record.contains_key("ourLock") {
        parts.push("locked".to_string());
    }
    format!("p4: {}", parts.join(" · "))
}

/// `word` as one word for a shell: as it is when it holds nothing a shell
/// reads specially, else in single quotes.
/// The new changelist form `p4 change -o` gave, with `description` and
/// only `files` (depot paths) of the default changelist.
fn change_spec(form: &str, description: &str, files: &[&str]) -> String {
    let mut out = String::new();
    // Inside a field being written anew: its old lines are dropped.
    let mut replaced = false;
    for line in form.lines() {
        let starts_field = !line.is_empty() && !line.starts_with([' ', '\t', '#']);
        if starts_field {
            replaced = false;
        }
        if replaced {
            continue;
        }
        let lines: Vec<&str> = if line.starts_with("Description:") {
            description.lines().collect()
        } else if line.starts_with("Files:") {
            files.to_vec()
        } else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        out.push_str(line.split_once(':').map_or(line, |(field, _)| field));
        out.push_str(":\n");
        for value in lines {
            out.push('\t');
            out.push_str(value);
            out.push('\n');
        }
        out.push('\n');
        replaced = true;
    }
    out
}

/// The number of the changelist `p4 change -i` said it created.
fn created_change(out: &str) -> Option<String> {
    let mut words = out.split_whitespace();
    (words.next()? == "Change")
        .then(|| words.next())
        .flatten()
        .filter(|n| n.chars().all(|c| c.is_ascii_digit()))
        .filter(|_| words.next() == Some("created"))
        .map(str::to_string)
}

fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_of_the_default_changelist_gets_a_form_of_its_own() {
        let form = "# A Perforce Change Specification.\n#  Change: The change number.\n\n\
                    Change:\tnew\n\nClient:\tws\n\nUser:\tbob\n\nStatus:\tnew\n\n\
                    Description:\n\t<enter description here>\n\n\
                    Files:\n\t//depot/a.c\t# edit\n\t//depot/b.c\t# add\n";
        assert_eq!(
            change_spec(form, "Fix it\nand more", &["//depot/b.c"]),
            "# A Perforce Change Specification.\n#  Change: The change number.\n\n\
             Change:\tnew\n\nClient:\tws\n\nUser:\tbob\n\nStatus:\tnew\n\n\
             Description:\n\tFix it\n\tand more\n\n\
             Files:\n\t//depot/b.c\n\n"
        );
        assert_eq!(
            created_change("Change 42 created with 1 open file(s).\n").as_deref(),
            Some("42")
        );
        assert_eq!(created_change("Change 42 updated."), None);
    }

    fn record(fields: &[(&str, &str)]) -> Record {
        fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_status_says_revisions_actions_and_other_people() {
        assert_eq!(describe(None), "p4: not in depot");
        assert_eq!(
            describe(Some(&record(&[("haveRev", "3"), ("headRev", "5")]))),
            "p4: #3/5 (out of date)"
        );
        assert_eq!(
            describe(Some(&record(&[
                ("haveRev", "1"),
                ("headRev", "1"),
                ("action", "edit"),
                ("change", "12"),
                ("otherOpen0", "bob@ws"),
                ("otherOpen", "3"),
            ]))),
            "p4: edit #1/1 · change 12 · also opened by bob@ws +2"
        );
        assert_eq!(
            describe(Some(&record(&[
                ("action", "add"),
                ("change", "default"),
                ("ourLock", ""),
            ]))),
            "p4: add · locked"
        );
        assert_eq!(
            describe(Some(&record(&[
                ("haveRev", "2"),
                ("headRev", "2"),
                ("otherLock0", "ann@ws"),
                ("otherOpen0", "ann@ws"),
            ]))),
            "p4: #2/2 · locked by ann@ws"
        );
    }

    #[test]
    fn a_shell_word_is_quoted_only_when_it_has_to_be() {
        assert_eq!(shell_word("p4"), "p4");
        assert_eq!(shell_word("/usr/local/bin/p4"), "/usr/local/bin/p4");
        assert_eq!(shell_word("/my tools/p4"), "'/my tools/p4'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
    }
}
