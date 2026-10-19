//! Web tools of the termide coding agent: `fetch` reads a page as markdown and
//! `web_search` asks a search engine, either over plain HTTP or through a local
//! Chrome driven over the DevTools protocol. See `doc/en/agent-design.md`
//! ("Web tools") for the decisions behind the shape.

mod browser;
mod cdp;
mod engine;
mod tools;
mod web;

use std::sync::Arc;

use termide_agent_core::Tool;

pub use browser::{find_chrome, Browser, Display, Page};
pub use engine::{Engine, SearchResult};
pub use tools::{FetchTool, WebSearchTool};
pub use web::{Backend, FetchedPage, Web, WebConfig};

/// The web tools `web` can back, in prompt order: `web_search` only when it
/// has an engine (over the browser or plain HTTP), `fetch` always.
#[must_use]
pub fn web_tools(web: &Arc<Web>) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    if web.can_search() {
        tools.push(Arc::new(WebSearchTool::new(Arc::clone(web))));
    }
    tools.push(Arc::new(FetchTool::new(Arc::clone(web))));
    tools
}
