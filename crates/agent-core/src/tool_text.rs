//! What a built-in tool tells the model about itself: its description (in
//! the tool schema), its one-line snippet (in the system prompt's tool list)
//! and its guidelines (in the system prompt's guidelines). No prompt text
//! lives in code: each tool's texts are a data file, `assets/tools/<name>.md`,
//! seeded to `ai/tools/<name>.md` in the configuration directory, where the
//! user can read and change them.
//!
//! ```markdown
//! ---
//! snippet: read a file as numbered lines, paged with offset/limit
//! guideline.1: Use `read` instead of `cat`, `head` or `sed -n` to look at files.
//! ---
//! Read a text file. Returns lines prefixed with their 1-based line number. …
//! ```
//!
//! The body is the description; a tool may fill placeholders of its own in
//! it (`task` lists the agents in `{{agents}}`, see
//! [`Tool::render_description`]). Without `snippet` the tool stays out of the
//! system prompt's list; guidelines are numbered so they keep their order.
//! A tool reads its seed ([`ToolText::seed`]); the user's file, when it says
//! something else, replaces the texts through [`apply_tool_texts`]. Only the
//! configuration level counts, as for `system/`: a checked-out project must
//! not be able to reword what the agent is told its tools do.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use crate::cancel::CancelToken;
use crate::layers::split_front_matter;
use crate::message::{ToolCall, ToolResultMessage};
use crate::tool::{Tool, ToolContext, ToolRegistry, ToolUpdate};

/// The tool texts under an `ai` directory: `tools/<name>.md`.
pub const TOOLS_DIR: &str = "tools";

/// The shipped texts of every built-in tool, by name.
pub const SEED_TOOLS: [(&str, &str); 11] = [
    ("read", include_str!("../assets/tools/read.md")),
    ("edit", include_str!("../assets/tools/edit.md")),
    ("write", include_str!("../assets/tools/write.md")),
    ("bash", include_str!("../assets/tools/bash.md")),
    ("question", include_str!("../assets/tools/question.md")),
    (
        "suggest_command",
        include_str!("../assets/tools/suggest_command.md"),
    ),
    ("task", include_str!("../assets/tools/task.md")),
    ("skill", include_str!("../assets/tools/skill.md")),
    ("fetch", include_str!("../assets/tools/fetch.md")),
    ("web_search", include_str!("../assets/tools/web_search.md")),
    ("recall", include_str!("../assets/tools/recall.md")),
];

/// One tool's texts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolText {
    /// The description, with any placeholders the tool fills.
    pub description: String,
    pub snippet: Option<String>,
    pub guidelines: Vec<String>,
}

impl ToolText {
    /// Parse a `tools/<name>.md` file.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        let (fields, body) = split_front_matter(text);
        let snippet = fields
            .get("snippet")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let mut numbered: Vec<(u32, String)> = fields
            .iter()
            .filter_map(|(key, value)| {
                let number = key.strip_prefix("guideline.")?.trim().parse().ok()?;
                let value = value.trim();
                (!value.is_empty()).then(|| (number, value.to_string()))
            })
            .collect();
        numbered.sort_by_key(|(number, _)| *number);
        Self {
            description: body.trim().to_string(),
            snippet,
            guidelines: numbered.into_iter().map(|(_, g)| g).collect(),
        }
    }

    /// Whether `key` is one a tool text's front matter is read for:
    /// `snippet` or `guideline.<number>`.
    #[must_use]
    pub fn is_known_key(key: &str) -> bool {
        key == "snippet"
            || key
                .strip_prefix("guideline.")
                .is_some_and(|n| n.trim().parse::<u32>().is_ok())
    }

    /// The shipped texts of the tool `name`; empty for a name with no seed.
    #[must_use]
    pub fn seed(name: &str) -> &'static ToolText {
        static SEEDS: OnceLock<BTreeMap<&'static str, ToolText>> = OnceLock::new();
        static EMPTY: ToolText = ToolText {
            description: String::new(),
            snippet: None,
            guidelines: Vec::new(),
        };
        SEEDS
            .get_or_init(|| {
                SEED_TOOLS
                    .iter()
                    .map(|(name, text)| (*name, ToolText::from_file(text)))
                    .collect()
            })
            .get(name)
            .unwrap_or(&EMPTY)
    }
}

/// A tool whose texts come from the user's file rather than its seed.
struct Retexted {
    inner: Arc<dyn Tool>,
    /// The description with the tool's placeholders filled.
    description: String,
    text: ToolText,
}

impl Tool for Retexted {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        self.inner.parameters()
    }

    fn prompt_snippet(&self) -> Option<&str> {
        self.text.snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &self.text.guidelines
    }

    fn render_description(&self, template: &str) -> String {
        self.inner.render_description(template)
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

/// Give every tool in `tools` that `texts` names, and whose seed says
/// something else, the texts of the user's file. A tool keeps its own when
/// the file is its seed unchanged, or has no description.
pub fn apply_tool_texts(tools: &mut ToolRegistry, texts: &BTreeMap<String, ToolText>) {
    let replaced: Vec<Arc<dyn Tool>> = tools
        .iter()
        .filter_map(|tool| {
            let text = texts.get(tool.name())?;
            if text.description.is_empty() || text == ToolText::seed(tool.name()) {
                return None;
            }
            Some(Arc::new(Retexted {
                description: tool.render_description(&text.description),
                inner: Arc::clone(tool),
                text: text.clone(),
            }) as Arc<dyn Tool>)
        })
        .collect();
    for tool in replaced {
        tools.insert(tool);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Echo;

    impl Tool for Echo {
        fn name(&self) -> &str {
            "read"
        }
        fn description(&self) -> &str {
            &ToolText::seed("read").description
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        fn prompt_snippet(&self) -> Option<&str> {
            ToolText::seed("read").snippet.as_deref()
        }
        fn prompt_guidelines(&self) -> &[String] {
            &ToolText::seed("read").guidelines
        }
        fn render_description(&self, template: &str) -> String {
            template.replace("{{who}}", "echo")
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            _on_update: &mut dyn FnMut(ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            ToolResultMessage::text(call, "ran")
        }
    }

    #[test]
    fn every_seed_parses_with_a_description() {
        for (name, _) in SEED_TOOLS {
            assert!(!ToolText::seed(name).description.is_empty(), "{name}");
        }
        let recall = ToolText::seed("recall");
        assert_eq!(recall.guidelines.len(), 2);
        assert!(recall.guidelines[0].starts_with("Use `recall` before"));
        assert!(ToolText::seed("task").description.contains("{{agents}}"));
        assert!(ToolText::seed("nothing").description.is_empty());
    }

    #[test]
    fn guidelines_keep_their_numbers_order_and_a_missing_snippet_hides() {
        let text = ToolText::from_file(
            "---\nguideline.10: tenth\nguideline.2: second\nguideline.x: dropped\n---\nBody.\n",
        );
        assert_eq!(text.guidelines, ["second", "tenth"]);
        assert_eq!(text.snippet, None);
        assert_eq!(text.description, "Body.");
    }

    #[test]
    fn the_users_file_replaces_the_texts_and_keeps_the_tool() {
        let mut tools = ToolRegistry::new();
        tools.insert(Arc::new(Echo));
        let mut texts = BTreeMap::new();
        // The seed unchanged leaves the tool as it is.
        texts.insert("read".to_string(), ToolText::seed("read").clone());
        apply_tool_texts(&mut tools, &texts);
        assert_eq!(
            tools.get("read").unwrap().description(),
            ToolText::seed("read").description
        );

        texts.insert(
            "read".to_string(),
            ToolText::from_file(
                "---\nsnippet: look\nguideline.1: Be brief.\n---\nRead, by {{who}}.\n",
            ),
        );
        apply_tool_texts(&mut tools, &texts);
        let tool = tools.get("read").unwrap();
        assert_eq!(tool.description(), "Read, by echo.");
        assert_eq!(tool.prompt_snippet(), Some("look"));
        assert_eq!(tool.prompt_guidelines(), ["Be brief."]);
        let call = ToolCall {
            id: "c".into(),
            name: "read".into(),
            arguments: json!({}),
            extra_content: None,
        };
        let result = tool.execute(
            &call,
            &ToolContext::new("/"),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert_eq!(result.plain_text(), "ran");
        assert_eq!(tools.len(), 1);
    }
}
