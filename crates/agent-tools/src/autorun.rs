//! `suggest_goal` and `suggest_loop`: offer the user unattended work.
//!
//! The model puts a `/goal` or a `/loop` on a card in the panel and waits:
//! nothing starts until the user picks `[Start]` on it, and then it starts as
//! if they had typed the line, so the work stays theirs to stop. This is what
//! the model reaches for instead of writing a `/goal …` line out for the user
//! to paste. The offer goes through the same channel as `suggest_command`'s;
//! a run no one watches (a subagent, headless mode) has nobody to start it,
//! and the model is told so.

use serde_json::{json, Value};
use termide_agent_core::{
    format_interval, parse_interval, CancelToken, Suggestion, SuggestionKind, SuggestionReply,
    Tool, ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

use crate::args::{optional_str, required_str};

/// The `suggest_goal` tool.
#[derive(Debug, Default)]
pub struct SuggestGoalTool;

/// The `suggest_loop` tool.
#[derive(Debug, Default)]
pub struct SuggestLoopTool;

const WHY: &str = "Why you offer it; shown to the user";

impl Tool for SuggestGoalTool {
    fn name(&self) -> &str {
        "suggest_goal"
    }

    fn description(&self) -> &str {
        &ToolText::seed("suggest_goal").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The outcome to reach, as a judge can check it from the session"
                },
                "why": { "type": "string", "description": WHY }
            },
            "required": ["goal"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("suggest_goal").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("suggest_goal").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let goal = match required_text(call, "goal") {
            Ok(goal) => goal,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        offer(call, ctx, SuggestionKind::Goal, goal)
    }
}

impl Tool for SuggestLoopTool {
    fn name(&self) -> &str {
        "suggest_loop"
    }

    fn description(&self) -> &str {
        &ToolText::seed("suggest_loop").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The message sent to you on each run, complete in itself"
                },
                "interval": {
                    "type": "string",
                    "description": "The wait between runs: `30s`, `5m`, `2h`; leave it out to run again as soon as each run ends"
                },
                "why": { "type": "string", "description": WHY }
            },
            "required": ["prompt"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("suggest_loop").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("suggest_loop").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let prompt = match required_text(call, "prompt") {
            Ok(prompt) => prompt,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let interval = match optional_str(call, "interval") {
            Ok(Some(token)) if !token.trim().is_empty() => match parse_interval(token.trim()) {
                Some(every) => Some(every),
                None => {
                    return ToolResultMessage::error(
                        call,
                        format!(
                            "`interval` `{}` is not a duration: give a number with `s`, `m` or `h` \
(`30s`, `5m`, `2h`), or leave it out",
                            token.trim()
                        ),
                    )
                }
            },
            Ok(_) => None,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        offer(call, ctx, SuggestionKind::Loop { interval }, prompt)
    }
}

/// `name`, trimmed, refused when missing or empty.
fn required_text(call: &ToolCall, name: &str) -> Result<String, String> {
    let text = required_str(call, name)?.trim().to_string();
    if text.is_empty() {
        return Err(format!("`{name}` is empty"));
    }
    Ok(text)
}

/// Put the offer on the card and tell the model how the user answered.
fn offer(
    call: &ToolCall,
    ctx: &ToolContext,
    kind: SuggestionKind,
    text: String,
) -> ToolResultMessage {
    let what = match kind {
        SuggestionKind::Loop { .. } => "loop",
        _ => "goal",
    };
    let Some(suggester) = &ctx.suggester else {
        return ToolResultMessage::error(
            call,
            format!(
                "No user is available to start a {what} in this run. Do not offer one; carry on \
with the task as it stands."
            ),
        );
    };
    let why = optional_str(call, "why")
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string();
    let suggestion = Suggestion {
        kind,
        command: text,
        why,
    };
    let typed = suggestion.typed();
    match suggester.suggest(suggestion) {
        SuggestionReply::Run => {
            let text = match kind {
                SuggestionKind::Loop { interval } => {
                    let then = match interval {
                        Some(every) => format!("again every {}", format_interval(every)),
                        None => "again as soon as each run ends".to_string(),
                    };
                    format!(
                        "The user started the loop. Its prompt comes to you as their message \
now, then {then}."
                    )
                }
                _ => "The user started the goal. Its first turn comes to you as their message \
once this run ends, and after each turn a judge checks whether it is reached. Finish the step you \
are on and end your turn."
                    .to_string(),
            };
            ToolResultMessage::text(call, text).with_details(json!({ "started": typed }))
        }
        SuggestionReply::Edit => ToolResultMessage::text(
            call,
            format!(
                "The user took the `{typed}` line to their input to change it before starting \
it. Wait for them; do not offer it again."
            ),
        )
        .with_details(json!({ "edited": typed })),
        SuggestionReply::Copied | SuggestionReply::Declined => ToolResultMessage::text(
            call,
            format!(
                "The user did not start the {what}. Do not offer it again; carry on with their \
request as it stands."
            ),
        )
        .with_details(json!({ "declined": typed })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use termide_agent_core::suggestion_channel;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "g1".into(),
            name: name.into(),
            arguments,
            extra_content: None,
        }
    }

    /// A context whose card a thread answers with `reply`, handing back what
    /// the card showed.
    fn answered(
        reply: SuggestionReply,
    ) -> (ToolContext, std::thread::JoinHandle<Option<Suggestion>>) {
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        let answerer = std::thread::spawn(move || {
            let envelope = rx.recv().ok()?;
            let _ = envelope.reply.send(reply);
            Some(envelope.suggestion)
        });
        let ctx = ToolContext {
            suggester: Some(suggester),
            ..ToolContext::new("/")
        };
        (ctx, answerer)
    }

    fn run(tool: &dyn Tool, arguments: Value, ctx: &ToolContext) -> ToolResultMessage {
        tool.execute(
            &call(tool.name(), arguments),
            ctx,
            &mut |_| {},
            &CancelToken::new(),
        )
    }

    #[test]
    fn with_no_one_to_start_it_the_model_is_told_to_carry_on() {
        let result = run(
            &SuggestGoalTool,
            json!({ "goal": "build green" }),
            &ToolContext::new("/"),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("No user is available"));
    }

    #[test]
    fn an_empty_goal_or_prompt_is_refused() {
        let (ctx, _) = answered(SuggestionReply::Run);
        assert!(run(&SuggestGoalTool, json!({ "goal": "  " }), &ctx).is_error);
        assert!(run(&SuggestLoopTool, json!({}), &ctx).is_error);
    }

    /// The goal reaches the card trimmed, as a goal, and starting it says
    /// when its first turn comes.
    #[test]
    fn a_started_goal_is_reported_with_its_line() {
        let (ctx, answerer) = answered(SuggestionReply::Run);
        let result = run(
            &SuggestGoalTool,
            json!({ "goal": "  the build is green  ", "why": " many steps " }),
            &ctx,
        );
        let shown = answerer.join().unwrap().unwrap();
        assert_eq!(shown.kind, SuggestionKind::Goal);
        assert_eq!(shown.command, "the build is green");
        assert_eq!(shown.why, "many steps");
        assert!(!result.is_error);
        assert!(result.plain_text().contains("started the goal"));
        assert_eq!(
            result.details.unwrap()["started"],
            json!("/goal the build is green")
        );
    }

    /// A loop's interval reaches the card parsed, and a bad one is refused
    /// before any card shows.
    #[test]
    fn a_loop_carries_its_interval() {
        let (ctx, answerer) = answered(SuggestionReply::Run);
        let result = run(
            &SuggestLoopTool,
            json!({ "prompt": "check CI", "interval": "5m" }),
            &ctx,
        );
        let shown = answerer.join().unwrap().unwrap();
        assert_eq!(
            shown.kind,
            SuggestionKind::Loop {
                interval: Some(Duration::from_secs(300))
            }
        );
        assert!(result.plain_text().contains("again every 5m"));

        let (ctx, answerer) = answered(SuggestionReply::Run);
        let result = run(
            &SuggestLoopTool,
            json!({ "prompt": "check CI", "interval": "soon" }),
            &ctx,
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("not a duration"));
        drop(ctx);
        assert!(
            answerer.join().unwrap().is_none(),
            "no card for a bad interval"
        );

        let (ctx, answerer) = answered(SuggestionReply::Run);
        let result = run(&SuggestLoopTool, json!({ "prompt": "next item" }), &ctx);
        assert_eq!(
            answerer.join().unwrap().unwrap().kind,
            SuggestionKind::Loop { interval: None }
        );
        assert!(result.plain_text().contains("as soon as each run ends"));
    }

    /// Editing and declining are not errors: the model waits, or carries
    /// on, but does not offer the same thing again.
    #[test]
    fn edit_and_decline_tell_the_model_not_to_offer_again() {
        let (ctx, _) = answered(SuggestionReply::Edit);
        let result = run(&SuggestGoalTool, json!({ "goal": "done" }), &ctx);
        assert!(!result.is_error);
        assert!(result.plain_text().contains("`/goal done`"));
        assert!(result.plain_text().contains("do not offer it again"));

        let (ctx, _) = answered(SuggestionReply::Declined);
        let result = run(&SuggestLoopTool, json!({ "prompt": "poll" }), &ctx);
        assert!(!result.is_error);
        assert!(result.plain_text().contains("did not start the loop"));
    }

    #[test]
    fn the_prompt_lists_them() {
        for tool in [&SuggestGoalTool as &dyn Tool, &SuggestLoopTool] {
            assert!(tool.prompt_snippet().is_some());
            assert!(!tool.prompt_guidelines().is_empty());
            assert!(tool.description().contains("[Start]"));
        }
    }
}
