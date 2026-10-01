//! An MCP server's tool as a [`Tool`] of the agent.

use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate};

use crate::client::{McpToolInfo, McpTransport};

pub struct McpTool {
    name: String,
    description: String,
    info: McpToolInfo,
    /// Over stdio or over Streamable HTTP: a tool does not care which.
    client: Arc<dyn McpTransport>,
}

impl McpTool {
    #[must_use]
    pub fn new(server: &str, info: McpToolInfo, client: Arc<dyn McpTransport>) -> Self {
        let description = if info.description.is_empty() {
            format!("Tool `{}` of the MCP server `{server}`.", info.name)
        } else {
            info.description.clone()
        };
        Self {
            name: tool_name(server, &info.name),
            description,
            info,
            client,
        }
    }
}

/// `<server>__<tool>`, with anything outside `[A-Za-z0-9_-]` replaced, since
/// OpenAI-compatible endpoints accept only those characters in a function
/// name. Two underscores keep the server apart from a tool with its own
/// underscores, the way Claude Code's `mcp__server__tool` does.
#[must_use]
pub fn tool_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    let name = format!("{}__{}", clean(server), clean(tool));
    if name.len() > 64 {
        log::warn!("MCP tool name {name} is longer than the 64 characters some endpoints allow");
    }
    name
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        if self.info.input_schema.is_object() {
            self.info.input_schema.clone()
        } else {
            json!({ "type": "object", "properties": {} })
        }
    }

    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        match self
            .client
            .call_tool(&self.info.name, &call.arguments, cancel)
        {
            Ok((text, false)) => ToolResultMessage::text(call, text),
            Ok((text, true)) => ToolResultMessage::error(call, text),
            Err(error) => ToolResultMessage::error(call, error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_endpoint_safe() {
        assert_eq!(
            tool_name("github", "search_issues"),
            "github__search_issues"
        );
        assert_eq!(tool_name("my server", "do.it"), "my_server__do_it");
    }
}
