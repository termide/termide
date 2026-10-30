//! What plugins say of files on behalf of another version control system:
//! `files/status`, coloured in the file manager, and `editor/base`, the text
//! the editor marks changed lines against. Git's own word comes first in
//! both; the panels decide that, this module only keeps and delivers.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use termide_core::Panel;
use termide_git::GitStatus;
use termide_plugins::rpc::{RpcError, INVALID_PARAMS};
use termide_plugins::PluginKey;

use super::App;
use crate::PanelExt;

/// The most paths one `files/status` may name.
const MAX_ITEMS: usize = 100_000;
/// The longest base text a plugin may give, in bytes.
const MAX_BASE: usize = 16 * 1024 * 1024;

/// The statuses and bases each running plugin set.
#[derive(Default)]
pub(crate) struct PluginFiles {
    status: BTreeMap<PluginKey, HashMap<PathBuf, GitStatus>>,
    bases: BTreeMap<PluginKey, BTreeMap<PathBuf, String>>,
    /// The current project's statuses, all its plugins together, as the
    /// file managers were last given them.
    shown: Arc<HashMap<PathBuf, GitStatus>>,
    /// The project `shown` is of.
    shown_for: PathBuf,
}

/// A status as the protocol names it; `None` for `null`.
fn parse_status(value: &Value) -> Result<Option<GitStatus>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(name) => match name.as_str() {
            "modified" => Ok(Some(GitStatus::Modified)),
            "added" => Ok(Some(GitStatus::Added)),
            "deleted" => Ok(Some(GitStatus::Deleted)),
            "ignored" => Ok(Some(GitStatus::Ignored)),
            other => Err(format!("no status {other:?}")),
        },
        _ => Err("`status` must be a string or null".to_string()),
    }
}

fn absolute(value: &Value) -> Result<PathBuf, RpcError> {
    value
        .as_str()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`path` must be an absolute path"))
}

impl App {
    /// `files/status` from `key`.
    pub(super) fn set_plugin_file_status(
        &mut self,
        key: &PluginKey,
        params: &Value,
    ) -> Result<(), RpcError> {
        let items = params["items"]
            .as_array()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`items` must be a list"))?;
        if items.len() > MAX_ITEMS {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("at most {MAX_ITEMS} items at a time"),
            ));
        }
        let mut parsed = Vec::with_capacity(items.len());
        for item in items {
            let path = absolute(&item["path"])?;
            let status =
                parse_status(&item["status"]).map_err(|e| RpcError::new(INVALID_PARAMS, e))?;
            parsed.push((path, status));
        }
        let by_path = self.plugins.files.status.entry(key.clone()).or_default();
        for (path, status) in parsed {
            match status {
                Some(status) => by_path.insert(path, status),
                None => by_path.remove(&path),
            };
        }
        if by_path.is_empty() {
            self.plugins.files.status.remove(key);
        }
        self.show_plugin_file_statuses();
        Ok(())
    }

    /// `editor/base` from `key`.
    pub(super) fn set_plugin_base(
        &mut self,
        key: &PluginKey,
        params: &Value,
    ) -> Result<(), RpcError> {
        let path = absolute(&params["path"])?;
        let text = match &params["text"] {
            Value::Null => None,
            Value::String(text) if text.len() <= MAX_BASE => Some(text.clone()),
            Value::String(_) => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("`text` may be at most {MAX_BASE} bytes"),
                ))
            }
            _ => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "`text` must be a string or null",
                ))
            }
        };
        let bases = self.plugins.files.bases.entry(key.clone()).or_default();
        match text {
            Some(text) => {
                bases.insert(path.clone(), text);
            }
            None => {
                bases.remove(&path);
            }
        }
        if bases.is_empty() {
            self.plugins.files.bases.remove(key);
        }
        self.show_plugin_base(&path);
        Ok(())
    }

    /// The plugin `key` is gone: what it said of the files goes with it.
    pub(super) fn forget_plugin_files(&mut self, key: &PluginKey) {
        let had_status = self.plugins.files.status.remove(key).is_some();
        if let Some(bases) = self.plugins.files.bases.remove(key) {
            for path in bases.keys() {
                self.show_plugin_base(path);
            }
        }
        if had_status {
            self.show_plugin_file_statuses();
        }
    }

    /// The base a plugin gave for `path`: the first plugin's, of the current
    /// project's before the others'.
    fn plugin_base(&self, path: &Path) -> Option<String> {
        let current = self.open_projects.current();
        let mut found = None;
        for (key, bases) in &self.plugins.files.bases {
            if let Some(text) = bases.get(path) {
                if key.project == current {
                    return Some(text.clone());
                }
                found.get_or_insert_with(|| text.clone());
            }
        }
        found
    }

    /// Give `path`'s editors the base the plugins hold for it now.
    fn show_plugin_base(&mut self, path: &Path) {
        let base = self.plugin_base(path);
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                if editor.file_path() == Some(path) {
                    editor.set_base_text(base.clone());
                    self.state.needs_redraw = true;
                }
            }
        }
    }

    /// Give the file managers the current project's statuses.
    pub(super) fn show_plugin_file_statuses(&mut self) {
        let current = self.open_projects.current();
        let mut merged: HashMap<PathBuf, GitStatus> = HashMap::new();
        for (key, by_path) in &self.plugins.files.status {
            if key.project != current {
                continue;
            }
            for (path, status) in by_path {
                merged.entry(path.clone()).or_insert(*status);
            }
        }
        if merged == *self.plugins.files.shown && self.plugins.files.shown_for == current {
            return;
        }
        self.plugins.files.shown_for = current.to_path_buf();
        self.plugins.files.shown = Arc::new(merged);
        let shown = Arc::clone(&self.plugins.files.shown);
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                fm.set_plugin_statuses(Arc::clone(&shown));
            }
        }
        self.state.needs_redraw = true;
    }

    /// Bring a panel about to be shown up to date with what the plugins
    /// said: a file manager gets the statuses, an editor its file's base.
    pub(super) fn apply_plugin_files(&self, panel: &mut Box<dyn Panel>) {
        if let Some(fm) = panel.as_file_manager_mut() {
            let current = self.open_projects.current();
            if !self.plugins.files.shown.is_empty() && self.plugins.files.shown_for == current {
                fm.set_plugin_statuses(Arc::clone(&self.plugins.files.shown));
            }
            return;
        }
        if self.plugins.files.bases.is_empty() {
            return;
        }
        if let Some(editor) = panel.as_editor_mut() {
            let base = editor.file_path().and_then(|path| self.plugin_base(path));
            if base.is_some() {
                editor.set_base_text(base);
            }
        }
    }

    /// Every panel of the layout brought up to date: after a project comes
    /// back from the background, whose panels missed what was said meanwhile.
    pub(super) fn reapply_plugin_files(&mut self) {
        // The current project changed: its statuses, not the last one's.
        self.show_plugin_file_statuses();
        let shown = Arc::clone(&self.plugins.files.shown);
        let paths: Vec<PathBuf> = self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_editor_mut()?.file_path().map(Path::to_path_buf))
            .collect();
        let bases: Vec<(PathBuf, Option<String>)> = paths
            .into_iter()
            .map(|path| {
                let base = self.plugin_base(&path);
                (path, base)
            })
            .collect();
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(fm) = panel.as_file_manager_mut() {
                fm.set_plugin_statuses(Arc::clone(&shown));
            } else if let Some(editor) = panel.as_editor_mut() {
                let base = editor
                    .file_path()
                    .and_then(|path| bases.iter().find(|(p, _)| p == path))
                    .and_then(|(_, base)| base.clone());
                if base.is_some() {
                    editor.set_base_text(base);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn statuses_are_git_s_names_or_null() {
        assert_eq!(
            parse_status(&json!("modified")),
            Ok(Some(GitStatus::Modified))
        );
        assert_eq!(
            parse_status(&json!("ignored")),
            Ok(Some(GitStatus::Ignored))
        );
        assert_eq!(parse_status(&Value::Null), Ok(None));
        assert!(parse_status(&json!("unmodified")).is_err());
        assert!(parse_status(&json!(1)).is_err());
    }
}
