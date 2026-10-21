//! Command registry loaded exclusively from `commands.toml`.
//!
//! Each TOML section key is a command identifier. The `command` field is the
//! shell command to run. Optional metadata: `name` (display name),
//! `mode`, `key` (hotkey), `group`, `params`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use super::get_config_dir;

/// Command execution mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandMode {
    /// Run in a new terminal panel (default).
    #[default]
    Terminal,
    /// Run silently in the background.
    Background,
    /// Run in background and show output in a scrollable modal on completion.
    Report,
}

impl CommandMode {
    /// The spelling `commands.toml` uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Background => "background",
            Self::Report => "report",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "terminal" => Some(Self::Terminal),
            "background" => Some(Self::Background),
            "report" => Some(Self::Report),
            _ => None,
        }
    }
}

/// Where a command runs: the directory it starts in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandCwd {
    /// The working directory of the focused panel, or the project root when the
    /// panel has none (default): a command follows where the user is looking.
    #[default]
    Panel,
    /// The project root, whatever panel is focused: the static point a command's
    /// relative paths are written against — the folder its `commands.toml` sits in.
    Project,
}

impl CommandCwd {
    /// The spelling `commands.toml` uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Panel => "panel",
            Self::Project => "project",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "panel" => Some(Self::Panel),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

/// Parameter type for command launch forms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandParamType {
    Text,
    Number,
    Bool,
    Select,
}

impl CommandParamType {
    /// The spelling `commands.toml` uses.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::Select => "select",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "number" => Some(Self::Number),
            "bool" => Some(Self::Bool),
            "select" => Some(Self::Select),
            _ => None,
        }
    }
}

/// A single parameter definition from commands.toml.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandParam {
    /// Variable name (used as TERMIDE_PARAM_<NAME> env var).
    pub name: String,
    /// Human-readable label shown in the form.
    #[serde(default)]
    pub label: String,
    /// Type of input widget to render.
    #[serde(rename = "type")]
    pub param_type: CommandParamType,
    /// For Select type: list of options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Default value as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Metadata for a single command, parsed from a [command_name] section in commands.toml.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandMetadata {
    /// Inline shell command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Override display name (if set in TOML).
    #[serde(rename = "name", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Execution mode override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<CommandMode>,
    /// Where the command runs; `panel` (the default) is the focused panel's
    /// directory, `project` the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<CommandCwd>,
    /// Hotkey binding string like "Ctrl+Shift+D".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Parameter definitions for the launch form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<CommandParam>,
    /// Group (virtual grouping in menu).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Debugger the command runs under (`lldb`, `gdb`, `dlv`, `pdb`); the
    /// command line is then the program to debug and its arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debugger: Option<String>,
    /// Flags of the debugger itself, each its own word, added to those
    /// termide starts it with: `["--only-same-user=false"]` for Delve.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub debugger_args: Vec<String>,
    /// Shell command run before the debugger starts, which it must pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// With a debug adapter: `terminal` (the default) runs the program in a
    /// terminal panel, where it reads the keyboard; `panel` shows its output
    /// in the debug panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub console: Option<String>,
}

/// Parsed contents of a commands.toml file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandsMetadata {
    /// Map from command name to its metadata.
    pub entries: HashMap<String, CommandMetadata>,
}

impl CommandsMetadata {
    /// Load metadata from `commands.toml` in the given directory.
    ///
    /// For `config_dir = ~/.config/termide/`, reads
    /// `~/.config/termide/commands.toml`.
    ///
    /// Returns empty metadata if the file does not exist.
    pub fn load(config_dir: &Path) -> Self {
        let toml_path = config_dir.join("commands.toml");
        if !toml_path.exists() {
            return Self::default();
        }
        let content = match std::fs::read_to_string(&toml_path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Failed to read {}: {}", toml_path.display(), e);
                return Self::default();
            }
        };
        Self::parse(&content)
    }

    fn parse(content: &str) -> Self {
        let root: toml::Value = match content.parse() {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Failed to parse commands.toml: {}", e);
                return Self::default();
            }
        };

        let table = match root.as_table() {
            Some(t) => t,
            None => return Self::default(),
        };

        let mut entries = HashMap::new();

        for (section_name, value) in table {
            let section = match value.as_table() {
                Some(t) => t,
                None => continue,
            };

            let command = section
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let display_name = section
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let mode = section
                .get("mode")
                .and_then(|v| v.as_str())
                .and_then(CommandMode::from_str);

            let key = section
                .get("key")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let params = Self::parse_params(section);

            let group = section
                .get("group")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let text = |key: &str| {
                section
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let debugger = text("debugger");
            let debugger_args = Self::parse_words(section, "debugger_args", section_name);
            let build = text("build");
            let console = text("console");
            let cwd = Self::parse_cwd(section, section_name);

            entries.insert(
                section_name.clone(),
                CommandMetadata {
                    command,
                    display_name,
                    mode,
                    cwd,
                    key,
                    params,
                    group,
                    debugger,
                    debugger_args,
                    build,
                    console,
                },
            );
        }

        Self { entries }
    }

    /// `cwd` of a section; an unknown value is reported and left out, so a typo
    /// runs the command where the user did not mean it to.
    fn parse_cwd(
        section: &toml::map::Map<String, toml::Value>,
        command: &str,
    ) -> Option<CommandCwd> {
        let raw = section.get("cwd").and_then(|v| v.as_str()).map(str::trim)?;
        if raw.is_empty() {
            return None;
        }
        CommandCwd::from_str(raw).or_else(|| {
            log::warn!(
                "commands.toml: `cwd` of `{command}` must be \"panel\" or \"project\", \
                 not {raw:?}; the focused panel's directory is used"
            );
            None
        })
    }

    /// An array of strings under `key`; anything else is reported and left
    /// out, so a typo cannot pass a stray flag.
    fn parse_words(
        section: &toml::map::Map<String, toml::Value>,
        key: &str,
        command: &str,
    ) -> Vec<String> {
        let Some(value) = section.get(key) else {
            return Vec::new();
        };
        let words = value.as_array().and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        });
        words.unwrap_or_else(|| {
            log::warn!(
                "commands.toml: `{key}` of `{command}` must be an array of strings, \
                 such as [\"--flag\", \"value\"]; ignored"
            );
            Vec::new()
        })
    }

    fn parse_params(section: &toml::map::Map<String, toml::Value>) -> Vec<CommandParam> {
        let params_array = match section.get("params").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => return Vec::new(),
        };

        let mut params = Vec::new();
        for param_value in params_array {
            let table = match param_value.as_table() {
                Some(t) => t,
                None => continue,
            };

            let name = match table.get("name").and_then(|v| v.as_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            let label = table
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or(&name)
                .to_string();

            let param_type = table
                .get("type")
                .and_then(|v| v.as_str())
                .and_then(CommandParamType::from_str)
                .unwrap_or(CommandParamType::Text);

            let options = table
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let default = table.get("default").map(|v| match v {
                toml::Value::String(s) => s.clone(),
                toml::Value::Boolean(b) => b.to_string(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Float(f) => f.to_string(),
                _ => String::new(),
            });

            params.push(CommandParam {
                name,
                label,
                param_type,
                options,
                default,
            });
        }

        params
    }

    /// Save metadata to `commands.toml` in the given directory.
    ///
    /// The file is edited in place rather than rewritten: commands keep
    /// their order and the comments around them, a changed command has only
    /// its changed fields rewritten, and new commands go at the end. A file
    /// that does not parse is replaced.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        let toml_path = config_dir.join("commands.toml");
        if let Some(parent) = toml_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let original = std::fs::read_to_string(&toml_path).unwrap_or_default();
        std::fs::write(&toml_path, self.render_over(&original))
    }

    /// The text of `original` edited to hold exactly these entries.
    fn render_over(&self, original: &str) -> String {
        use toml_edit::{DocumentMut, Item};

        let mut doc: DocumentMut = original.parse().unwrap_or_default();
        let before = Self::parse(original).entries;

        let gone: Vec<String> = doc
            .iter()
            .filter(|(key, item)| item.is_table() && !self.entries.contains_key(*key))
            .map(|(key, _)| key.to_string())
            .collect();
        for key in gone {
            doc.remove(&key);
        }

        let mut added: Vec<&String> = Vec::new();
        for (name, meta) in &self.entries {
            match doc.get_mut(name).and_then(Item::as_table_mut) {
                Some(table) => {
                    if before.get(name) != Some(meta) {
                        write_fields(table, meta, before.get(name).map(|m| &m.params));
                    }
                }
                None => added.push(name),
            }
        }
        added.sort();
        for name in added {
            let mut table = toml_edit::Table::new();
            write_fields(&mut table, &self.entries[name], None);
            doc.insert(name, Item::Table(table));
        }
        doc.to_string()
    }

    /// `base`, or `base-2`, `base-3`… — the first identifier no entry has.
    pub fn free_name(&self, base: &str) -> String {
        if !self.entries.contains_key(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|name| !self.entries.contains_key(name))
            .expect("an unbounded range has a free name")
    }
}

/// Write `meta` into `table`, touching only the fields that differ, so that
/// the comments and layout of the rest survive. `old_params` is what the
/// table held; the parameter tables are rewritten only when they changed.
fn write_fields(
    table: &mut toml_edit::Table,
    meta: &CommandMetadata,
    old_params: Option<&Vec<CommandParam>>,
) {
    fn set(table: &mut toml_edit::Table, key: &str, value: Option<&str>) {
        match value {
            Some(v) if table.get(key).and_then(toml_edit::Item::as_str) == Some(v) => {}
            Some(v) => {
                table.insert(key, toml_edit::value(v));
            }
            None => {
                table.remove(key);
            }
        }
    }
    set(table, "name", meta.display_name.as_deref());
    set(table, "command", meta.command.as_deref());
    set(table, "mode", meta.mode.map(CommandMode::as_str));
    set(table, "cwd", meta.cwd.map(CommandCwd::as_str));
    set(table, "group", meta.group.as_deref());
    set(table, "key", meta.key.as_deref());
    set(table, "debugger", meta.debugger.as_deref());
    let old_args: Vec<&str> = table
        .get("debugger_args")
        .and_then(toml_edit::Item::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    if old_args
        != meta
            .debugger_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        if meta.debugger_args.is_empty() {
            table.remove("debugger_args");
        } else {
            let args: toml_edit::Array = meta.debugger_args.iter().map(String::as_str).collect();
            table.insert("debugger_args", toml_edit::value(args));
        }
    }
    set(table, "build", meta.build.as_deref());
    set(table, "console", meta.console.as_deref());

    if old_params == Some(&meta.params) {
        return;
    }
    if meta.params.is_empty() {
        table.remove("params");
        return;
    }
    let mut params = toml_edit::ArrayOfTables::new();
    for param in &meta.params {
        let mut t = toml_edit::Table::new();
        t.insert("name", toml_edit::value(&param.name));
        if param.label != param.name {
            t.insert("label", toml_edit::value(&param.label));
        }
        t.insert("type", toml_edit::value(param.param_type.as_str()));
        if !param.options.is_empty() {
            let options: toml_edit::Array = param.options.iter().map(String::as_str).collect();
            t.insert("options", toml_edit::value(options));
        }
        if let Some(default) = &param.default {
            t.insert("default", toml_edit::value(default));
        }
        params.push(t);
    }
    table.insert("params", toml_edit::Item::ArrayOfTables(params));
}

/// A single command item from commands.toml.
#[derive(Debug, Clone)]
pub struct CommandItem {
    /// Identifier (TOML section key).
    pub name: String,
    /// Inline shell command.
    pub command: Option<String>,
    /// Execution mode.
    pub mode: CommandMode,
    /// Where this command runs; see [`CommandCwd`]. `cwd` in `commands.toml`,
    /// the focused panel's directory when unset.
    pub cwd: CommandCwd,
    /// Whether this command comes from a project-local `.termide/` directory.
    pub is_project: bool,
    /// Metadata from commands.toml.
    pub metadata: Option<CommandMetadata>,
}

impl CommandItem {
    /// The debugger this command runs under, as written in `commands.toml`.
    pub fn debugger(&self) -> Option<&str> {
        self.metadata.as_ref()?.debugger.as_deref()
    }

    /// The debugger's own flags, as written in `commands.toml`.
    pub fn debugger_args(&self) -> &[String] {
        self.metadata
            .as_ref()
            .map(|m| m.debugger_args.as_slice())
            .unwrap_or_default()
    }
}

/// A group of commands (virtual grouping via `group` field in TOML).
#[derive(Debug, Clone)]
pub struct CommandGroup {
    /// Group name.
    pub name: String,
    /// Commands in this group.
    pub items: Vec<CommandItem>,
    /// Whether this group comes from a project-local `.termide/` directory.
    pub is_project: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandMenuKeyKind {
    Command,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedCommandMenuKey {
    pub kind: CommandMenuKeyKind,
    pub is_project: bool,
    pub name: String,
}

pub fn encode_command_menu_key(kind: CommandMenuKeyKind, name: &str, is_project: bool) -> String {
    let kind = match kind {
        CommandMenuKeyKind::Command => "cmd",
        CommandMenuKeyKind::Group => "grp",
    };
    let scope = if is_project { "p" } else { "g" };
    format!("{kind}:{scope}:{name}")
}

pub fn decode_command_menu_key(key: &str) -> Option<DecodedCommandMenuKey> {
    let mut parts = key.splitn(3, ':');
    let kind = match parts.next()? {
        "cmd" => CommandMenuKeyKind::Command,
        "grp" => CommandMenuKeyKind::Group,
        _ => return None,
    };
    let is_project = match parts.next()? {
        "p" => true,
        "g" => false,
        _ => return None,
    };
    let name = parts.next()?.to_string();
    Some(DecodedCommandMenuKey {
        kind,
        is_project,
        name,
    })
}

/// Registry of all available commands.
#[derive(Debug, Clone, Default)]
pub struct CommandsRegistry {
    /// Commands without a group.
    pub root_items: Vec<CommandItem>,
    /// Command groups.
    pub groups: Vec<CommandGroup>,
}

impl CommandsRegistry {
    /// Find a root-level command by name and source.
    pub fn find_root_command(&self, name: &str, is_project: bool) -> Option<&CommandItem> {
        self.root_items
            .iter()
            .find(|s| s.name == name && s.is_project == is_project)
    }

    /// Find a command by name across root items and all groups, scoped by source.
    pub fn find_command_anywhere_scoped(
        &self,
        name: &str,
        is_project: bool,
    ) -> Option<&CommandItem> {
        self.root_items
            .iter()
            .find(|s| s.name == name && s.is_project == is_project)
            .or_else(|| {
                self.groups
                    .iter()
                    .filter(|g| g.is_project == is_project)
                    .flat_map(|g| g.items.iter())
                    .find(|s| s.name == name && s.is_project == is_project)
            })
    }

    /// Find a command group by name and source.
    pub fn find_group(&self, name: &str, is_project: bool) -> Option<&CommandGroup> {
        self.groups
            .iter()
            .find(|g| g.name == name && g.is_project == is_project)
    }

    /// Collect all commands that have hotkey bindings defined in metadata.
    pub fn commands_with_hotkeys(&self) -> Vec<(&CommandItem, &str)> {
        let mut result = Vec::new();
        for item in &self.root_items {
            if let Some(ref meta) = item.metadata {
                if let Some(ref key) = meta.key {
                    result.push((item, key.as_str()));
                }
            }
        }
        for group in &self.groups {
            for item in &group.items {
                if let Some(ref meta) = item.metadata {
                    if let Some(ref key) = meta.key {
                        result.push((item, key.as_str()));
                    }
                }
            }
        }
        result
    }

    /// Every command that runs under a debugger: project commands first,
    /// commands without a group before grouped ones.
    pub fn debug_commands(&self) -> Vec<&CommandItem> {
        let mut out: Vec<&CommandItem> = self
            .root_items
            .iter()
            .chain(self.groups.iter().flat_map(|g| g.items.iter()))
            .filter(|c| c.debugger().is_some())
            .collect();
        out.sort_by_key(|c| !c.is_project);
        out
    }

    /// Load commands from the global commands.toml in the config directory.
    pub fn load() -> Option<Self> {
        let config_dir = get_config_dir().ok()?;
        Self::load_from_dir(&config_dir)
    }

    /// Build registry from TOML entries in the given config directory.
    pub fn load_from_dir(config_dir: &Path) -> Option<Self> {
        let metadata = CommandsMetadata::load(config_dir);

        let mut registry = Self::default();

        for (name, meta) in &metadata.entries {
            let mode = meta.mode.unwrap_or_default();
            let item = CommandItem {
                name: name.clone(),
                command: meta.command.clone(),
                mode,
                cwd: meta.cwd.unwrap_or_default(),
                is_project: false,
                metadata: Some(meta.clone()),
            };

            if let Some(ref group) = meta.group {
                if let Some(existing) = registry.groups.iter_mut().find(|g| g.name == *group) {
                    existing.items.push(item);
                } else {
                    registry.groups.push(CommandGroup {
                        name: group.clone(),
                        items: vec![item],
                        is_project: false,
                    });
                }
            } else {
                registry.root_items.push(item);
            }
        }

        registry.root_items.sort_by(|a, b| a.name.cmp(&b.name));
        registry.groups.sort_by(|a, b| a.name.cmp(&b.name));
        for group in &mut registry.groups {
            group.items.sort_by(|a, b| a.name.cmp(&b.name));
        }

        log::debug!(
            "Loaded commands from {}: {} root items, {} groups",
            config_dir.display(),
            registry.root_items.len(),
            registry.groups.len()
        );

        Some(registry)
    }

    /// Load and merge commands from global and project-local TOML.
    /// Project items appear first and are marked with `is_project = true`.
    pub fn load_merged(project_root: Option<&Path>) -> Option<Self> {
        let mut registry = Self::load().unwrap_or_default();

        if let Some(root) = project_root {
            let project_config_dir = root.join(".termide");
            if let Some(mut project) = Self::load_from_dir(&project_config_dir) {
                for item in &mut project.root_items {
                    item.is_project = true;
                }
                for group in &mut project.groups {
                    group.is_project = true;
                    for item in &mut group.items {
                        item.is_project = true;
                    }
                }
                project.root_items.append(&mut registry.root_items);
                project.groups.append(&mut registry.groups);
                registry = project;
            }
        }

        Some(registry)
    }

    /// Check if the registry has any commands.
    pub fn is_empty(&self) -> bool {
        self.root_items.is_empty() && self.groups.is_empty()
    }

    /// Get total number of commands (including those in groups).
    pub fn total_count(&self) -> usize {
        self.root_items.len() + self.groups.iter().map(|g| g.items.len()).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_commands_toml() {
        let toml = r#"
[deploy]
name = "Deploy to production"
mode = "report"
key = "Ctrl+Shift+D"

[[deploy.params]]
name = "target"
label = "Target environment"
type = "select"
default = "staging"
options = ["staging", "production"]

[[deploy.params]]
name = "verbose"
label = "Verbose output"
type = "bool"
default = "false"

[test]
mode = "background"
key = "Ctrl+T"

[clean]
"#;
        let meta = CommandsMetadata::parse(toml);
        assert_eq!(meta.entries.len(), 3);

        let deploy = &meta.entries["deploy"];
        assert_eq!(deploy.display_name.as_deref(), Some("Deploy to production"));
        assert_eq!(deploy.mode, Some(CommandMode::Report));
        assert_eq!(deploy.key.as_deref(), Some("Ctrl+Shift+D"));
        assert_eq!(deploy.params.len(), 2);
        assert_eq!(deploy.params[0].name, "target");
        assert_eq!(deploy.params[0].param_type, CommandParamType::Select);
        assert_eq!(deploy.params[0].options, vec!["staging", "production"]);
        assert_eq!(deploy.params[0].default.as_deref(), Some("staging"));
        assert_eq!(deploy.params[1].param_type, CommandParamType::Bool);

        let test = &meta.entries["test"];
        assert_eq!(test.mode, Some(CommandMode::Background));
        assert_eq!(test.key.as_deref(), Some("Ctrl+T"));
        assert!(test.params.is_empty());

        let clean = &meta.entries["clean"];
        assert!(clean.mode.is_none());
        assert!(clean.key.is_none());
    }

    /// Saving edits the file: order, comments and untouched fields stay.
    #[test]
    fn saving_keeps_order_comments_and_other_commands() {
        let original = r#"# Build helpers
[zeta]
command = "make z"   # the slow one

[alpha]
name = "Alpha"
command = "make a"

[[alpha.params]]
name = "target"
type = "select"
options = ["a", "b"]

[gone]
command = "rm -rf nothing"
"#;
        let mut meta = CommandsMetadata::parse(original);
        meta.entries.remove("gone");
        meta.entries.get_mut("alpha").unwrap().display_name = None;
        meta.entries.insert(
            "new".into(),
            CommandMetadata {
                command: Some("echo new".into()),
                mode: Some(CommandMode::Report),
                ..Default::default()
            },
        );
        let saved = meta.render_over(original);
        assert_eq!(
            saved,
            r#"# Build helpers
[zeta]
command = "make z"   # the slow one

[alpha]
command = "make a"

[[alpha.params]]
name = "target"
type = "select"
options = ["a", "b"]

[new]
command = "echo new"
mode = "report"
"#
        );
        // What was written reads back as what was saved.
        assert_eq!(CommandsMetadata::parse(&saved).entries, meta.entries);
    }

    #[test]
    fn a_new_command_does_not_take_an_existing_name() {
        let meta = CommandsMetadata::parse("[build]\ncommand = \"make\"\n[build-2]\n");
        assert_eq!(meta.free_name("test"), "test");
        assert_eq!(meta.free_name("build"), "build-3");
    }

    #[test]
    fn test_parse_empty_toml() {
        let meta = CommandsMetadata::parse("");
        assert!(meta.entries.is_empty());
    }

    #[test]
    fn test_parse_invalid_toml() {
        let meta = CommandsMetadata::parse("not valid toml [[[[");
        assert!(meta.entries.is_empty());
    }

    #[test]
    fn command_menu_keys_encode_scope_and_kind() {
        let command_key = encode_command_menu_key(CommandMenuKeyKind::Command, "build", true);
        assert_eq!(
            decode_command_menu_key(&command_key),
            Some(DecodedCommandMenuKey {
                kind: CommandMenuKeyKind::Command,
                is_project: true,
                name: "build".to_string(),
            })
        );

        let group_key = encode_command_menu_key(CommandMenuKeyKind::Group, "dev", false);
        assert_eq!(
            decode_command_menu_key(&group_key),
            Some(DecodedCommandMenuKey {
                kind: CommandMenuKeyKind::Group,
                is_project: false,
                name: "dev".to_string(),
            })
        );
    }

    #[test]
    fn a_debug_command_keeps_its_debugger_and_build() {
        let original = r#"[debug-app]
command = "target/debug/app --verbose"
debugger = "lldb"
build = "cargo build"
console = "panel"
"#;
        let meta = CommandsMetadata::parse(original);
        let entry = &meta.entries["debug-app"];
        assert_eq!(entry.debugger.as_deref(), Some("lldb"));
        assert_eq!(entry.build.as_deref(), Some("cargo build"));
        assert_eq!(entry.console.as_deref(), Some("panel"));
        // Saving untouched entries leaves the file as it was.
        assert_eq!(meta.render_over(original), original);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("commands.toml"), original).unwrap();
        let registry = CommandsRegistry::load_from_dir(dir.path()).unwrap();
        let debug: Vec<&str> = registry
            .debug_commands()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(debug, ["debug-app"]);
    }

    // `cwd` names where a command runs: unset is the focused panel's directory,
    // and a value that is neither of the two is a mistake, not a third place.
    #[test]
    fn cwd_names_where_a_command_runs() {
        let original = r#"[build]
command = "cargo build"
cwd = "project"

[test]
command = "cargo test"
"#;
        let meta = CommandsMetadata::parse(original);
        assert_eq!(meta.entries["build"].cwd, Some(CommandCwd::Project));
        assert_eq!(meta.entries["test"].cwd, None);
        // Saving untouched entries leaves the file as it was.
        assert_eq!(meta.render_over(original), original);

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("commands.toml"), original).unwrap();
        let registry = CommandsRegistry::load_from_dir(dir.path()).unwrap();
        let cwd = |name: &str| {
            registry
                .root_items
                .iter()
                .find(|c| c.name == name)
                .map(|c| c.cwd)
        };
        assert_eq!(cwd("build"), Some(CommandCwd::Project));
        assert_eq!(cwd("test"), Some(CommandCwd::Panel));

        // A value that is not `panel` or `project` is reported at parse and the
        // command runs in the panel's directory; the line itself is left alone —
        // saving an unchanged entry does not rewrite what termide did not change.
        let stray = "[stray]\ncommand = \"ls\"\ncwd = \"home\"\n";
        let meta = CommandsMetadata::parse(stray);
        assert_eq!(meta.entries["stray"].cwd, None);
        assert_eq!(meta.render_over(stray), stray);
    }

    #[test]
    fn debugger_args_are_an_array_of_words() {
        let original = r#"[go]
command = "./cmd/app"
debugger = "dlv-dap"
debugger_args = ["--only-same-user=false", "--log"]
"#;
        let mut meta = CommandsMetadata::parse(original);
        assert_eq!(
            meta.entries["go"].debugger_args,
            ["--only-same-user=false", "--log"]
        );
        assert_eq!(meta.render_over(original), original);

        meta.entries.get_mut("go").unwrap().debugger_args = vec!["--log".into()];
        let changed = meta.render_over(original);
        assert!(changed.contains("debugger_args = [\"--log\"]"), "{changed}");
        meta.entries.get_mut("go").unwrap().debugger_args.clear();
        assert!(!meta.render_over(original).contains("debugger_args"));

        // A string is a mistake, not one flag: it is left out.
        let wrong = "[go]\ndebugger = \"dlv-dap\"\ndebugger_args = \"--log\"\n";
        assert!(CommandsMetadata::parse(wrong).entries["go"]
            .debugger_args
            .is_empty());
    }
}
