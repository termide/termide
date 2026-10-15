//! Fitting a conversation into a budget by leaving out what matters least.
//!
//! A summary rewords the history through a model call, which takes time and
//! can go wrong; this cuts it down instead, by kinds of content in order of
//! how cheaply each is lost, oldest first within a kind, stopping as soon as
//! the history fits:
//!
//! 1. reasoning, which the answers and calls after it already carry out;
//! 2. the output of calls that only read (files, searches, look-only
//!    commands, web pages, recall), which the same call brings back;
//! 3. the output of every other call, cut to its start and end, where
//!    commands put their errors and totals;
//! 4. long arguments of calls (a file written, a text replaced), the path
//!    and the rest staying;
//! 5. the exploration as a whole, down to the decisions
//!    ([`crate::prune::prune_to_decisions`]);
//! 6. the oldest messages, a note saying how many went.
//!
//! Steps 1–4 go through the older part first and only then, if that is not
//! enough, through the most recent messages within `keep_tail`, which never
//! lose a message to steps 5 and 6. The user's messages, questions with
//! their answers and skills loaded are not touched before the last step.
//! Every call keeps its result, so the history stays
//! valid for any provider. The same input gives the same output, so a
//! session log can record the fitting and replay it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compaction::message_chars;
use crate::message::{AssistantContent, Message, ToolCall, ToolResultContent, Usage, UserMessage};
use crate::permissions::is_read_only_call;
use crate::prune::{prune_by, KEPT_TOOLS};

/// Characters a token is taken to hold when a budget in tokens is turned
/// into one in characters, as a ratio (`5 / 2` = 2.5). Lower than the usual
/// four on purpose: code and non-Latin text come out at 2.5–3 for the
/// tokenizers seen, and a budget that errs comes out too small, not too big.
const CHARS_PER_TOKEN: (u64, u64) = (5, 2);

/// A call output shorter than this stays, whatever the step: a stub would
/// save next to nothing.
const SHORT_OUTPUT: usize = 300;

/// What a cut output keeps at each end.
const OUTPUT_EDGE: usize = 600;

/// An argument longer than this is replaced by a note of its length.
const LONG_ARGUMENT: usize = 400;

/// The most the note on the oldest messages left out takes.
const NOTE_ROOM: usize = 80;

/// The steps that shorten content, taken message by message.
const SHORTENING: [FitStep; 4] = [
    FitStep::Reasoning,
    FitStep::ReadOutputs,
    FitStep::CommandOutputs,
    FitStep::CallArguments,
];

/// The characters a budget of `tokens` comes to, erring low.
#[must_use]
pub fn chars_for_tokens(tokens: u64) -> usize {
    usize::try_from(tokens.saturating_mul(CHARS_PER_TOKEN.0) / CHARS_PER_TOKEN.1)
        .unwrap_or(usize::MAX)
}

/// A step of the fitting, in the order they are taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitStep {
    Reasoning,
    ReadOutputs,
    CommandOutputs,
    CallArguments,
    Exploration,
    Oldest,
}

/// A history fitted into a budget.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    pub messages: Vec<Message>,
    /// For each message, the index of the one it came from in the input;
    /// `None` for the note on the oldest messages left out.
    pub origin: Vec<Option<usize>>,
    /// The steps taken, in order, each with the characters it freed.
    pub steps: Vec<(FitStep, usize)>,
    /// Whether the result is within the budget: not when the kept tail
    /// alone is over it.
    pub fits: bool,
}

impl Fitted {
    /// Whether anything was left out.
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.steps.is_empty()
    }
}

/// The characters `messages` come to, as [`message_chars`] counts them.
#[must_use]
pub fn chars(messages: &[Message]) -> usize {
    messages.iter().map(message_chars).sum()
}

/// `messages` cut down to `budget` characters at most, keeping the most
/// recent messages within `keep_tail` characters (the last one always)
/// whole. See the module documentation for what goes first.
#[must_use]
pub fn fit(messages: &[Message], budget: usize, keep_tail: usize) -> Fitted {
    if chars(messages) <= budget {
        return Fitted {
            messages: messages.to_vec(),
            origin: (0..messages.len()).map(Some).collect(),
            steps: Vec::new(),
            fits: true,
        };
    }
    let tail_at = tail_start(messages, keep_tail);
    let mut tail_size = chars(&messages[tail_at..]);
    let mut head: Vec<(Option<usize>, Message)> = messages[..tail_at]
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, message)| (Some(index), message))
        .collect();
    let mut tail: Vec<(Option<usize>, Message)> = messages[tail_at..]
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, message)| (Some(tail_at + index), message))
        .collect();
    let mut head_size = chars(&messages[..tail_at]);
    let calls: HashMap<String, ToolCall> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) => Some(assistant.tool_calls()),
            _ => None,
        })
        .flatten()
        .map(|call| (call.id.clone(), call.clone()))
        .collect();

    let mut steps: Vec<(FitStep, usize)> = Vec::new();
    // The kinds are cut in the older part first, then, if that is not
    // enough, in the kept tail too — shortened content anywhere costs less
    // than whole messages lost.
    for in_tail in [false, true] {
        for step in SHORTENING {
            let mut freed = 0;
            // The other part does not change while this one is cut.
            let other = if in_tail { head_size } else { tail_size };
            let (part, size) = if in_tail {
                (&mut tail, &mut tail_size)
            } else {
                (&mut head, &mut head_size)
            };
            for (_, message) in part.iter_mut() {
                if *size + other <= budget {
                    break;
                }
                let before = message_chars(message);
                shorten(message, step, &calls);
                let gone = before - message_chars(message);
                *size -= gone;
                freed += gone;
            }
            if freed > 0 {
                match steps.iter_mut().find(|(taken, _)| *taken == step) {
                    Some((_, total)) => *total += freed,
                    None => steps.push((step, freed)),
                }
            }
        }
    }
    // Reasoning alone in a message leaves it empty: it goes.
    for part in [&mut head, &mut tail] {
        part.retain(
            |(_, message)| !matches!(message, Message::Assistant(a) if a.content.is_empty()),
        );
    }
    let over = |head_size: usize| head_size + tail_size > budget;

    if over(head_size) {
        // The exploration goes from the oldest on, as little of it as
        // fits: the shortest older part whose pruning is enough.
        let cut = |at: usize| {
            let mut kept: Vec<(Option<usize>, Message)> =
                prune_by(&head[..at], |(_, message)| message)
                    .into_iter()
                    .map(|((origin, _), message)| (*origin, message))
                    .collect();
            kept.extend_from_slice(&head[at..]);
            let size: usize = kept.iter().map(|(_, message)| message_chars(message)).sum();
            (kept, size)
        };
        // Where the older part may end: never between a call and its result.
        let ends: Vec<usize> = (1..=head.len())
            .filter(|&at| at == head.len() || !matches!(head[at].1, Message::ToolResult(_)))
            .collect();
        let (mut low, mut high) = (0, ends.len().saturating_sub(1));
        while low < high {
            let middle = (low + high) / 2;
            if over(cut(ends[middle]).1) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let (pruned, size) = cut(ends.get(low).copied().unwrap_or(0));
        if size < head_size {
            steps.push((FitStep::Exploration, head_size - size));
            head = pruned;
            head_size = size;
        }
    }

    if over(head_size) {
        let mut dropped = 0;
        let mut freed = 0;
        let mut start = 0;
        // Room for the note that says how many went.
        while start < head.len() && over(head_size + NOTE_ROOM) {
            // A message goes with the results of its calls.
            let mut end = start + 1;
            while end < head.len() && matches!(head[end].1, Message::ToolResult(_)) {
                end += 1;
            }
            let size: usize = head[start..end]
                .iter()
                .map(|(_, message)| message_chars(message))
                .sum();
            head_size -= size;
            freed += size;
            dropped += end - start;
            start = end;
        }
        // The note takes the time of the first message it stands for, so
        // the same history always fits the same way.
        let since = head.first().map_or(0, |(_, message)| timestamp_of(message));
        head.drain(..start);
        if dropped > 0 {
            let mut note = UserMessage::text(format!(
                "[{dropped} earlier messages were left out to fit the context.]"
            ));
            note.timestamp = since;
            let note = Message::User(note);
            head_size += message_chars(&note);
            head.insert(0, (None, note));
            steps.push((FitStep::Oldest, freed));
        }
    }

    head.extend(tail);
    let (origin, mut messages): (Vec<Option<usize>>, Vec<Message>) = head.into_iter().unzip();
    if !steps.is_empty() {
        // A reply's usage measured the context before; left in place, the
        // next estimate would go on counting what was left out.
        for message in &mut messages {
            if let Message::Assistant(assistant) = message {
                assistant.usage = Usage::default();
            }
        }
    }
    Fitted {
        messages,
        origin,
        steps,
        fits: !over(head_size),
    }
}

fn timestamp_of(message: &Message) -> u64 {
    match message {
        Message::User(user) => user.timestamp,
        Message::Assistant(assistant) => assistant.timestamp,
        Message::ToolResult(result) => result.timestamp,
    }
}

/// Where the kept tail starts: the most recent messages within `keep`
/// characters, the last one always, never a result apart from its call.
fn tail_start(messages: &[Message], keep: usize) -> usize {
    let Some(mut start) = messages.len().checked_sub(1) else {
        return 0;
    };
    let mut size = message_chars(&messages[start]);
    while start > 0 {
        let next = size + message_chars(&messages[start - 1]);
        if next > keep {
            break;
        }
        size = next;
        start -= 1;
    }
    while start > 0 && matches!(messages[start], Message::ToolResult(_)) {
        start -= 1;
    }
    start
}

/// Take `step` on one message.
fn shorten(message: &mut Message, step: FitStep, calls: &HashMap<String, ToolCall>) {
    match (step, message) {
        (FitStep::Reasoning, Message::Assistant(assistant)) => {
            assistant
                .content
                .retain(|block| !matches!(block, AssistantContent::Thinking { .. }));
        }
        (FitStep::ReadOutputs, Message::ToolResult(result)) => {
            let reads = calls
                .get(&result.tool_call_id)
                .is_some_and(|call| !kept(&call.name) && is_read_only_call(call));
            let size = result_chars(&result.content);
            if reads && size > SHORT_OUTPUT {
                result.content = vec![ToolResultContent::Text {
                    text: format!(
                        "[{size} characters of output left out to fit the context; \
                         run the call again if you need them.]"
                    ),
                }];
            }
        }
        (FitStep::CommandOutputs, Message::ToolResult(result)) => {
            if kept(&result.tool_name) {
                return;
            }
            let text: String = result
                .content
                .iter()
                .filter_map(|block| match block {
                    ToolResultContent::Text { text } => Some(text.as_str()),
                    ToolResultContent::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let size = text.chars().count();
            if size > 2 * OUTPUT_EDGE + SHORT_OUTPUT {
                let start: String = text.chars().take(OUTPUT_EDGE).collect();
                let end: String = text.chars().skip(size - OUTPUT_EDGE).collect::<String>();
                let gone = size - 2 * OUTPUT_EDGE;
                result.content = vec![ToolResultContent::Text {
                    text: format!(
                        "{start}\n[… {gone} characters left out to fit the context …]\n{end}"
                    ),
                }];
            }
        }
        (FitStep::CallArguments, Message::Assistant(assistant)) => {
            for block in &mut assistant.content {
                if let AssistantContent::ToolCall(call) = block {
                    if !kept(&call.name) {
                        shorten_strings(&mut call.arguments);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Whether a tool's calls record a decision and are never shortened.
fn kept(tool: &str) -> bool {
    KEPT_TOOLS.contains(&tool)
}

fn result_chars(content: &[ToolResultContent]) -> usize {
    content
        .iter()
        .map(|block| match block {
            ToolResultContent::Text { text } => text.chars().count(),
            ToolResultContent::Image { .. } => crate::message::IMAGE_ESTIMATE_CHARS,
        })
        .sum()
}

/// Replace every string in `value` longer than [`LONG_ARGUMENT`] by a note
/// of its length.
fn shorten_strings(value: &mut Value) {
    match value {
        Value::String(text) => {
            let size = text.chars().count();
            if size > LONG_ARGUMENT {
                *text = format!("[{size} characters left out to fit the context]");
            }
        }
        Value::Array(items) => items.iter_mut().for_each(shorten_strings),
        Value::Object(fields) => fields.values_mut().for_each(shorten_strings),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::{text_reply, tool_reply};
    use crate::message::{StopReason, ToolResultMessage};
    use crate::prune::prune_to_decisions;
    use serde_json::json;

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn call(id: &str, name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            extra_content: None,
        }
    }

    /// A step that read a file: reasoning, a narrating line and the call,
    /// then its output.
    fn read_step(id: &str, output: usize) -> Vec<Message> {
        let read = call(id, "read", json!({ "path": format!("src/{id}.rs") }));
        let mut reply = tool_reply(vec![], StopReason::ToolUse);
        reply.content = vec![
            AssistantContent::thinking("r".repeat(1_000)),
            AssistantContent::Text {
                text: format!("Reading {id}."),
            },
            AssistantContent::ToolCall(read.clone()),
        ];
        reply.usage = Usage {
            input: 50_000,
            ..Usage::default()
        };
        vec![
            Message::Assistant(reply),
            Message::ToolResult(ToolResultMessage::text(&read, "f".repeat(output))),
        ]
    }

    /// A step that ran a command that changes things, with its output.
    fn command_step(id: &str, output: usize) -> Vec<Message> {
        let run = call(id, "bash", json!({ "command": "cargo build" }));
        vec![
            Message::Assistant(tool_reply(
                vec![(id, "bash", run.arguments.clone())],
                StopReason::ToolUse,
            )),
            Message::ToolResult(ToolResultMessage::text(
                &run,
                format!("start{}end", "o".repeat(output)),
            )),
        ]
    }

    fn history() -> Vec<Message> {
        let mut history = vec![user("fix the build")];
        history.extend(read_step("a", 5_000));
        history.extend(command_step("b", 5_000));
        history.extend(read_step("c", 5_000));
        history.push(Message::Assistant(text_reply("Fixed.")));
        history
    }

    fn valid(messages: &[Message]) -> bool {
        let calls: Vec<&str> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Assistant(a) => Some(a.tool_calls()),
                _ => None,
            })
            .flatten()
            .map(|c| c.id.as_str())
            .collect();
        let results: Vec<&str> = messages
            .iter()
            .filter_map(|m| match m {
                Message::ToolResult(r) => Some(r.tool_call_id.as_str()),
                _ => None,
            })
            .collect();
        calls == results
    }

    #[test]
    fn a_history_within_the_budget_is_left_alone() {
        let history = history();
        let fitted = fit(&history, chars(&history), 0);
        assert_eq!(fitted.messages, history);
        assert!(fitted.fits && !fitted.changed());
    }

    #[test]
    fn reasoning_goes_first_and_only_as_much_as_needed() {
        let history = history();
        // Over by less than one block of reasoning: only the oldest goes.
        let fitted = fit(&history, chars(&history) - 500, 0);
        assert!(fitted.fits);
        assert_eq!(fitted.steps, vec![(FitStep::Reasoning, 1_000)]);
        let Message::Assistant(first) = &fitted.messages[1] else {
            panic!()
        };
        assert!(first.thinking_text().is_empty());
        let Message::Assistant(later) = &fitted.messages[5] else {
            panic!()
        };
        assert!(!later.thinking_text().is_empty());
        // The usage measured what was there before.
        assert_eq!(later.usage, Usage::default());
        assert!(valid(&fitted.messages));
    }

    #[test]
    fn read_outputs_go_before_command_outputs_and_are_stubbed() {
        let history = history();
        let fitted = fit(&history, chars(&history) - 6_000, 0);
        assert!(fitted.fits);
        let taken: Vec<FitStep> = fitted.steps.iter().map(|(s, _)| *s).collect();
        assert_eq!(taken, vec![FitStep::Reasoning, FitStep::ReadOutputs]);
        let Message::ToolResult(read) = &fitted.messages[2] else {
            panic!()
        };
        assert!(read
            .plain_text()
            .contains("5000 characters of output left out"));
        let Message::ToolResult(command) = &fitted.messages[4] else {
            panic!()
        };
        assert_eq!(command.plain_text().len(), 5_008);
        assert!(valid(&fitted.messages));
    }

    #[test]
    fn a_command_output_keeps_its_start_and_end() {
        let history = history();
        let fitted = fit(&history, chars(&history) - 12_000, 0);
        assert!(fitted.fits);
        let Message::ToolResult(command) = &fitted.messages[4] else {
            panic!()
        };
        let text = command.plain_text();
        assert!(text.starts_with("start") && text.ends_with("end"), "{text}");
        assert!(text.contains("characters left out to fit the context"));
    }

    #[test]
    fn the_tail_and_the_decisions_are_kept() {
        let question = call("q", "question", json!({ "question": "x".repeat(2_000) }));
        let mut history = history();
        history.insert(
            1,
            Message::Assistant(tool_reply(
                vec![("q", "question", question.arguments.clone())],
                StopReason::ToolUse,
            )),
        );
        history.insert(
            2,
            Message::ToolResult(ToolResultMessage::text(&question, "y".repeat(2_000))),
        );
        let tail = read_step("d", 5_000);
        history.extend(tail.clone());
        let keep = chars(&tail);
        let fitted = fit(&history, 9_000 + keep, keep);
        assert!(fitted.fits, "{:?}", fitted.steps);
        // Whole, but for the usage measured before.
        let tail: Vec<Message> = tail
            .into_iter()
            .map(|message| match message {
                Message::Assistant(mut assistant) => {
                    assistant.usage = Usage::default();
                    Message::Assistant(assistant)
                }
                other => other,
            })
            .collect();
        assert!(fitted.messages.ends_with(&tail));
        assert!(fitted.messages.contains(&history[1]));
        assert!(fitted.messages.contains(&history[2]));
        assert!(fitted.messages.contains(&history[0]));
        assert!(valid(&fitted.messages));
    }

    #[test]
    fn long_arguments_are_noted_by_their_length() {
        let write = call(
            "w",
            "write",
            json!({ "path": "src/new.rs", "content": "z".repeat(10_000) }),
        );
        let history = vec![
            user("write it"),
            Message::Assistant(tool_reply(
                vec![("w", "write", write.arguments.clone())],
                StopReason::ToolUse,
            )),
            Message::ToolResult(ToolResultMessage::text(&write, "Created src/new.rs")),
            Message::Assistant(text_reply("Done.")),
        ];
        let fitted = fit(&history, 1_000, 0);
        assert!(fitted.fits);
        let Message::Assistant(reply) = &fitted.messages[1] else {
            panic!()
        };
        let arguments = &reply.tool_calls().next().unwrap().arguments;
        assert_eq!(arguments["path"], "src/new.rs");
        assert_eq!(
            arguments["content"],
            "[10000 characters left out to fit the context]"
        );
    }

    #[test]
    fn last_the_exploration_then_the_oldest_messages_go() {
        let history = history();
        // Room for the decisions only.
        let decisions = prune_to_decisions(&history);
        let fitted = fit(&history, chars(&decisions), 0);
        assert!(fitted.fits);
        assert_eq!(fitted.steps.last().unwrap().0, FitStep::Exploration);
        assert_eq!(fitted.messages.len(), decisions.len());

        let many: Vec<Message> = (0..20)
            .flat_map(|i| {
                vec![
                    user(&format!("request {i} {}", "x".repeat(100))),
                    Message::Assistant(text_reply(&format!("answer {i}"))),
                ]
            })
            .collect();
        let fitted = fit(&many, 1_000, 0);
        assert!(fitted.fits);
        assert_eq!(fitted.steps.last().unwrap().0, FitStep::Oldest);
        let Message::User(note) = &fitted.messages[0] else {
            panic!()
        };
        assert!(note.plain_text().contains("earlier messages were left out"));
        assert_eq!(fitted.messages.last(), many.last());
        assert!(chars(&fitted.messages) <= 1_000);
    }

    #[test]
    fn a_tail_over_the_budget_does_not_fit() {
        let history = history();
        let fitted = fit(&history, 10, chars(&history));
        assert!(!fitted.fits);
    }

    #[test]
    fn the_exploration_goes_from_the_oldest_on_and_no_further_than_needed() {
        let mut history = vec![user("start")];
        for i in 0..6 {
            history.extend(command_step(&format!("c{i}"), 100));
            history.push(Message::Assistant(text_reply(&format!("done {i}"))));
        }
        // Room for all but some of the oldest exploration.
        let budget = chars(&history) - 300;
        let fitted = fit(&history, budget, 0);
        assert!(fitted.fits);
        assert_eq!(fitted.steps.last().unwrap().0, FitStep::Exploration);
        // The latest commands are still there, the oldest are not.
        let calls: Vec<String> = fitted
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Assistant(a) => Some(a.tool_calls()),
                _ => None,
            })
            .flatten()
            .map(|c| c.id.clone())
            .collect();
        assert!(calls.contains(&"c5".to_string()) && calls.contains(&"c4".to_string()));
        assert!(!calls.contains(&"c0".to_string()));
        assert!(valid(&fitted.messages));
    }

    #[test]
    fn each_message_tells_where_it_came_from() {
        let history = history();
        let decisions = prune_to_decisions(&history);
        let fitted = fit(&history, chars(&decisions), 0);
        assert_eq!(fitted.origin, vec![Some(0), Some(7)]);
        let many: Vec<Message> = (0..20)
            .map(|i| user(&format!("request {i} {}", "x".repeat(100))))
            .collect();
        let fitted = fit(&many, 1_000, 0);
        assert_eq!(fitted.origin[0], None);
        assert_eq!(fitted.origin.last(), Some(&Some(19)));
        assert_eq!(fitted.origin.len(), fitted.messages.len());
    }

    #[test]
    fn the_same_history_fits_the_same_way() {
        let history = history();
        assert_eq!(fit(&history, 3_000, 100), fit(&history, 3_000, 100));
    }

    #[test]
    fn a_token_budget_turns_into_fewer_characters_than_four_a_token() {
        assert_eq!(chars_for_tokens(100_000), 250_000);
    }
}
