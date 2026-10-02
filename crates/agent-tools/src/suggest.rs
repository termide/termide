//! `suggest_command`: offer the user a command to run by hand.
//!
//! The model puts one command on a card in the panel and waits: nothing runs
//! until the user picks `[Run]` on it. This is the path a blocked call takes —
//! a refusal tells the model to say what it needs run, and this turns that
//! into something the user does rather than a line of text they must retype.
//! It is also how the model hands over a command it should not run itself:
//! publishing, anything that needs credentials, anything the user said to
//! leave to them.
//!
//! The command is shown exactly as it would run, in full. Picking `[Run]`
//! runs it through the same shell path as a command the user typed after `$`,
//! so what reaches the shell is what was on the card. A run no one watches (a
//! subagent, headless mode) has nobody to confirm, and the model is told so.

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Suggestion, SuggestionReply, Tool, ToolCall, ToolContext, ToolResultMessage,
    ToolUpdate,
};

use crate::args::{optional_str, required_str};

/// The `suggest_command` tool.
#[derive(Debug, Default)]
pub struct SuggestCommandTool;

impl Tool for SuggestCommandTool {
    fn name(&self) -> &str {
        "suggest_command"
    }

    fn description(&self) -> &str {
        "Offer the user a shell command to run by hand, and wait for them to \
confirm it. Use it when a call was blocked or you should not run something \
yourself but the user plausibly will: publishing, anything that needs their \
credentials or their judgement, a destructive step you were told to leave to \
them. The command is shown on a card in full, exactly as it would run, with \
`[Run]`, `[Edit first]`, `[Copy]` and `[Dismiss]`; nothing runs unless they \
pick `[Run]`. Do not use it to ask permission for a call you could make with \
the tools you have, and do not offer a command the permission rules deny — \
the card then withholds `[Run]` and only offers the text."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command line, exactly as it should run; shown in full"
                },
                "why": {
                    "type": "string",
                    "description": "Why you offer it and what it does; shown to the user, never run"
                }
            },
            "required": ["command"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("offer the user a command to run by hand, and wait for them to confirm it")
    }

    fn prompt_guidelines(&self) -> &[&str] {
        &[
            "Use `suggest_command` when a call was blocked or the user should run it \
themselves; put the exact command in `command` and the reason in `why`. Never \
run what you suggest, and never suggest a command to reach the same outcome a \
denial already refused by another route.",
        ]
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let command = match required_str(call, "command") {
            Ok(command) => command.trim().to_string(),
            Err(message) => return ToolResultMessage::error(call, message),
        };
        if command.is_empty() {
            return ToolResultMessage::error(call, "`command` is empty");
        }
        let why = optional_str(call, "why")
            .ok()
            .flatten()
            .unwrap_or_default()
            .trim()
            .to_string();
        let suggestion = Suggestion { command, why };
        let Some(suggester) = &ctx.suggester else {
            return ToolResultMessage::error(
                call,
                "No user is available to confirm a command in this run. Do not offer one; \
say in your answer what you would have run and why, and wait for the user.",
            );
        };
        match suggester.suggest(suggestion.clone()) {
            // The user confirmed it: run it now, through the same runner a
            // typed `$` command uses, and give the model what it printed. The command
            // is not handed to `bash` as a tool call of its own — the loop is
            // busy with this one — so the runner is what reaches the shell.
            SuggestionReply::Run => match &ctx.shell_run {
                Some(run) => match run.run(&suggestion.command, cancel) {
                    Ok(out) => {
                        let body = if out.text.trim().is_empty() {
                            "(no output)".to_string()
                        } else {
                            out.text.clone()
                        };
                        let message = ToolResultMessage::text(
                            call,
                            format!("The user ran it. Its output:\n{body}"),
                        )
                        .with_details(json!({
                            "ran": suggestion.command,
                            "failed": out.failed,
                        }));
                        if out.failed {
                            ToolResultMessage {
                                is_error: true,
                                ..message
                            }
                        } else {
                            message
                        }
                    }
                    Err(error) => ToolResultMessage::error(
                        call,
                        format!("The user confirmed it but it could not run: {error}"),
                    ),
                },
                // Nothing to run it with. Say so rather than pretend: the
                // model must not conclude the command took effect.
                None => ToolResultMessage::error(
                    call,
                    format!(
                        "The user confirmed `{}`, but this run has no way to execute a \
command. Tell them it did not run.",
                        suggestion.command
                    ),
                ),
            },
            SuggestionReply::Edit => ToolResultMessage::text(
                call,
                "The user took the command to their input to change it before running it. \
Wait for them; do not run it yourself.",
            )
            .with_details(json!({ "edited": suggestion.command })),
            SuggestionReply::Copied => ToolResultMessage::text(
                call,
                "The user copied the command and did not run it. Wait for them.",
            )
            .with_details(json!({ "copied": suggestion.command })),
            SuggestionReply::Declined => ToolResultMessage::error(
                call,
                "The user dismissed the command and did not run it. Do not offer it again; \
wait for their next message.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use termide_agent_core::{question_channel, suggestion_channel, ShellOutput, ShellRunner};

    fn call(arguments: Value) -> ToolCall {
        ToolCall {
            id: "s1".into(),
            name: "suggest_command".into(),
            arguments,
            extra_content: None,
        }
    }

    /// A context whose card a thread answers with `reply`. The tool blocks
    /// until it is answered, so a test that has its result already knows the
    /// thread ran. `shell_run` records what reached the shell.
    fn answered(reply: SuggestionReply) -> ToolContext {
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        std::thread::spawn(move || {
            if let Ok(envelope) = rx.recv() {
                let _ = envelope.reply.send(reply);
            }
        });
        ToolContext {
            cwd: "/".into(),
            asker: None,
            suggester: Some(suggester),
            shell_run: Some(ShellRunner::new(|command, _| {
                Ok(ShellOutput {
                    text: format!("ran:{command}"),
                    failed: false,
                })
            })),
            session: None,
        }
    }

    /// The same, with nothing to run the command with.
    fn answered_without_shell(reply: SuggestionReply) -> ToolContext {
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        std::thread::spawn(move || {
            if let Ok(envelope) = rx.recv() {
                let _ = envelope.reply.send(reply);
            }
        });
        ToolContext {
            cwd: "/".into(),
            asker: None,
            suggester: Some(suggester),
            shell_run: None,
            session: None,
        }
    }

    #[test]
    fn with_no_one_to_confirm_the_model_is_told_to_say_it_instead() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "git push", "why": "publish" })),
            &ToolContext::new("/"),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("No user is available"));
    }

    #[test]
    fn an_empty_command_is_refused() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "   " })),
            &answered(SuggestionReply::Run),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("empty"));
    }

    #[test]
    fn a_missing_command_is_refused() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "why": "x" })),
            &answered(SuggestionReply::Run),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("missing required argument"));
    }

    /// The command reaches the card trimmed of what the model padded it with.
    #[test]
    fn the_command_shows_trimmed() {
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        let ctx = ToolContext {
            cwd: "/".into(),
            asker: None,
            suggester: Some(suggester),
            shell_run: Some(ShellRunner::new(|command, _| {
                Ok(ShellOutput {
                    text: format!("ran:{command}"),
                    failed: false,
                })
            })),
            session: None,
        };
        let answerer = std::thread::spawn(move || {
            let envelope = rx.recv().unwrap();
            assert_eq!(envelope.suggestion.command, "git push");
            assert_eq!(envelope.suggestion.why, "publish it");
            envelope.reply.send(SuggestionReply::Run).unwrap();
        });
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "  git push  ", "why": "  publish it  " })),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        answerer.join().unwrap();
        assert!(!result.is_error);
    }

    /// The card's answer reaches the model, and only `[Run]` says it ran.
    #[test]
    fn running_reports_the_command_ran() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "git push" })),
            &answered(SuggestionReply::Run),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(!result.is_error);
        assert!(result.plain_text().contains("The user ran it"));
        assert!(result.plain_text().contains("git push"));
        assert_eq!(result.details.unwrap()["ran"], json!("git push"));
    }

    #[test]
    fn editing_reports_the_user_took_it_to_their_input() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "git push" })),
            &answered(SuggestionReply::Edit),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(!result.is_error);
        assert!(result.plain_text().contains("to change it"));
        assert_eq!(result.details.unwrap()["edited"], json!("git push"));
    }

    #[test]
    fn copying_reports_it_did_not_run() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "ls" })),
            &answered(SuggestionReply::Copied),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(!result.is_error);
        assert!(result.plain_text().contains("did not run"));
    }

    /// A declined card is an error, so the model does not offer it again.
    #[test]
    fn declining_tells_the_model_to_wait() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "git push" })),
            &answered(SuggestionReply::Declined),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("Do not offer it again"));
    }

    /// A question tool and a suggestion tool coexist: the asker is not
    /// disturbed by a suggestion, nor the reverse.
    #[test]
    fn the_suggester_is_independent_of_the_asker() {
        let (asker, _qrx) = question_channel(CancelToken::new());
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        let ctx = ToolContext {
            cwd: "/".into(),
            asker: Some(asker),
            suggester: Some(suggester),
            shell_run: Some(ShellRunner::new(|command, _| {
                Ok(ShellOutput {
                    text: format!("ran:{command}"),
                    failed: false,
                })
            })),
            session: None,
        };
        let answerer = std::thread::spawn(move || {
            let envelope = rx.recv().unwrap();
            envelope.reply.send(SuggestionReply::Copied).unwrap();
        });
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "ls" })),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        answerer.join().unwrap();
        assert!(!result.is_error);
        assert!(result.plain_text().contains("copied"));
    }

    /// `[Run]` puts the exact command on the card through the runner, and the
    /// output the runner reported reaches the model.
    #[test]
    fn running_goes_through_the_runner_with_the_exact_command() {
        let seen = Arc::<std::sync::Mutex<Vec<String>>>::default();
        let recorder = Arc::clone(&seen);
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        std::thread::spawn(move || {
            if let Ok(envelope) = rx.recv() {
                let _ = envelope.reply.send(SuggestionReply::Run);
            }
        });
        let ctx = ToolContext {
            cwd: "/".into(),
            asker: None,
            suggester: Some(suggester),
            shell_run: Some(ShellRunner::new(move |command, _| {
                recorder.lock().unwrap().push(command.to_string());
                Ok(ShellOutput {
                    text: "posted".to_string(),
                    failed: false,
                })
            })),
            session: None,
        };
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "gh issue comment 59" })),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        assert_eq!(seen.lock().unwrap().as_slice(), ["gh issue comment 59"]);
        assert!(!result.is_error);
        assert!(result.plain_text().contains("posted"));
    }

    /// A runner that could not start the shell is an error, so the model does
    /// not report a command that never ran.
    #[test]
    fn a_runner_failure_is_an_error() {
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        std::thread::spawn(move || {
            if let Ok(envelope) = rx.recv() {
                let _ = envelope.reply.send(SuggestionReply::Run);
            }
        });
        let ctx = ToolContext {
            cwd: "/".into(),
            asker: None,
            suggester: Some(suggester),
            shell_run: Some(ShellRunner::new(|_, _| Err("no shell".to_string()))),
            session: None,
        };
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "ls" })),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("could not run"));
    }

    /// A confirmed command with no runner at all must not read as done.
    #[test]
    fn a_confirmed_command_with_no_runner_says_it_did_not_run() {
        let result = SuggestCommandTool.execute(
            &call(json!({ "command": "git push" })),
            &answered_without_shell(SuggestionReply::Run),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("did not run"));
    }

    /// The tool is offered to the model with a snippet and a guideline, so it
    /// knows when to reach for it.
    #[test]
    fn the_prompt_lists_it() {
        assert!(SuggestCommandTool.prompt_snippet().is_some());
        assert!(!SuggestCommandTool.prompt_guidelines().is_empty());
        assert!(SuggestCommandTool.description().contains("[Run]"));
    }
}
