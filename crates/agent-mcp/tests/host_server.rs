//! termide's own MCP server against termide's own client: a change of the
//! tools it serves reaches a connected agent over the server stream, as it
//! must for an MCP server of termide's that connects after the agent's
//! session started.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, LateTools, McpServerConfig, NoHooks, Tool, ToolCall, ToolContext, ToolRegistry,
    ToolResultMessage, ToolUpdate,
};
use termide_agent_mcp::{Connections, McpServer};

struct Named(&'static str);

impl Tool for Named {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "A tool"
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }
    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        ToolResultMessage::text(call, self.0)
    }
}

fn registry(names: &[&'static str]) -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    for name in names {
        tools.insert(Arc::new(Named(name)));
    }
    tools
}

fn names(events: &Receiver<LateTools>) -> Vec<String> {
    match events
        .recv_timeout(Duration::from_secs(15))
        .expect("an event in time")
    {
        LateTools::Ready { tools, .. } => tools.iter().map(|t| t.name().to_string()).collect(),
        LateTools::Failed { error, .. } => panic!("failed: {error}"),
        _ => panic!("an unexpected event"),
    }
}

#[test]
fn a_change_of_the_served_tools_reaches_the_connected_agent() {
    let server =
        McpServer::start(registry(&["read"]), Box::new(NoHooks), PathBuf::from(".")).unwrap();
    let config = McpServerConfig {
        url: Some(server.url()),
        headers: [(
            "Authorization".to_string(),
            format!("Bearer {}", server.token()),
        )]
        .into_iter()
        .collect(),
        timeout_secs: 5,
        ..Default::default()
    };
    let connections = Connections::new(BTreeMap::from([("host".to_string(), config)]));
    let events = connections.subscribe();
    assert_eq!(names(&events), ["host__read"]);

    // An MCP server of termide's connected: its tools join the set, and the
    // agent lists them without connecting again.
    server.set_tools(registry(&["read", "db__query"]));
    assert_eq!(names(&events), ["host__read", "host__db__query"]);

    // It went away again: its tools leave.
    server.set_tools(registry(&["read"]));
    assert_eq!(names(&events), ["host__read"]);
}
