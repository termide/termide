//! `fetch` and `web_search` as agent tools over the shared [`Web`] service.

use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

use crate::web::Web;

/// Page size of `fetch`, as for `read`: whichever cap comes first.
const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 64 * 1024;
/// Results `web_search` returns unless asked otherwise, and at most.
const DEFAULT_RESULTS: usize = 10;
const MAX_RESULTS: usize = 30;

pub struct FetchTool {
    web: Arc<Web>,
}

impl FetchTool {
    #[must_use]
    pub fn new(web: Arc<Web>) -> Self {
        Self { web }
    }
}

impl Tool for FetchTool {
    fn name(&self) -> &str {
        "fetch"
    }

    fn description(&self) -> &str {
        &ToolText::seed("fetch").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "An http or https URL" },
                "offset": { "type": "integer", "minimum": 1, "description": "1-based line number to start from (default 1)" },
                "limit": { "type": "integer", "minimum": 1, "description": "Maximum number of lines to return (default 2000)" }
            },
            "required": ["url"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("fetch").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("fetch").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        match self.fetch(call, cancel) {
            Ok((text, details)) => ToolResultMessage::text(call, text).with_details(details),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

impl FetchTool {
    fn fetch(&self, call: &ToolCall, cancel: &CancelToken) -> Result<(String, Value), String> {
        let url = required_str(call, "url")?.trim();
        let parsed = url::Url::parse(url).map_err(|error| format!("invalid URL {url}: {error}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!(
                "`fetch` reads http and https URLs, not {}",
                parsed.scheme()
            ));
        }
        let offset = optional_usize(call, "offset")?.unwrap_or(1).max(1);
        let limit = optional_usize(call, "limit")?
            .unwrap_or(MAX_LINES)
            .clamp(1, MAX_LINES);

        let page = self.web.fetch(url, cancel)?;
        let mut out = format!("URL: {}\n", page.url);
        if !page.title.is_empty() {
            out.push_str(&format!("Title: {}\n", page.title));
        }
        out.push('\n');

        let lines: Vec<&str> = page.content.lines().collect();
        let total = lines.len();
        if total == 0 {
            out.push_str("[the page has no text]");
            return Ok((
                out,
                json!({ "url": page.url, "title": page.title, "total_lines": 0 }),
            ));
        }
        if offset > total {
            return Err(format!(
                "offset {offset} is past the end of the page ({total} lines)"
            ));
        }
        let body_budget = MAX_BYTES.saturating_sub(out.len());
        let mut body = String::new();
        let mut shown_end = offset - 1;
        for (index, line) in lines.iter().enumerate().skip(offset - 1).take(limit) {
            if !body.is_empty() && body.len() + line.len() + 1 > body_budget {
                break;
            }
            body.push_str(line);
            body.push('\n');
            shown_end = index + 1;
        }
        out.push_str(body.trim_end_matches('\n'));
        let truncated = shown_end < total;
        if truncated {
            out.push_str(&format!(
                "\n\n[Showing lines {offset}-{shown_end} of {total}. Use offset={} to continue.]",
                shown_end + 1
            ));
        }
        Ok((
            out,
            json!({
                "url": page.url,
                "title": page.title,
                "total_lines": total,
                "shown": [offset, shown_end],
                "truncated": truncated
            }),
        ))
    }
}

pub struct WebSearchTool {
    web: Arc<Web>,
}

impl WebSearchTool {
    #[must_use]
    pub fn new(web: Arc<Web>) -> Self {
        Self { web }
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        &ToolText::seed("web_search").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What to search for" },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_RESULTS, "description": "Number of results (default 10)" }
            },
            "required": ["query"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("web_search").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("web_search").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        match self.search(call, on_update, cancel) {
            Ok((text, details)) => ToolResultMessage::text(call, text).with_details(details),
            Err(message) => ToolResultMessage::error(call, message),
        }
    }
}

impl WebSearchTool {
    fn search(
        &self,
        call: &ToolCall,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> Result<(String, Value), String> {
        let query = required_str(call, "query")?.trim();
        if query.is_empty() {
            return Err("`query` must not be empty".into());
        }
        let limit = optional_usize(call, "limit")?
            .unwrap_or(DEFAULT_RESULTS)
            .clamp(1, MAX_RESULTS);
        let mut on_wait = |message: String| on_update(ToolUpdate::Output(message));
        let results = self.web.search(query, limit, cancel, &mut on_wait)?;
        if results.is_empty() {
            return Ok((
                format!(
                    "No results for \"{query}\". If the engine's results page changed, its \
                     selectors in ai/web/engines/ need updating."
                ),
                json!({ "query": query, "results": 0 }),
            ));
        }
        let mut out = String::new();
        for (index, result) in results.iter().enumerate() {
            out.push_str(&format!(
                "{}. {}\n   {}\n",
                index + 1,
                result.title,
                result.url
            ));
            if !result.snippet.is_empty() {
                out.push_str(&format!("   {}\n", result.snippet));
            }
        }
        Ok((
            out.trim_end().to_string(),
            json!({ "query": query, "results": results.len() }),
        ))
    }
}

fn required_str<'a>(call: &'a ToolCall, key: &str) -> Result<&'a str, String> {
    match call.arguments.get(key) {
        Some(value) => value
            .as_str()
            .ok_or_else(|| format!("`{key}` must be a string")),
        None => Err(format!("missing required argument `{key}`")),
    }
}

fn optional_usize(call: &ToolCall, key: &str) -> Result<Option<usize>, String> {
    match call.arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|n| Some(n as usize))
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::Display;
    use crate::web::{Backend, FetchedPage, WebConfig};

    fn http_web() -> Arc<Web> {
        Web::new(WebConfig {
            backend: Backend::Http,
            engine: None,
            chrome_path: None,
            display: Display::Headless,
            profile: std::env::temp_dir(),
        })
    }

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments,
            extra_content: None,
        }
    }

    fn text_of(result: &ToolResultMessage) -> String {
        result.plain_text()
    }

    #[test]
    fn fetch_pages_through_a_cached_page() {
        let web = http_web();
        let content: String = (1..=5).map(|n| format!("line {n}\n")).collect();
        web.remember(
            "https://example.com/a",
            &FetchedPage {
                url: "https://example.com/a".into(),
                title: "A".into(),
                content,
            },
        );
        let tool = FetchTool::new(web);
        let cancel = CancelToken::new();
        let ctx = ToolContext::new(std::env::temp_dir());
        let first = tool.execute(
            &call(
                "fetch",
                json!({ "url": "https://example.com/a", "limit": 2 }),
            ),
            &ctx,
            &mut |_| {},
            &cancel,
        );
        assert!(!first.is_error);
        assert_eq!(
            text_of(&first),
            "URL: https://example.com/a\nTitle: A\n\nline 1\nline 2\n\n[Showing lines 1-2 of 5. Use offset=3 to continue.]"
        );
        let rest = tool.execute(
            &call(
                "fetch",
                json!({ "url": "https://example.com/a", "offset": 3 }),
            ),
            &ctx,
            &mut |_| {},
            &cancel,
        );
        assert_eq!(
            text_of(&rest),
            "URL: https://example.com/a\nTitle: A\n\nline 3\nline 4\nline 5"
        );
    }

    #[test]
    fn fetch_refuses_other_schemes_and_bad_urls() {
        let tool = FetchTool::new(http_web());
        let cancel = CancelToken::new();
        let ctx = ToolContext::new(std::env::temp_dir());
        for url in ["file:///etc/passwd", "not a url"] {
            let result = tool.execute(
                &call("fetch", json!({ "url": url })),
                &ctx,
                &mut |_| {},
                &cancel,
            );
            assert!(result.is_error, "{url}");
        }
    }

    #[test]
    fn search_without_a_browser_explains_itself() {
        let tool = WebSearchTool::new(http_web());
        let result = tool.execute(
            &call("web_search", json!({ "query": "rust" })),
            &ToolContext::new(std::env::temp_dir()),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
    }
}
