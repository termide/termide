//! `checklist`: the steps of the work at hand, which the user sees above the
//! panel's input and the model is reminded of while steps remain.
//!
//! Each call writes the whole list, as every agent's tool of the kind does
//! (Claude Code's `TodoWrite`, Codex's `update_plan`): no ids to get wrong,
//! reordering and splitting in one call, the empty list clearing it. The
//! list is kept for the session ([`checklist::remember`]) and goes to the
//! panel in the result's details; see `doc/en/agent-design.md`, 4c.

use serde_json::{json, Value};
use termide_agent_core::checklist::{self, ChecklistItem, ChecklistStatus, DETAILS_KEY};
use termide_agent_core::ToolUpdate;
use termide_agent_core::{CancelToken, Tool, ToolCall, ToolContext, ToolResultMessage, ToolText};

/// The `checklist` tool.
pub struct ChecklistTool;

impl Tool for ChecklistTool {
    fn name(&self) -> &str {
        "checklist"
    }

    fn description(&self) -> &str {
        &ToolText::seed("checklist").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "description": "The whole list, in order; empty to clear it",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "The step, in a few words"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "done"]
                            }
                        },
                        "required": ["text", "status"]
                    }
                }
            },
            "required": ["items"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("checklist").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("checklist").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let items = match parse_items(&call.arguments) {
            Ok(items) => items,
            Err(error) => return ToolResultMessage::error(call, error),
        };
        if let Some(session) = ctx.session.as_ref().and_then(|view| view.id.as_deref()) {
            checklist::remember(session, &items);
        }
        let text = if items.is_empty() {
            "Checklist cleared.".to_string()
        } else if checklist::all_done(&items) {
            format!("Every step is done:\n{}", checklist::render(&items))
        } else {
            format!("Checklist:\n{}", checklist::render(&items))
        };
        let mut result = ToolResultMessage::text(call, text);
        result.details = Some(json!({ DETAILS_KEY: items }));
        result
    }
}

/// The list in the call's `items`. A step with no text is an error rather
/// than dropped, so the model's list and the one shown never differ;
/// `completed` reads as `done`, the word Codex and ACP use.
fn parse_items(arguments: &Value) -> Result<Vec<ChecklistItem>, String> {
    let Some(items) = arguments.get("items").and_then(Value::as_array) else {
        return Err("`items` must be the whole list, an array of {text, status}".to_string());
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let text = item["text"].as_str().map(str::trim).unwrap_or("");
            if text.is_empty() {
                return Err(format!("step {} has no text", index + 1));
            }
            let status = match item["status"].as_str() {
                Some("pending") => ChecklistStatus::Pending,
                Some("in_progress") => ChecklistStatus::InProgress,
                Some("done" | "completed") => ChecklistStatus::Done,
                other => {
                    return Err(format!(
                        "step {} has status {}; use pending, in_progress or done",
                        index + 1,
                        other.map_or_else(|| "none".to_string(), |s| format!("`{s}`"))
                    ))
                }
            };
            Ok(ChecklistItem::new(text, status))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: Value) -> ToolCall {
        ToolCall {
            id: "c1".to_string(),
            name: "checklist".to_string(),
            arguments,
            extra_content: None,
        }
    }

    fn run(arguments: Value) -> ToolResultMessage {
        ChecklistTool.execute(
            &call(arguments),
            &ToolContext::new("/tmp"),
            &mut |_| {},
            &CancelToken::new(),
        )
    }

    #[test]
    fn the_list_comes_back_to_the_model_and_to_the_panel() {
        let result = run(json!({ "items": [
            { "text": "Read", "status": "done" },
            { "text": " Fix ", "status": "in_progress" },
            { "text": "Test", "status": "pending" },
        ]}));
        assert!(!result.is_error);
        assert_eq!(
            result.plain_text(),
            "Checklist:\n- [x] Read\n- [>] Fix\n- [ ] Test"
        );
        assert_eq!(
            checklist::of_result(&result),
            Some(vec![
                ChecklistItem::new("Read", ChecklistStatus::Done),
                ChecklistItem::new("Fix", ChecklistStatus::InProgress),
                ChecklistItem::new("Test", ChecklistStatus::Pending),
            ])
        );
        let cleared = run(json!({ "items": [] }));
        assert_eq!(cleared.plain_text(), "Checklist cleared.");
        assert_eq!(checklist::of_result(&cleared), Some(Vec::new()));
    }

    #[test]
    fn a_malformed_list_is_refused_whole() {
        for arguments in [
            json!({}),
            json!({ "items": [{ "text": "", "status": "pending" }] }),
            json!({ "items": [{ "text": "a", "status": "started" }] }),
        ] {
            let result = run(arguments.clone());
            assert!(result.is_error, "{arguments}");
            assert_eq!(checklist::of_result(&result), None);
        }
    }
}
