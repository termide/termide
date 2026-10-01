//! Configuration of MCP servers, `ai/mcp.toml`: one table per server.
//!
//! The shape is data the agent directories carry, so it lives here beside
//! the other directory contents; the client that speaks to a server is the
//! `termide-agent-mcp` crate.
//!
//! ```toml
//! [github]
//! command = "npx"
//! args = ["-y", "@modelcontextprotocol/server-github"]
//! env = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_TOKEN" }
//! tools = ["search_issues", "get_issue"]
//! ```
//!
//! A `.mcp.json` at the root of a project is read beside it, so a config
//! committed for another tool works here unchanged, as `.agents/skills` and
//! `CLAUDE.md` do; see [`mcp_servers_from_json`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The file, at any level of the `ai` directory.
pub const MCP_FILE: &str = "mcp.toml";

/// Another tool's file, at the root of a project rather than inside an `ai`
/// directory: the portable shape Claude Code writes for a shared project and
/// Cursor and Windsurf repeat, and the one VS Code calls portable and steers
/// new servers to. There is no standard for this file — the MCP specification
/// leaves configuration to the client — so the name and the shape are what
/// the tools settled on, and only that much is taken here.
pub const MCP_JSON_FILE: &str = ".mcp.json";

/// The keys a `.mcp.json` may carry its servers under: `mcpServers` is the
/// shape Claude Code, Cursor and Windsurf write, `servers` the one VS Code
/// writes in its own files and is moving its portable files to. The first
/// one present wins; the entries under it are the same object either way.
const MCP_JSON_KEYS: [&str; 2] = ["mcpServers", "servers"];

/// One MCP server: either a program termide starts over stdio, or a URL it
/// speaks Streamable HTTP to. Exactly one of `command` and `url` is set;
/// [`McpServerConfig::target`] is where that is decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Program to run; absent on a `url` server.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment for the server process. `$NAME` and `${NAME}` in a value
    /// are replaced from termide's own environment, so a token stays out of
    /// the file.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory of the server; the panel's when absent.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The server's address over Streamable HTTP; absent on a `command`
    /// server. One POST per request, the reply in the response body, the
    /// session in the `Mcp-Session-Id` header.
    #[serde(default)]
    pub url: Option<String>,
    /// Headers for a `url` server — the shape Claude Code and VS Code carry
    /// their API keys in. `$NAME` is expanded as in `env`, so a token stays
    /// out of the file; unlike a stdio server's arguments these never reach
    /// the process list.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Tools to take from the server, by their names there; all of them
    /// when absent. Every tool costs its schema in each request, so a server
    /// with dozens is worth narrowing.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Seconds to wait for the server to start and for one tool call.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// `false` at a higher level switches off a server a lower level defines.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// How a `url` server that asks for OAuth is signed in to. Nothing is
    /// needed for a server that registers clients itself; this is for one that
    /// does not, or that wants a fixed callback port.
    #[serde(default)]
    pub oauth: McpOAuth,
}

/// The OAuth side of a `url` server: what termide cannot learn from the
/// server's own metadata. A `Authorization` in `headers` takes precedence —
/// a static key is the server's whole sign-in, and nothing here is used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuth {
    /// A client registered with the authorization server by hand; absent,
    /// termide registers itself where the server allows it.
    #[serde(default)]
    pub client_id: Option<String>,
    /// The secret of a confidential `client_id`; `$NAME` is expanded as in
    /// `env`, so it can stay out of the file.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Scopes to ask for; the server's advertised ones when absent.
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// The loopback port the browser returns to; any free one when absent.
    /// A client registered by hand usually allows one exact redirect URI.
    #[serde(default)]
    pub callback_port: Option<u16>,
}

/// Where one configured server stands, as `/mcp` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    Connecting,
    Ready {
        tools: usize,
    },
    Failed(String),
    /// It wants a sign-in termide does not hold.
    NeedsLogin,
    /// A sign-in waits in the browser.
    SigningIn,
}

/// Whether a server is one termide signs in to, and whether it is signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSignIn {
    /// A program, a URL with a static `Authorization`, or no place to keep
    /// a sign-in: there is nothing to sign in to.
    None,
    SignedOut,
    SignedIn,
}

/// One configured server, as `/mcp` and the toolset list show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerState {
    pub name: String,
    pub status: McpStatus,
    pub sign_in: McpSignIn,
}

/// What a reload of the configuration did, by server name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpReload {
    /// New, changed or not connected before: connecting now.
    pub started: Vec<String>,
    /// No longer configured: gone with their tools.
    pub removed: Vec<String>,
    /// Connected and unchanged: left as they were.
    pub kept: Vec<String>,
}

/// A server nothing is configured for: no way to reach it, so
/// [`McpServerConfig::target`] refuses it. The values are what an absent
/// field takes, which is why the timeout is the default and not zero.
impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            url: None,
            headers: BTreeMap::new(),
            tools: None,
            timeout_secs: default_timeout(),
            enabled: true,
            oauth: McpOAuth::default(),
        }
    }
}

/// What a configured server is: a process to start, or an address to post to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTarget<'a> {
    /// A program termide starts, speaking one JSON-RPC message per line over
    /// its stdin and stdout.
    Stdio {
        command: &'a str,
        args: &'a [String],
    },
    /// A server at a URL, spoken to over Streamable HTTP.
    Http { url: &'a str },
}

impl McpServerConfig {
    /// What to connect to. An entry with both spellings or neither is a
    /// broken entry and is reported as one rather than guessed at — the same
    /// rule as the skipped entries of a `.mcp.json`.
    ///
    /// # Errors
    ///
    /// When neither `command` nor `url` is set, or when both are.
    pub fn target(&self) -> Result<McpTarget<'_>, String> {
        match (self.url.as_deref(), self.command.as_deref()) {
            (Some(url), None) => Ok(McpTarget::Http { url }),
            (None, Some(command)) => Ok(McpTarget::Stdio {
                command,
                args: &self.args,
            }),
            (Some(_), Some(_)) => {
                Err("sets both command and url; a server is one or the other".to_string())
            }
            (None, None) => Err("sets neither command nor url".to_string()),
        }
    }
}

fn default_timeout() -> u64 {
    60
}

fn default_enabled() -> bool {
    true
}

/// Replace `$NAME` and `${NAME}` with `lookup(NAME)`; an unknown name
/// becomes empty. `$$` is a literal dollar.
pub fn expand_env(value: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        if let Some(after) = tail.strip_prefix('$') {
            out.push('$');
            rest = after;
        } else if let Some(after) = tail.strip_prefix('{') {
            match after.find('}') {
                Some(end) => {
                    out.push_str(&lookup(&after[..end]).unwrap_or_default());
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('$');
                    rest = tail;
                }
            }
        } else {
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len());
            if end == 0 {
                out.push('$');
                rest = tail;
            } else {
                out.push_str(&lookup(&tail[..end]).unwrap_or_default());
                rest = &tail[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Servers from another tool's `.mcp.json`, in the portable shape Claude Code,
/// Cursor and Windsurf share and VS Code now steers new servers to. Read at
/// the root of a project only, beside the `ai` directory's `mcp.toml`, so a
/// repository committed for another tool brings its servers here unchanged.
///
/// `command` with `args`, `env`, `cwd` and `tools`, or `url` with `headers`;
/// `disabled` and `enabled = false` are both read as off. Anything else is
/// skipped with a reason, so a server never starts on half a configuration: an
/// `sse` entry, the older transport whose requests go to an address its
/// stream names first, and one whose values hold a reference termide cannot
/// resolve — `${input:…}` above all, since that asks the user for a secret in
/// the other tool and an empty token would go out quietly.
///
/// `oauth` is read for its `clientId`, `clientSecret`, `scopes` and
/// `callbackPort`, the fields Claude Code writes there. `envFile`, `inputs`
/// and `sandbox` are not read and not reported: not termide's to run, and the
/// server runs the same without them.
///
/// Returns the servers and the reasons to report for what was not taken; a
/// file that does not parse is reported as such and contributes nothing.
#[must_use]
pub fn mcp_servers_from_json(path: &Path) -> (BTreeMap<String, McpServerConfig>, Vec<String>) {
    let mut servers = BTreeMap::new();
    let mut skipped = Vec::new();
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (servers, skipped);
        }
        Err(error) => {
            skipped.push(format!("{} cannot be read: {error}", path.display()));
            return (servers, skipped);
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(root) => root,
        Err(error) => {
            skipped.push(format!("{} is not valid JSON: {error}", path.display()));
            return (servers, skipped);
        }
    };
    let entries = MCP_JSON_KEYS
        .iter()
        .find_map(|key| root.get(*key).and_then(Value::as_object));
    let Some(entries) = entries else {
        let keys = MCP_JSON_KEYS.join(" or ");
        skipped.push(format!(
            "{} has neither {keys}: no servers taken",
            path.display()
        ));
        return (servers, skipped);
    };
    // `${workspaceFolder}` is the directory the file sits in, which is how
    // the tool that wrote it means it.
    let workspace = path.parent().unwrap_or(Path::new("."));
    for (name, entry) in entries {
        match server_from_json(entry, workspace) {
            // The same name at a higher level wins, as it does between roots.
            Ok(config) => {
                servers.entry(name.clone()).or_insert(config);
            }
            Err(reason) => skipped.push(format!("{name}: {reason}")),
        }
    }
    (servers, skipped)
}

/// One entry of a `.mcp.json`; `Err` is the reason not to take it.
fn server_from_json(entry: &Value, workspace: &Path) -> Result<McpServerConfig, String> {
    let url = entry.get("url").and_then(Value::as_str);
    let command = entry.get("command").and_then(Value::as_str);
    // A `url` entry is not a refusal any more: termide speaks Streamable
    // HTTP to it. Both spellings at once is a broken entry, not a choice.
    if url.is_none() && command.is_none() {
        return Err("sets neither command nor url".to_string());
    }
    if let (Some(u), Some(c)) = (url, command) {
        return Err(format!("sets both command ({c}) and url ({u})"));
    }
    // `sse` is the older transport — a GET first, whose stream names the
    // address requests are posted to — and not the one implemented here, so
    // it is refused by name rather than dialed wrongly and timing out.
    if url.is_some() && entry.get("type").and_then(Value::as_str) == Some("sse") {
        return Err("an sse server: termide speaks Streamable HTTP".to_string());
    }
    // `${...}` is resolved here, `$NAME` and `${env:NAME}` reach expand_env,
    // which reads termide's environment when the server starts.
    let text = |value: &Value| -> Result<String, String> {
        match value {
            Value::String(text) => expand_json_vars(text, workspace),
            // A number the other tool wrote unquoted goes through as written.
            Value::Number(n) => Ok(n.to_string()),
            // `null`, absent: a variable left empty is an empty one.
            _ => Ok(String::new()),
        }
    };
    let list = |key: &str| -> Result<Vec<String>, String> {
        entry
            .get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().map(text).collect::<Result<Vec<_>, _>>())
            .transpose()
            .map(|list| list.unwrap_or_default())
    };
    let cwd = entry
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map(|cwd| expand_json_vars(cwd, workspace))
        .transpose()?
        .map(PathBuf::from);
    let tools = entry
        .get("tools")
        .and_then(Value::as_array)
        .map(|_| list("tools"))
        .transpose()?
        .filter(|tools| !tools.is_empty());
    let mut headers = BTreeMap::new();
    if let Some(entries) = entry.get("headers").and_then(Value::as_object) {
        for (key, value) in entries {
            headers.insert(key.clone(), text(value)?);
        }
    }
    let mut env = BTreeMap::new();
    if let Some(entries) = entry.get("env").and_then(Value::as_object) {
        for (key, value) in entries {
            env.insert(key.clone(), text(value)?);
        }
    }
    // Both spellings of the switch: termide's `enabled = false` and the
    // `disabled` the portable shape writes. Either one turns it off, and
    // either is carried as `enabled = false` so a lower level cannot switch
    // the server back on.
    let off = entry.get("disabled").and_then(Value::as_bool) == Some(true)
        || entry.get("enabled").and_then(Value::as_bool) == Some(false);
    let oauth = match entry.get("oauth") {
        Some(oauth) => {
            let field = |key: &str| oauth.get(key).map(text).transpose();
            McpOAuth {
                client_id: field("clientId")?.filter(|id| !id.is_empty()),
                client_secret: field("clientSecret")?.filter(|s| !s.is_empty()),
                // An array, or the space-separated string OAuth itself uses.
                scopes: match oauth.get("scopes") {
                    Some(Value::Array(items)) => {
                        Some(items.iter().map(text).collect::<Result<Vec<_>, _>>()?)
                    }
                    Some(Value::String(scopes)) => Some(
                        expand_json_vars(scopes, workspace)?
                            .split_whitespace()
                            .map(str::to_string)
                            .collect(),
                    ),
                    _ => None,
                },
                callback_port: oauth
                    .get("callbackPort")
                    .and_then(Value::as_u64)
                    .and_then(|port| u16::try_from(port).ok()),
            }
        }
        None => McpOAuth::default(),
    };
    Ok(McpServerConfig {
        command: command.map(str::to_string),
        args: list("args")?,
        env,
        cwd,
        url: url.map(str::to_string),
        headers,
        tools,
        timeout_secs: entry
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or_else(default_timeout),
        enabled: !off,
        oauth,
    })
}

/// Resolve what a `.mcp.json` may reference in a value termide runs:
/// `${workspaceFolder}` — and Codex's `${workspaceRoot}` — is the directory
/// the file is in, `${userHome}` the home directory, `${env:NAME}` rewritten
/// to `$NAME` so [`expand_env`] takes it from termide's environment at spawn.
/// A bare `$NAME` is left as written, for the same reason.
///
/// `Err` is a reference termide cannot resolve — `${input:NAME}` asks the
/// user for a secret when the other tool starts the server, and termide asks
/// nowhere outside its own permission card — or an unknown `${…}`, which
/// would otherwise reach the server as literal text.
///
/// [`expand_env`]: crate::expand_env
fn expand_json_vars(text: &str, workspace: &Path) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let Some(end) = tail.find('}') else {
            out.push_str("${");
            rest = tail;
            continue;
        };
        let name = &tail[..end];
        rest = &tail[end + 1..];
        if let Some(var) = name.strip_prefix("env:") {
            out.push('$');
            out.push_str(var);
            continue;
        }
        match name {
            "workspaceFolder" | "workspaceRoot" => {
                out.push_str(&workspace.display().to_string());
            }
            "userHome" | "home" => out.push_str(&home_dir()),
            _ => {
                return Err(format!(
                    "cannot resolve ${{{name}}}; termide takes $NAME from its environment"
                ))
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The user's home, the way the `dirs` crate resolves it for the
/// configuration directory, without taking the dependency here.
fn home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_placeholders_expand_from_the_lookup() {
        let lookup = |name: &str| match name {
            "TOKEN" => Some("t0k".to_string()),
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(expand_env("$TOKEN", lookup), "t0k");
        assert_eq!(
            expand_env("${HOME}/bin:$HOME", lookup),
            "/home/u/bin:/home/u"
        );
        assert_eq!(expand_env("x$MISSING-y", lookup), "x-y");
        assert_eq!(expand_env("cost $$5 $", lookup), "cost $5 $");
        assert_eq!(expand_env("${unterminated", lookup), "${unterminated");
    }

    #[test]
    fn a_server_table_parses_with_defaults() {
        let servers: BTreeMap<String, McpServerConfig> = toml::from_str(
            "[github]\ncommand = \"npx\"\nargs = [\"-y\", \"server\"]\n\n[off]\ncommand = \"x\"\nenabled = false\n",
        )
        .unwrap();
        let github = &servers["github"];
        assert_eq!(github.args, ["-y", "server"]);
        assert_eq!(github.timeout_secs, 60);
        assert!(github.enabled && github.tools.is_none());
        assert!(!servers["off"].enabled);
    }

    /// Write one portable file into a temporary directory and hand back the
    /// path it sits at.
    fn portable(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(MCP_JSON_FILE);
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn a_portable_mcp_json_is_taken_as_written() {
        let (_dir, path) = portable(
            r#"{"mcpServers":{"github":{"command":"npx","args":["-y","server-github"],
               "env":{"TOKEN":"$GITHUB_TOKEN","PORT":8080,"VOID":null},
               "tools":["search_issues"],"disabled":false},
               "remote":{"type":"http","url":"https://mcp.example.com/mcp",
                        "headers":{"Authorization":"Bearer t0k"}},
               "stream":{"type":"sse","url":"https://mcp.example.com/sse"},
               "off":{"command":"x","disabled":true},
               "also_off":{"command":"y","enabled":false},
               "ask":{"command":"x","env":{"K":"${input:key}"}}}}"#,
        );
        let (servers, skipped) = mcp_servers_from_json(&path);
        // `disabled` and `enabled = false` are both carried rather than
        // dropped, so a lower level cannot switch the server back on.
        assert_eq!(
            servers.keys().collect::<Vec<_>>(),
            ["also_off", "github", "off", "remote"]
        );
        assert!(servers["github"].enabled);
        assert!(!servers["off"].enabled && !servers["also_off"].enabled);
        let github = &servers["github"];
        assert_eq!(github.command.as_deref(), Some("npx"));
        assert_eq!(github.args, ["-y", "server-github"]);
        assert_eq!(
            github.tools.as_deref(),
            Some(&["search_issues".to_string()][..])
        );
        assert_eq!(github.env["TOKEN"], "$GITHUB_TOKEN");
        assert_eq!(github.env["PORT"], "8080");
        assert_eq!(github.env["VOID"], "");
        assert_eq!(github.timeout_secs, 60);
        // A url server is taken as itself: no command, the address instead.
        let remote = &servers["remote"];
        assert_eq!(remote.command, None);
        assert_eq!(remote.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert_eq!(remote.headers["Authorization"], "Bearer t0k");
        assert_eq!(
            remote.target(),
            Ok(crate::McpTarget::Http {
                url: "https://mcp.example.com/mcp"
            })
        );
        assert!(matches!(
            github.target(),
            Ok(crate::McpTarget::Stdio { command: "npx", .. })
        ));
        // What is left is refused with a reason, never started half-made.
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(skipped[0].starts_with("ask:") && skipped[0].contains("${input:key}"));
        assert!(skipped[1].starts_with("stream:") && skipped[1].contains("sse"));
    }

    #[test]
    fn the_vs_code_key_and_the_path_references_work_too() {
        let (_dir, path) = portable(
            r#"{"servers":{"fs":{"command":"server","args":["${workspaceFolder}/bin"],
               "cwd":"${workspaceFolder}","env":{"H":"${userHome}","E":"${env:PATH}"}}}}"#,
        );
        let (servers, skipped) = mcp_servers_from_json(&path);
        assert!(skipped.is_empty(), "{skipped:?}");
        let fs = &servers["fs"];
        let workspace = path.parent().unwrap();
        assert_eq!(fs.args, [format!("{}/bin", workspace.display())]);
        assert_eq!(fs.cwd, Some(workspace.to_path_buf()));
        assert_eq!(fs.env["H"], home_dir());
        // Left for expand_env, which reads the environment when it starts.
        assert_eq!(fs.env["E"], "$PATH");
    }

    #[test]
    fn an_unreadable_or_wrong_shaped_file_is_reported_not_fatal() {
        // Absent: nothing at all, and no complaint.
        let dir = tempfile::tempdir().unwrap();
        let (servers, skipped) = mcp_servers_from_json(&dir.path().join(MCP_JSON_FILE));
        assert!(servers.is_empty() && skipped.is_empty());

        let (_dir, path) = portable("{ not json");
        let (servers, skipped) = mcp_servers_from_json(&path);
        assert!(servers.is_empty());
        assert!(skipped[0].contains("not valid JSON"), "{skipped:?}");

        // Another tool's file that carries neither key.
        let (_dir, path) = portable(r#"{"permissions":{"a":"b"}}"#);
        let (servers, skipped) = mcp_servers_from_json(&path);
        assert!(servers.is_empty());
        assert!(skipped[0].contains("mcpServers or servers"), "{skipped:?}");
    }

    #[test]
    fn an_unknown_reference_refuses_the_server_it_sits_in() {
        let (_dir, path) = portable(
            r#"{"mcpServers":{"good":{"command":"a"},"bad":{"command":"b","args":["${cmd:x}"]}}}"#,
        );
        let (servers, skipped) = mcp_servers_from_json(&path);
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["good"]);
        assert!(skipped[0].contains("${cmd:x}"), "{skipped:?}");
    }
}
