//! The plugin protocol: method names and the shapes of their messages.
//!
//! JSON-RPC 2.0, one message per line on the plugin's stdin and stdout.
//!
//! termide → plugin:
//! - `initialize` (request): [`initialize_params`]; the result is a
//!   [`Contributions`].
//! - `initialized` (notification), once the result is taken.
//! - events (notifications) the plugin subscribed to, such as
//!   `file/saved` `{ "path" }`.
//! - `command/execute` (request) `{ "id" }`: one of its commands was run.
//! - `tool/call` (request) `{ "name", "arguments" }`: the agent called one
//!   of its tools; the result is `{ "text", "isError" }`.
//! - `shutdown` (request), then `exit` (notification), before termide
//!   ends the process.
//!
//! plugin → termide:
//! - `contributions/changed` (notification): a new [`Contributions`].
//! - `window/notify` (request) `{ "message", "level"? }`.
//! - `status/set` (request) `{ "id", "text" }`; an empty text removes it.
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
pub const WINDOW_NOTIFY: &str = "window/notify";
pub const STATUS_SET: &str = "status/set";
pub const PANEL_OPEN: &str = "panel/open";
pub const PANEL_UPDATE: &str = "panel/update";
pub const PANEL_CLOSE: &str = "panel/close";
pub const PANEL_ACTION: &str = "panel/action";
pub const PANEL_CLOSED: &str = "panel/closed";

/// Events a plugin may subscribe to.
pub mod events {
    pub const FILE_OPENED: &str = "file/opened";
    pub const FILE_SAVED: &str = "file/saved";
}

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
    fn nothing_is_an_empty_contribution() {
        assert_eq!(
            Contributions::from_value(&Value::Null),
            Contributions::default()
        );
    }
}
