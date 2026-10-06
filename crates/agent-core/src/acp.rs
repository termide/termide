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

/// The provider an external agent's messages are logged under: the
/// model behind it is the agent's business.
pub const ACP_PROVIDER: &str = "acp";

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

#[cfg(test)]
mod tests {
    use super::*;

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
