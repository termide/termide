//! Configuration of an external agent reached over ACP (the Agent Client
//! Protocol): an `AGENT.md` whose front matter names a `command`.
//!
//! ```markdown
//! ---
//! description: Claude Code through its ACP adapter
//! command: npx -y @agentclientprotocol/claude-agent-acp
//! env.ANTHROPIC_BASE_URL: http://localhost:8080
//! ---
//! ```
//!
//! The client that speaks to the process is the `termide-agent-acp` crate.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::cancel::CancelToken;
use crate::layers::SkillInfo;
use crate::message::{ToolCall, ToolResultMessage};
use crate::tool::{Tool, ToolContext, ToolRegistry, ToolUpdate};

/// The provider an external agent's messages are logged under: the
/// model behind it is the agent's business.
pub const ACP_PROVIDER: &str = "acp";

/// The key the session log keeps an external agent's model under among its
/// settings ([`crate::Session::agent_options`]), whichever way the agent
/// offers its models.
pub const MODEL_OPTION: &str = "model";

/// Seconds to wait for an external agent to start when its file names none.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConfig {
    /// Program to run; it speaks ACP on its stdin/stdout.
    pub command: String,
    pub args: Vec<String>,
    /// Environment for the process on top of termide's own, which it
    /// inherits; `$NAME` and `${NAME}` come from termide's environment.
    pub env: BTreeMap<String, String>,
    /// Seconds to wait for the agent to start and open a session.
    pub timeout_secs: u64,
    /// Which adapter this is, when termide knows it; set by the provider,
    /// never by a file.
    pub flavor: AcpFlavor,
    /// The context window the user limited the agent to, in tokens: the
    /// window the agent reports is shown no larger. `None` takes the
    /// agent's own.
    pub context_limit: Option<u64>,
}

/// An ACP adapter termide knows how to take further than the protocol: the
/// CLI agents a connection names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AcpFlavor {
    /// Any agent: driven over plain ACP, answering to its own configuration.
    #[default]
    Generic,
    /// Claude Code's adapter: it takes termide's system prompt and tools in
    /// place of its own, and leaves every permission decision to termide.
    ClaudeCode,
    /// Codex's adapter: it keeps its prompt and tools, and termide maps the
    /// permission mode onto Codex's modes.
    Codex,
    /// Gemini CLI in its ACP mode: like Codex, it keeps its prompt and tools,
    /// and termide maps the permission mode onto Gemini's approval modes.
    GeminiCli,
}

impl AcpFlavor {
    /// The flavor an `AGENT.md`'s `adapter:` names: `claude_code`, `codex`,
    /// `gemini_cli` or `generic`, as the connections' providers are named.
    ///
    /// # Errors
    ///
    /// When it names none of them.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim() {
            "claude_code" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            "gemini_cli" => Ok(Self::GeminiCli),
            "generic" => Ok(Self::Generic),
            other => Err(format!(
                "unknown adapter `{other}` (claude_code, codex, gemini_cli or generic)"
            )),
        }
    }

    /// The flavor a command line runs, judged by the adapter it names:
    /// Claude Code's or Codex's ACP adapter, or Gemini CLI in its ACP mode;
    /// any other program is generic.
    #[must_use]
    pub fn detect(command: &str, args: &[String]) -> Self {
        let words: Vec<&str> = std::iter::once(command)
            .chain(args.iter().map(String::as_str))
            .collect();
        let named = |part: &str| words.iter().any(|word| word.contains(part));
        if named("claude-agent-acp") || named("claude-code-acp") {
            return Self::ClaudeCode;
        }
        if named("codex-acp") {
            return Self::Codex;
        }
        let gemini = words.iter().any(|word| {
            let base = word.rsplit('/').next().unwrap_or(word);
            base == "gemini" || base.starts_with("gemini-cli")
        });
        if gemini && (named("--acp") || named("--experimental-acp")) {
            return Self::GeminiCli;
        }
        Self::Generic
    }
}

/// Split a command line into the program and its arguments the way a POSIX
/// shell splits words, without running one: whitespace separates words,
/// single quotes keep everything literally, double quotes keep whitespace
/// and honour `\"` and `\\`, and a backslash outside quotes takes the next
/// character as it is. Nothing is expanded.
///
/// # Errors
///
/// When a quote is left open or the line holds no word.
pub fn split_command_line(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    // A word exists once something starts it, so `''` is an empty argument.
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("unclosed single quote".to_string()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("unclosed double quote".to_string()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("unclosed double quote".to_string()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(c) = chars.next() {
                    word.push(c);
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    if words.is_empty() {
        return Err("empty command".to_string());
    }
    Ok(words)
}

/// termide's tools an external agent on its own tools (Codex, Gemini CLI) is
/// served beside them: what it has no tool of its own for — the project's
/// memory and skills, and the panel's question and command cards.
pub const COMPANION_TOOLS: &[&str] = &["recall", "skill", "question", "suggest_command"];

/// Of `tools`, those in [`COMPANION_TOOLS`], for an agent that runs on its
/// own system prompt: the `skill` tool, whose skills termide's prompt lists,
/// lists `skills` in its description instead.
#[must_use]
pub fn companion_tools(tools: &ToolRegistry, skills: &[SkillInfo]) -> ToolRegistry {
    let mut companions = ToolRegistry::new();
    for name in COMPANION_TOOLS {
        let Some(tool) = tools.get(name) else {
            continue;
        };
        if *name == "skill" {
            companions.insert(Arc::new(ListedSkills::new(Arc::clone(tool), skills)));
        } else {
            companions.insert(Arc::clone(tool));
        }
    }
    companions
}

/// The `skill` tool with the skills listed in its description.
struct ListedSkills {
    inner: Arc<dyn Tool>,
    description: String,
}

impl ListedSkills {
    fn new(inner: Arc<dyn Tool>, skills: &[SkillInfo]) -> Self {
        let mut description = inner.description().to_string();
        description.push_str("\n\nThe skills:");
        for skill in skills {
            description.push_str(&format!("\n- {}", skill.name));
            if !skill.argument_hint.is_empty() {
                description.push_str(&format!(" {}", skill.argument_hint));
            }
            if !skill.description.is_empty() {
                description.push_str(&format!(": {}", skill.description));
            }
        }
        Self { inner, description }
    }
}

impl Tool for ListedSkills {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters(&self) -> Value {
        self.inner.parameters()
    }
    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        self.inner.execute(call, ctx, on_update, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_adapter_is_told_by_the_command_it_runs() {
        let detect = |line: &str| {
            let mut words = split_command_line(line).unwrap();
            let command = words.remove(0);
            AcpFlavor::detect(&command, &words)
        };
        assert_eq!(
            detect("npx -y @agentclientprotocol/claude-agent-acp"),
            AcpFlavor::ClaudeCode
        );
        assert_eq!(detect("/opt/bin/claude-code-acp"), AcpFlavor::ClaudeCode);
        assert_eq!(
            detect("npx -y @agentclientprotocol/codex-acp@latest"),
            AcpFlavor::Codex
        );
        assert_eq!(
            detect("npx @google/gemini-cli@latest --acp"),
            AcpFlavor::GeminiCli
        );
        assert_eq!(detect("gemini --experimental-acp"), AcpFlavor::GeminiCli);
        // Gemini CLI outside its ACP mode, and anything else, is generic.
        assert_eq!(detect("gemini"), AcpFlavor::Generic);
        assert_eq!(detect("/opt/my-agent --stdio"), AcpFlavor::Generic);
        assert_eq!(AcpFlavor::parse("codex"), Ok(AcpFlavor::Codex));
        assert!(AcpFlavor::parse("cursor").is_err());
    }

    #[test]
    fn a_command_line_splits_like_shell_words() {
        assert_eq!(
            split_command_line("npx -y @agentclientprotocol/claude-agent-acp").unwrap(),
            ["npx", "-y", "@agentclientprotocol/claude-agent-acp"]
        );
        assert_eq!(
            split_command_line(r#""/opt/my agent/bin" --name 'two words' a\ b "q\"uote" ''"#)
                .unwrap(),
            [
                "/opt/my agent/bin",
                "--name",
                "two words",
                "a b",
                "q\"uote",
                ""
            ]
        );
        // Nothing is expanded: the process gets the text as written.
        assert_eq!(
            split_command_line("run $HOME *").unwrap(),
            ["run", "$HOME", "*"]
        );
        assert!(split_command_line("run 'open").is_err());
        assert!(split_command_line("run \"open").is_err());
        assert!(split_command_line("   ").is_err());
    }
}
