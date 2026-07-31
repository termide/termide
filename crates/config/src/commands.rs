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
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "terminal" => Some(Self::Terminal),
            "background" => Some(Self::Background),
            "report" => Some(Self::Report),
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// Hotkey binding string like "Ctrl+Shift+D".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Parameter definitions for the launch form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<CommandParam>,
    /// Group (virtual grouping in menu).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
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

            entries.insert(
                section_name.clone(),
                CommandMetadata {
                    command,
                    display_name,
                    mode,
                    key,
                    params,
                    group,
                },
            );
        }

        Self { entries }
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
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        let toml_path = config_dir.join("commands.toml");
        if let Some(parent) = toml_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&self.entries).map_err(std::io::Error::other)?;
        std::fs::write(&toml_path, content)
    }
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
    /// Whether this command comes from a project-local `.termide/` directory.
    pub is_project: bool,
    /// Metadata from commands.toml.
    pub metadata: Option<CommandMetadata>,
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
}
