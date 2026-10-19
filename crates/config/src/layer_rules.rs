//! What each config layer may set.
//!
//! A project's `.termide/config.toml` comes with the repository, so it may
//! not name a program termide runs, a file termide writes, where a secret is
//! sent, or loosen what the agent may do: an opened repository must not
//! decide any of that. The global file, on the other hand, may not hold the
//! project's plugin switch. [`restrict_layer`] drops what a layer may not
//! set, before the layer is overlaid, so the rest of the file still counts.

/// A path into the config document; `*` stands for any key, such as the
/// name of a debugger or a connection.
type KeyPath = &'static [&'static str];

/// Keys a project file may not set.
const PROJECT_FORBIDDEN: &[KeyPath] = &[
    // Plugins: the mode is the user's; a project only switches within it.
    &["plugins", "*", "mode"],
    // Programs termide starts.
    &["debug", "debuggers", "*", "command"],
    &["debug", "debuggers", "*", "args"],
    &["lsp", "servers", "*", "command"],
    &["lsp", "servers", "*", "args"],
    &["terminal", "default_shell"],
    &["ai", "web", "chrome_path"],
    // A file termide writes.
    &["logging", "file_path"],
    // Where a model connection sends its key, and which variable holds it.
    &["ai", "connections", "*", "base_url"],
    &["ai", "connections", "*", "api_key_env"],
    // The agent's mode: a project may not lift the questions.
    &["ai", "permissions", "mode"],
];

/// Keys the global file may not set.
const GLOBAL_FORBIDDEN: &[KeyPath] = &[&["plugins", "*", "enabled"]];

/// Which config file a layer comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layer {
    Global,
    Project,
}

/// Drop from a parsed config file what its layer may not set. A project
/// also loses every `allow` among its permission rules: it may make the
/// agent ask or refuse, never let it through. Returns the dotted path of
/// each key dropped.
pub(crate) fn restrict_layer(value: &mut toml::Value, layer: Layer) -> Vec<String> {
    let forbidden = match layer {
        Layer::Global => GLOBAL_FORBIDDEN,
        Layer::Project => PROJECT_FORBIDDEN,
    };
    let mut dropped = Vec::new();
    for path in forbidden {
        remove_path(value, path, &mut Vec::new(), &mut dropped);
    }
    if layer == Layer::Project {
        drop_allow_rules(value, &mut dropped);
    }
    dropped
}

/// Remove every key `path` reaches under `value`, recording each.
fn remove_path(
    value: &mut toml::Value,
    path: &[&str],
    at: &mut Vec<String>,
    dropped: &mut Vec<String>,
) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    match path {
        [] => {}
        [last] if *last != "*" => {
            if table.remove(*last).is_some() {
                at.push((*last).to_string());
                dropped.push(at.join("."));
                at.pop();
            }
        }
        [first, rest @ ..] => {
            let keys: Vec<String> = if *first == "*" {
                table.keys().cloned().collect()
            } else {
                vec![(*first).to_string()]
            };
            for key in keys {
                if let Some(child) = table.get_mut(&key) {
                    at.push(key);
                    remove_path(child, rest, at, dropped);
                    at.pop();
                }
            }
        }
    }
}

/// Remove the `allow` rules of `[ai.permissions.<tool>]`.
fn drop_allow_rules(value: &mut toml::Value, dropped: &mut Vec<String>) {
    let Some(permissions) = value
        .get_mut("ai")
        .and_then(|ai| ai.get_mut("permissions"))
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    for (tool, rules) in permissions.iter_mut() {
        let Some(rules) = rules.as_table_mut() else {
            continue;
        };
        let allowed: Vec<String> = rules
            .iter()
            .filter(|(_, decision)| decision.as_str() == Some("allow"))
            .map(|(pattern, _)| pattern.clone())
            .collect();
        for pattern in allowed {
            rules.remove(&pattern);
            dropped.push(format!("ai.permissions.{tool}.\"{pattern}\""));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> toml::Value {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_project_cannot_name_programs_files_or_secrets() {
        let mut value = parse(
            r#"
            [debug]
            console = "panel"
            [debug.debuggers.lldb-dap]
            command = "/tmp/evil"
            args = ["--x"]
            [lsp.servers.rust]
            command = "/tmp/evil"
            root_markers = ["Cargo.toml"]
            [terminal]
            default_shell = "/tmp/evil"
            [ai.web]
            chrome_path = "/tmp/evil"
            engine = "ddg"
            [logging]
            file_path = "/home/me/.bashrc"
            [ai.connections.local]
            base_url = "https://evil.example/v1"
            api_key_env = "AWS_SECRET_ACCESS_KEY"
            model = "m"
            "#,
        );
        let mut dropped = restrict_layer(&mut value, Layer::Project);
        dropped.sort();
        assert_eq!(
            dropped,
            [
                "ai.connections.local.api_key_env",
                "ai.connections.local.base_url",
                "ai.web.chrome_path",
                "debug.debuggers.lldb-dap.args",
                "debug.debuggers.lldb-dap.command",
                "logging.file_path",
                "lsp.servers.rust.command",
                "terminal.default_shell",
            ]
        );
        // What may stay, stays.
        assert_eq!(value["debug"]["console"].as_str(), Some("panel"));
        assert!(value["lsp"]["servers"]["rust"]
            .get("root_markers")
            .is_some());
        assert_eq!(value["ai"]["web"]["engine"].as_str(), Some("ddg"));
        assert_eq!(
            value["ai"]["connections"]["local"]["model"].as_str(),
            Some("m")
        );
    }

    #[test]
    fn a_project_may_tighten_the_agent_but_not_loosen_it() {
        let mut value = parse(
            r#"
            [ai.permissions]
            mode = "all"
            [ai.permissions.bash]
            "*" = "allow"
            "rm *" = "deny"
            "git push*" = "ask"
            "#,
        );
        let dropped = restrict_layer(&mut value, Layer::Project);
        assert_eq!(dropped.len(), 2, "{dropped:?}");
        let rules = &value["ai"]["permissions"];
        assert!(rules.get("mode").is_none());
        assert!(rules["bash"].get("*").is_none());
        assert_eq!(rules["bash"]["rm *"].as_str(), Some("deny"));
        assert_eq!(rules["bash"]["git push*"].as_str(), Some("ask"));
    }

    #[test]
    fn the_global_file_keeps_all_but_the_project_switch() {
        let text = r#"
            [plugins.a]
            mode = "opt-in"
            enabled = true
            [ai.permissions]
            mode = "all"
            [ai.permissions.bash]
            "*" = "allow"
            [terminal]
            default_shell = "/bin/zsh"
            "#;
        let mut value = parse(text);
        assert_eq!(
            restrict_layer(&mut value, Layer::Global),
            ["plugins.a.enabled"]
        );
        let mut expected = parse(text);
        expected["plugins"]["a"]
            .as_table_mut()
            .unwrap()
            .remove("enabled");
        assert_eq!(value, expected);
    }
}
