//! The plugin protocol: method names and the shapes of their messages.
//!
//! JSON-RPC 2.0, one message per line on the plugin's stdin and stdout.
//!
//! termide → plugin:
//! - `initialize` (request): [`initialize_params`]; the result is a
//!   [`Contributions`].
//! - `initialized` (notification), once the result is taken.
//! - events (notifications) the plugin subscribed to, of its own project:
//!   `file/opened`, `file/saved`, `file/closed` `{ "path" }`;
//!   `editor/focused` `{ "path" }`; `editor/changed` `{ "path", "version" }`
//!   once the typing pauses; `git/changed` `{ "root" }`.
//! - `$/cancelRequest` (notification) `{ "id" }`: termide gave up on that
//!   request.
//! - `command/execute` (request) `{ "id" }`: one of its commands was run.
//! - `tool/call` (request) `{ "name", "arguments", "progress" }`: the agent
//!   called one of its tools; the result is `{ "text", "isError" }`. Until
//!   then the plugin may send `tool/progress` `{ "token": progress, "text" }`
//!   (notification), the output so far.
//! - `shutdown` (request), then `exit` (notification), before termide
//!   ends the process.
//!
//! plugin → termide:
//! - `contributions/changed` (notification): a new [`Contributions`].
//! - `window/notify` (request) `{ "message", "level"? }`.
//! - `status/set` (request) `{ "id", "text" }`; an empty text removes it.
//! - `window/confirm` `{ "title", "message" }` → `true`, `false` or `null`
//!   (dismissed); `window/choose` `{ "title", "items" }` → an index or
//!   `null`; `window/input` `{ "title", "prompt", "default"? }` → text or
//!   `null`; `window/form` `{ "title", "fields" }` → `{ name: value }` or
//!   `null`; `window/message` `{ "title", "message" }` → `null`. Requests,
//!   answered once the user closes the window; one window per plugin at a
//!   time, shown while its project is current.
//! - `editor/active` → `{ "path", "line", "col", "selection", "modified",
//!   "version" }` or `null`; `editor/read` `{ "path" }` → `{ "text",
//!   "version", "modified" }` of an open buffer; `editor/edit` `{ "path",
//!   "edits", "version"? }` → `{ "version" }`, applied to the open buffer as
//!   one undo step; `editor/open` `{ "path", "line"?, "col"? }`.
//! - `terminal/run` `{ "command", "cwd"? }` opens a terminal panel running
//!   it, in the current project.
//! - `editor/diagnostics` (request) `{ "path", "items" }`: the plugin's marks
//!   on the file at `path`, replacing the ones it set before (an empty list
//!   clears them). Each item is `{ "line", "col"?, "end_line"?, "end_col"?,
//!   "severity"?, "message", "code"? }`, lines from 0, columns as LSP counts
//!   them; without `col` the whole line is marked.
//! - `panel/open` (request) `{ "title", "markdown" }` opens a panel in the
//!   plugin's project, which must be the current one; the result is
//!   `{ "panel" }`, its id.
//! - `panel/update` (request) `{ "panel", "markdown", "title"? }`.
//! - `panel/close` (request) `{ "panel" }`.
//!
//! About its panels, termide → plugin (notifications):
//! - `panel/action` `{ "panel", "action" }`: a `plugin:<action>` link in
//!   the panel was followed.
//! - `panel/closed` `{ "panel" }`: the user closed it.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::host::{Launch, PluginKey};

/// The protocol revision termide speaks.
pub const PROTOCOL_VERSION: u32 = 1;

pub const INITIALIZE: &str = "initialize";
pub const INITIALIZED: &str = "initialized";
pub const SHUTDOWN: &str = "shutdown";
pub const EXIT: &str = "exit";
pub const CONTRIBUTIONS_CHANGED: &str = "contributions/changed";
pub const COMMAND_EXECUTE: &str = "command/execute";
pub const TOOL_CALL: &str = "tool/call";
pub const TOOL_PROGRESS: &str = "tool/progress";
pub const WINDOW_NOTIFY: &str = "window/notify";
pub const STATUS_SET: &str = "status/set";
pub const WINDOW_CONFIRM: &str = "window/confirm";
pub const WINDOW_CHOOSE: &str = "window/choose";
pub const WINDOW_INPUT: &str = "window/input";
pub const WINDOW_FORM: &str = "window/form";
pub const WINDOW_MESSAGE: &str = "window/message";
pub const EDITOR_DIAGNOSTICS: &str = "editor/diagnostics";
pub const EDITOR_ACTIVE: &str = "editor/active";
pub const EDITOR_READ: &str = "editor/read";
pub const EDITOR_EDIT: &str = "editor/edit";
pub const EDITOR_OPEN: &str = "editor/open";
pub const EDITOR_COMPLETION: &str = "editor/completion";
pub const EDITOR_HOVER: &str = "editor/hover";
pub const EDITOR_CODE_ACTIONS: &str = "editor/code_actions";
pub const EDITOR_DEFINITION: &str = "editor/definition";
pub const EDITOR_REFERENCES: &str = "editor/references";
pub const TERMINAL_RUN: &str = "terminal/run";
pub const PANEL_OPEN: &str = "panel/open";
pub const PANEL_UPDATE: &str = "panel/update";
pub const PANEL_CLOSE: &str = "panel/close";
pub const PANEL_ACTION: &str = "panel/action";
pub const PANEL_CLOSED: &str = "panel/closed";

/// Events a plugin may subscribe to.
pub mod events {
    pub const FILE_OPENED: &str = "file/opened";
    pub const FILE_SAVED: &str = "file/saved";
    pub const FILE_CLOSED: &str = "file/closed";
    pub const EDITOR_FOCUSED: &str = "editor/focused";
    pub const EDITOR_CHANGED: &str = "editor/changed";
    pub const GIT_CHANGED: &str = "git/changed";
    pub const AGENT_STARTED: &str = "agent/started";
    pub const AGENT_FINISHED: &str = "agent/finished";
    /// A request, not a notification: `{ "path", "version" }` before `Ctrl+S`
    /// writes the file; the answer is `{ "edits" }` or `null`.
    pub const FILE_WILL_SAVE: &str = "file/will_save";
}

/// `text` from a plugin made fit to stand in termide's own chrome — a panel
/// title, a menu entry, a window's heading: control and invisible
/// characters (zero-width ones, direction overrides) removed, runs of
/// whitespace made one space, and cut to `max` characters with `…`.
#[must_use]
pub fn clean_text(text: &str, max: usize) -> String {
    let invisible = |c: char| {
        c.is_control()
            || matches!(
                c,
                '\u{200B}'..='\u{200F}'
                    | '\u{202A}'..='\u{202E}'
                    | '\u{2060}'..='\u{2064}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{FEFF}'
            )
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    let joined = words.join(" ");
    let kept: String = joined.chars().filter(|c| !invisible(*c)).collect();
    if kept.chars().count() <= max {
        kept
    } else {
        let mut cut: String = kept.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// The entries of a `menu` param, `[{ "label", "action" }]`: at most
/// [`MAX_MENU_ITEMS`], labels cleaned, the malformed ones left out.
#[must_use]
pub fn menu_items(value: &Value) -> Vec<(String, String)> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let label = clean_text(item["label"].as_str()?, MAX_LABEL);
            let action = item["action"].as_str()?.to_string();
            (!label.is_empty() && !action.is_empty()).then_some((label, action))
        })
        .take(MAX_MENU_ITEMS)
        .collect()
}

/// The most entries a plugin's menu holds.
pub const MAX_MENU_ITEMS: usize = 12;
/// The longest a plugin's title may be, in characters.
pub const MAX_TITLE: usize = 60;
/// The longest a plugin's menu label may be, in characters.
pub const MAX_LABEL: usize = 40;

/// The params of `initialize`.
#[must_use]
pub fn initialize_params(key: &PluginKey, launch: &Launch) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "termide": { "version": env!("CARGO_PKG_VERSION") },
        "plugin": { "name": key.name, "dir": launch.dir },
        "project": { "root": key.project },
        "settings": launch.settings,
    })
}

/// A command for the palette and menu.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CommandInfo {
    pub id: String,
    pub title: String,
    /// A key that runs it, as `config.toml` writes them (`Ctrl+Alt+R`);
    /// taken only if termide binds it nowhere and it is unlikely to be a
    /// program's in the terminal.
    #[serde(default)]
    pub key: Option<String>,
}

/// A tool for the agent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_schema")]
    pub input_schema: Value,
    /// The tool changes nothing.
    #[serde(default)]
    pub read_only: bool,
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// The languages a plugin answers for in the editor, by LSP language id
/// (`rust`, `python`, …); `*` is every one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Providers {
    #[serde(default)]
    pub completion: Vec<String>,
    #[serde(default)]
    pub hover: Vec<String>,
    #[serde(default)]
    pub code_actions: Vec<String>,
    #[serde(default)]
    pub definition: Vec<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

impl Providers {
    /// Whether `langs` covers `lang`.
    #[must_use]
    pub fn covers(langs: &[String], lang: &str) -> bool {
        langs.iter().any(|l| l == "*" || l == lang)
    }
}

/// What a plugin adds to termide.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Contributions {
    /// The events it wants.
    #[serde(default)]
    pub subscriptions: Vec<String>,
    #[serde(default)]
    pub commands: Vec<CommandInfo>,
    #[serde(default)]
    pub tools: Vec<ToolInfo>,
    #[serde(default)]
    pub providers: Providers,
}

impl Contributions {
    /// Read from a message, keeping each entry that is well formed and
    /// dropping the rest, so one bad entry does not cost the others.
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        fn each<T: for<'de> Deserialize<'de>>(value: &Value, field: &str) -> Vec<T> {
            value[field]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                .collect()
        }
        Self {
            subscriptions: each(value, "subscriptions"),
            commands: each(value, "commands"),
            tools: each(value, "tools"),
            providers: serde_json::from_value(value["providers"].clone()).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contributions_keep_good_entries() {
        let value = json!({
            "subscriptions": ["file/saved", 3],
            "commands": [{ "id": "a", "title": "A" }, { "id": "no-title" }],
            "tools": [{ "name": "t", "readOnly": true }, { "description": "nameless" }],
        });
        let c = Contributions::from_value(&value);
        assert_eq!(c.subscriptions, ["file/saved"]);
        assert_eq!(c.commands.len(), 1);
        assert_eq!(c.tools.len(), 1);
        assert!(c.tools[0].read_only);
        assert_eq!(c.tools[0].input_schema["type"], "object");
    }

    #[test]
    fn text_from_a_plugin_is_cleaned_for_termides_chrome() {
        assert_eq!(clean_text("  Report\n\tdaily ", 60), "Report daily");
        assert_eq!(clean_text("a\u{202E}b\u{200B}c\u{1b}[31m", 60), "abc[31m");
        assert_eq!(clean_text("abcdef", 4), "abc…");
        let menu = menu_items(&json!([
            { "label": "Refresh", "action": "refresh" },
            { "label": "", "action": "x" },
            { "action": "y" },
        ]));
        assert_eq!(menu, [("Refresh".to_string(), "refresh".to_string())]);
    }

    #[test]
    fn nothing_is_an_empty_contribution() {
        assert_eq!(
            Contributions::from_value(&Value::Null),
            Contributions::default()
        );
    }
}
