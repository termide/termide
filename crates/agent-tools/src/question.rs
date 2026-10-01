//! `question`: ask the user and wait for the answer.
//!
//! The model puts one to four questions, each with a few choices — one to
//! pick, or several with `multi_select` — and the user may always type an
//! answer of their own instead. The panel shows them one after another as a
//! card in the agent panel, the way Claude Code's `AskUserQuestion` and
//! OpenCode's `question` do. A run no one watches (a subagent, headless mode)
//! has nobody to ask, and the model is told to decide on its own.

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Question, QuestionAnswer, QuestionOption, QuestionReply, Tool, ToolCall,
    ToolContext, ToolResultMessage, ToolUpdate,
};

/// At most this many questions in one call.
pub const MAX_QUESTIONS: usize = 4;
/// At most this many choices per question, so every row of the card — the
/// choices, the typed answer and a multi-select's confirmation — keeps a digit.
pub const MAX_OPTIONS: usize = 6;

/// The `question` tool.
#[derive(Debug, Default)]
pub struct QuestionTool;

impl Tool for QuestionTool {
    fn name(&self) -> &str {
        "question"
    }

    fn description(&self) -> &str {
        "Ask the user one or more questions and wait for the answers. Use it when a decision is \
the user's to make and you cannot settle it from the request, the code or a sensible default: \
choosing between approaches, clarifying an ambiguous requirement, confirming a preference. \
Each question offers a few choices; the user can always type an answer of their own instead, \
so do not add an \"Other\" choice. Put the recommended choice first and say so in its label. \
Do not use it to ask for permission to run a tool, or to ask whether you may proceed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_QUESTIONS,
                    "description": "The questions, asked one after another",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question, ending with a question mark"
                            },
                            "header": {
                                "type": "string",
                                "description": "A very short label for the topic (at most 12 characters), such as \"Approach\""
                            },
                            "options": {
                                "type": "array",
                                "maxItems": MAX_OPTIONS,
                                "description": "The choices; leave empty for a question answered only in the user's own words",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "The choice itself, in 1-5 words"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "What choosing it means, its trade-off"
                                        }
                                    },
                                    "required": ["label"]
                                }
                            },
                            "multi_select": {
                                "type": "boolean",
                                "description": "Whether several choices can be picked together"
                            }
                        },
                        "required": ["question"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some("ask the user to choose between options or answer in their own words")
    }

    fn prompt_guidelines(&self) -> &[&str] {
        &["Use `question` only when you are blocked on a decision that is the user's to make; otherwise pick the sensible default and say which you chose."]
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let questions = match parse_questions(&call.arguments) {
            Ok(questions) => questions,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let Some(asker) = &ctx.asker else {
            return ToolResultMessage::error(
                call,
                "No user is available to answer questions in this run. Decide on your own, \
and state the assumptions you made in your final answer.",
            );
        };
        match asker.ask(questions.clone()) {
            QuestionReply::Answered(answers) => {
                ToolResultMessage::text(call, answer_text(&questions, &answers))
                    .with_details(answer_details(&questions, &answers))
            }
            QuestionReply::Declined => ToolResultMessage::error(
                call,
                "The user declined to answer the questions. Do not ask them again; wait for \
the user's next message.",
            ),
        }
    }
}

/// The questions of a call, checked against the schema's limits.
fn parse_questions(arguments: &Value) -> Result<Vec<Question>, String> {
    let Some(list) = arguments.get("questions").and_then(Value::as_array) else {
        return Err("missing required argument `questions` (an array)".into());
    };
    if list.is_empty() || list.len() > MAX_QUESTIONS {
        return Err(format!(
            "`questions` must hold 1 to {MAX_QUESTIONS} questions"
        ));
    }
    list.iter()
        .enumerate()
        .map(|(index, item)| {
            let n = index + 1;
            let text = |key: &str| {
                item.get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("")
                    .to_string()
            };
            let question = text("question");
            if question.is_empty() {
                return Err(format!("question {n} has no `question` text"));
            }
            let options = match item.get("options") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(options)) => options
                    .iter()
                    .map(|option| {
                        let field = |key: &str| {
                            option
                                .get(key)
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .unwrap_or("")
                                .to_string()
                        };
                        let label = field("label");
                        if label.is_empty() {
                            return Err(format!("question {n} has a choice with no `label`"));
                        }
                        Ok(QuestionOption {
                            label,
                            description: field("description"),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => return Err(format!("question {n}: `options` must be an array")),
            };
            if options.len() > MAX_OPTIONS {
                return Err(format!(
                    "question {n} offers {} choices; at most {MAX_OPTIONS}",
                    options.len()
                ));
            }
            let multi_select = item
                .get("multi_select")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && !options.is_empty();
            Ok(Question {
                header: text("header"),
                question,
                options,
                multi_select,
            })
        })
        .collect()
}

/// The answers as the model reads them: each question with what was picked
/// and what was typed.
fn answer_text(questions: &[Question], answers: &[QuestionAnswer]) -> String {
    let mut text = String::from("The user answered:");
    for (question, answer) in questions.iter().zip(answers) {
        let mut parts: Vec<String> = answer.chosen.clone();
        if let Some(custom) = &answer.custom {
            parts.push(format!("in their own words: {custom}"));
        }
        let reply = if parts.is_empty() {
            "(no answer)".to_string()
        } else {
            parts.join("; ")
        };
        text.push_str(&format!("\n- {} → {reply}", question.question));
    }
    text
}

/// The answers for the transcript, which shows them under the call.
fn answer_details(questions: &[Question], answers: &[QuestionAnswer]) -> Value {
    let answers: Vec<Value> = questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| {
            json!({
                "header": question.header,
                "question": question.question,
                "chosen": answer.chosen,
                "custom": answer.custom,
            })
        })
        .collect();
    json!({ "answers": answers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_agent_core::question_channel;

    fn call(arguments: Value) -> ToolCall {
        ToolCall {
            id: "q1".into(),
            name: "question".into(),
            arguments,
            extra_content: None,
        }
    }

    fn two_questions() -> Value {
        json!({ "questions": [
            {
                "header": "Approach",
                "question": "Which approach?",
                "options": [
                    { "label": "Channel (recommended)", "description": "like permissions" },
                    { "label": "Shared slot" }
                ]
            },
            {
                "question": "Which crates?",
                "options": [{ "label": "core" }, { "label": "ui" }],
                "multi_select": true
            }
        ]})
    }

    #[test]
    fn questions_are_checked_against_the_limits() {
        let parsed = parse_questions(&two_questions()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].header, "Approach");
        assert_eq!(parsed[0].options[0].description, "like permissions");
        assert!(!parsed[0].multi_select);
        assert!(parsed[1].multi_select);

        assert!(parse_questions(&json!({ "questions": [] })).is_err());
        assert!(parse_questions(&json!({ "questions": [{ "question": " " }] })).is_err());
        let many: Vec<Value> = (0..=MAX_OPTIONS)
            .map(|n| json!({ "label": n.to_string() }))
            .collect();
        assert!(
            parse_questions(&json!({ "questions": [{ "question": "?", "options": many }] }))
                .is_err()
        );
        // No choices: answered in the user's own words, never multi-select.
        let free = parse_questions(
            &json!({ "questions": [{ "question": "Name?", "multi_select": true }] }),
        )
        .unwrap();
        assert!(free[0].options.is_empty() && !free[0].multi_select);
    }

    #[test]
    fn with_no_one_to_ask_the_model_decides() {
        let result = QuestionTool.execute(
            &call(two_questions()),
            &ToolContext::new("/"),
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(result.is_error);
        assert!(result.plain_text().contains("Decide on your own"));
    }

    #[test]
    fn the_answers_come_back_as_text_and_details() {
        let (asker, rx) = question_channel(CancelToken::new());
        let ctx = ToolContext {
            cwd: "/".into(),
            asker: Some(asker),
            session: None,
        };
        let answerer = std::thread::spawn(move || {
            let envelope = rx.recv().unwrap();
            assert_eq!(envelope.questions.len(), 2);
            envelope
                .reply
                .send(QuestionReply::Answered(vec![
                    QuestionAnswer {
                        chosen: vec!["Channel (recommended)".into()],
                        custom: None,
                    },
                    QuestionAnswer {
                        chosen: vec!["core".into(), "ui".into()],
                        custom: Some("and app".into()),
                    },
                ]))
                .unwrap();
        });
        let result = QuestionTool.execute(
            &call(two_questions()),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        answerer.join().unwrap();
        assert!(!result.is_error);
        assert_eq!(
            result.plain_text(),
            "The user answered:\n- Which approach? → Channel (recommended)\n\
- Which crates? → core; ui; in their own words: and app"
        );
        let details = result.details.unwrap();
        assert_eq!(details["answers"][1]["chosen"], json!(["core", "ui"]));
        assert_eq!(details["answers"][1]["custom"], json!("and app"));
    }

    #[test]
    fn a_declined_question_tells_the_model_to_wait() {
        let (asker, rx) = question_channel(CancelToken::new());
        let ctx = ToolContext {
            cwd: "/".into(),
            asker: Some(asker),
            session: None,
        };
        let answerer = std::thread::spawn(move || {
            let envelope = rx.recv().unwrap();
            envelope.reply.send(QuestionReply::Declined).unwrap();
        });
        let result = QuestionTool.execute(
            &call(two_questions()),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        answerer.join().unwrap();
        assert!(result.is_error);
        assert!(result.plain_text().contains("declined"));
    }
}
