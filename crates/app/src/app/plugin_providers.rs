//! Plugins as sources of completion, hover and code actions, asked by the
//! language manager beside the servers (see `termide_lsp::extra`).
//!
//! A plugin answers for the files of its own project, in the languages it
//! named. What it sends is checked: labels and titles are cleaned, a hover
//! is cut to length, and a code action's edits apply to the file it was
//! asked about and nothing else — the open buffer, as one undo step when
//! the editor applies them. Each action's title is headed by the plugin's
//! name, so it is clear who suggests it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse, CompletionItem,
    CompletionResponse, Diagnostic, GotoDefinitionResponse, Hover, HoverContents, Location,
    MarkupContent, MarkupKind, Position, Range, WorkspaceEdit,
};
use serde_json::{json, Value};
use termide_lsp::extra::{ExtraProviders, EXTRA_WAIT};
use termide_plugins::protocol::{self, Providers};
use termide_plugins::{Peer, PluginKey};

/// The most completion items one plugin offers.
const MAX_ITEMS: usize = 500;
/// The longest hover, in characters.
const MAX_HOVER: usize = 20_000;
/// The most code actions one plugin offers.
const MAX_ACTIONS: usize = 50;

#[derive(Default)]
pub(crate) struct ProviderHub {
    by_plugin: Mutex<BTreeMap<PluginKey, (Arc<Peer>, Providers)>>,
}

/// The one hub, also the language manager's extra source.
pub(crate) fn provider_hub() -> &'static Arc<ProviderHub> {
    static HUB: OnceLock<Arc<ProviderHub>> = OnceLock::new();
    HUB.get_or_init(|| {
        let hub = Arc::new(ProviderHub::default());
        termide_lsp::extra::set_extra_providers(Arc::clone(&hub) as Arc<dyn ExtraProviders>);
        hub
    })
}

impl ProviderHub {
    /// `key` answers as `providers` say from now on; `None` withdraws it.
    pub(crate) fn set(&self, key: &PluginKey, entry: Option<(Arc<Peer>, Providers)>) {
        let mut map = self
            .by_plugin
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match entry {
            Some(entry) if entry.1 != Providers::default() => {
                map.insert(key.clone(), entry);
            }
            _ => {
                map.remove(key);
            }
        }
    }

    /// The plugins whose project holds `path` and who answer `pick` for `lang`.
    fn answering(
        &self,
        path: &Path,
        lang: &str,
        pick: fn(&Providers) -> &Vec<String>,
    ) -> Vec<(PluginKey, Arc<Peer>)> {
        let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.by_plugin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(key, (_, providers))| {
                (canonical.starts_with(&key.project) || path.starts_with(&key.project))
                    && Providers::covers(pick(providers), lang)
            })
            .map(|(key, (peer, _))| (key.clone(), Arc::clone(peer)))
            .collect()
    }
}

/// Ask `peer` on a thread of its own; the answer, read by `read`, arrives on
/// the channel, or nothing once the wait is over.
fn ask<T: Send + 'static>(
    peer: Arc<Peer>,
    method: &'static str,
    params: Value,
    read: impl FnOnce(Value) -> Option<T> + Send + 'static,
) -> Receiver<Option<T>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let answer = peer
            .request(method, params, EXTRA_WAIT, &|| false)
            .ok()
            .and_then(read);
        let _ = tx.send(answer);
    });
    rx
}

fn at(path: &Path, position: Position) -> Value {
    json!({ "path": path, "line": position.line, "col": position.character })
}

/// Completion items from a plugin's answer `{ "items" }`, LSP-shaped.
fn read_items(answer: Value) -> Option<CompletionResponse> {
    let items: Vec<CompletionItem> = answer["items"]
        .as_array()?
        .iter()
        .take(MAX_ITEMS)
        .filter_map(|item| serde_json::from_value::<CompletionItem>(item.clone()).ok())
        .map(|mut item| {
            item.label = protocol::clean_text(&item.label, 120);
            item.detail = item.detail.map(|d| protocol::clean_text(&d, 200));
            item
        })
        .filter(|item| !item.label.is_empty())
        .collect();
    (!items.is_empty()).then_some(CompletionResponse::Array(items))
}

/// A hover from a plugin's answer `{ "markdown" }`.
fn read_hover(answer: Value) -> Option<Hover> {
    let text: String = answer["markdown"]
        .as_str()?
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(MAX_HOVER)
        .collect();
    if text.trim().is_empty() {
        return None;
    }
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: text,
        }),
        range: None,
    })
}

/// Code actions from a plugin's answer `{ "actions": [{ "title", "edits" }] }`:
/// each edits `path` only, and is titled with the plugin's name.
// `WorkspaceEdit::changes` is keyed by `Uri`, as lsp_types defines it.
#[allow(clippy::mutable_key_type)]
fn read_actions(plugin: &str, path: &Path, answer: &Value) -> Option<CodeActionResponse> {
    let uri = termide_lsp::path_to_uri(path)?;
    let actions: CodeActionResponse = answer["actions"]
        .as_array()?
        .iter()
        .take(MAX_ACTIONS)
        .filter_map(|action| {
            let title = protocol::clean_text(action["title"].as_str()?, 120);
            let edits = super::plugin_editor::text_edits(&action["edits"]).ok()?;
            if title.is_empty() || edits.is_empty() {
                return None;
            }
            let mut changes = std::collections::HashMap::new();
            changes.insert(uri.clone(), edits);
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("{plugin}: {title}"),
                kind: Some(CodeActionKind::QUICKFIX),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                ..Default::default()
            }))
        })
        .collect();
    (!actions.is_empty()).then_some(actions)
}

/// The most places one plugin points at.
const MAX_LOCATIONS: usize = 1000;

/// Places from a plugin's answer `{ "locations": [{ "path", "line", "col",
/// "end_line"?, "end_col"? }] }`; a path must be absolute.
fn read_locations(answer: &Value) -> Option<Vec<Location>> {
    let locations: Vec<Location> = answer["locations"]
        .as_array()?
        .iter()
        .take(MAX_LOCATIONS)
        .filter_map(|place| {
            let path = Path::new(place["path"].as_str()?);
            if !path.is_absolute() {
                return None;
            }
            let number = |field: &str| place[field].as_u64().and_then(|n| u32::try_from(n).ok());
            let start = Position::new(number("line")?, number("col").unwrap_or(0));
            let end = Position::new(
                number("end_line").unwrap_or(start.line),
                number("end_col").unwrap_or(start.character),
            );
            Some(Location::new(
                termide_lsp::path_to_uri(path)?,
                Range::new(start, end),
            ))
        })
        .collect();
    (!locations.is_empty()).then_some(locations)
}

/// What the plugin is told of a diagnostic in a code-action request.
fn diagnostic_param(diagnostic: &Diagnostic) -> Value {
    json!({
        "line": diagnostic.range.start.line,
        "col": diagnostic.range.start.character,
        "end_line": diagnostic.range.end.line,
        "end_col": diagnostic.range.end.character,
        "message": diagnostic.message,
        "source": diagnostic.source,
    })
}

impl ExtraProviders for ProviderHub {
    fn completion(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Vec<Receiver<Option<CompletionResponse>>> {
        self.answering(file_path, lang, |p| &p.completion)
            .into_iter()
            .map(|(_, peer)| {
                ask(
                    peer,
                    protocol::EDITOR_COMPLETION,
                    at(file_path, position),
                    read_items,
                )
            })
            .collect()
    }

    fn hover(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Vec<Receiver<Option<Hover>>> {
        self.answering(file_path, lang, |p| &p.hover)
            .into_iter()
            .map(|(_, peer)| {
                ask(
                    peer,
                    protocol::EDITOR_HOVER,
                    at(file_path, position),
                    read_hover,
                )
            })
            .collect()
    }

    fn code_action(
        &self,
        lang: &str,
        file_path: &Path,
        range: Range,
        diagnostics: &[Diagnostic],
    ) -> Vec<Receiver<Option<CodeActionResponse>>> {
        let params = json!({
            "path": file_path,
            "line": range.start.line,
            "col": range.start.character,
            "end_line": range.end.line,
            "end_col": range.end.character,
            "diagnostics": diagnostics.iter().map(diagnostic_param).collect::<Vec<_>>(),
        });
        self.answering(file_path, lang, |p| &p.code_actions)
            .into_iter()
            .map(|(key, peer)| {
                let path: PathBuf = file_path.to_path_buf();
                ask(
                    peer,
                    protocol::EDITOR_CODE_ACTIONS,
                    params.clone(),
                    move |answer| read_actions(&key.name, &path, &answer),
                )
            })
            .collect()
    }

    fn definition(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Vec<Receiver<Option<GotoDefinitionResponse>>> {
        self.answering(file_path, lang, |p| &p.definition)
            .into_iter()
            .map(|(_, peer)| {
                ask(
                    peer,
                    protocol::EDITOR_DEFINITION,
                    at(file_path, position),
                    |answer| read_locations(&answer).map(GotoDefinitionResponse::Array),
                )
            })
            .collect()
    }

    fn references(
        &self,
        lang: &str,
        file_path: &Path,
        position: Position,
    ) -> Vec<Receiver<Option<Vec<Location>>>> {
        self.answering(file_path, lang, |p| &p.references)
            .into_iter()
            .map(|(_, peer)| {
                ask(
                    peer,
                    protocol::EDITOR_REFERENCES,
                    at(file_path, position),
                    |answer| read_locations(&answer),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn a_plugins_answers_are_checked() {
        let items = read_items(json!({ "items": [
            { "label": "ok\u{202E}", "insertText": "ok()" },
            { "label": "" },
            { "nolabel": 1 },
        ] }))
        .unwrap();
        let CompletionResponse::Array(items) = items else {
            panic!()
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "ok");

        let hover = read_hover(json!({ "markdown": "**x**\u{1b}[31m" })).unwrap();
        let HoverContents::Markup(m) = hover.contents else {
            panic!()
        };
        assert_eq!(m.value, "**x**[31m");

        let path = Path::new("/p/src/main.rs");
        let actions = read_actions(
            "lint",
            path,
            &json!({ "actions": [
                { "title": "Fix it", "edits": [{ "line": 1, "col": 0, "text": "x" }] },
                { "title": "Nothing", "edits": [] },
            ] }),
        )
        .unwrap();
        assert_eq!(actions.len(), 1);
        let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
            panic!()
        };
        assert_eq!(action.title, "lint: Fix it");
        let changes = action.edit.as_ref().unwrap().changes.as_ref().unwrap();
        assert_eq!(changes.len(), 1, "the file asked about and no other");

        let places = read_locations(&json!({ "locations": [
            { "path": "/p/lib.rs", "line": 3, "col": 4 },
            { "path": "relative.rs", "line": 1 },
        ] }))
        .unwrap();
        assert_eq!(places.len(), 1, "a relative path is no place");
        assert_eq!(places[0].range.start, Position::new(3, 4));
    }
}
