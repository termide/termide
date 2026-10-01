//! The reviewer of `auto` mode: a separate model call that decides, in place
//! of a permission prompt, whether a tool call the rules leave open runs.
//!
//! The reviewer sees what the user wrote and the calls the agent made — never
//! the results of those calls, which is where hostile content from files and
//! web pages enters, nor the agent's own text, so the agent cannot argue its
//! case. What it is told comes from `system/classify.md`, the seed being a
//! data file in `assets/`, the way the other service prompts are.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::cancel::CancelToken;
use crate::layers::split_front_matter;
use crate::message::{Message, StopReason, ToolCall, UserMessage};
use crate::provider::{ModelSpec, Provider, Request, ThinkingLevel};
use crate::tool::ToolContext;

/// The seed of `system/classify.md`.
pub const SEED_CLASSIFY: &str = include_str!("../assets/system/classify.md");

/// The longest user message the reviewer is shown, in characters; a pasted
/// log keeps its head.
const MAX_USER_CHARS: usize = 4000;
/// How many of the agent's latest calls the reviewer is shown.
const MAX_CALLS: usize = 40;
/// The longest call arguments shown for an earlier call, and for the one
/// being judged, in characters.
const MAX_CALL_CHARS: usize = 1000;
const MAX_PENDING_CHARS: usize = 8000;
/// Room for the verdict: two short lines, and some for a model that reasons
/// even when asked not to.
const VERDICT_TOKENS: u64 = 1024;

/// One thing the reviewer knows about the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentEntry {
    /// What the user wrote.
    User(String),
    /// A task another agent delegated to this one: its words, not the user's.
    Delegated(String),
    /// A call the agent made, with its arguments.
    Call { tool: String, arguments: String },
}

/// What the reviewer judges a call against: the user's messages and the
/// agent's calls, in order. Shared between the loop that records it and the
/// hooks that read it; kept apart from the transcript, so a compaction does
/// not lose a boundary the user set.
#[derive(Debug, Clone, Default)]
pub struct IntentLog {
    entries: Arc<Mutex<Vec<IntentEntry>>>,
}

impl IntentLog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A log for an agent working for another: the user's messages of
    /// `parent` (and the tasks delegated to it), without its calls. The task
    /// itself arrives as the delegated loop's prompt.
    #[must_use]
    pub fn delegated(parent: &IntentLog) -> Self {
        let log = Self::new();
        for entry in parent.snapshot() {
            if matches!(entry, IntentEntry::User(_) | IntentEntry::Delegated(_)) {
                log.push(entry);
            }
        }
        log
    }

    pub fn push(&self, entry: IntentEntry) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry);
        }
    }

    /// Record the calls of an assistant message.
    pub fn push_calls<'a>(&self, calls: impl Iterator<Item = &'a ToolCall>) {
        for call in calls {
            self.push(IntentEntry::Call {
                tool: call.name.clone(),
                arguments: call.arguments.to_string(),
            });
        }
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<IntentEntry> {
        self.entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default()
    }
}

/// The session a call comes from, as the reviewer needs it: what the user
/// asked for, and the model the session runs on, which reviews unless
/// another one is configured.
#[derive(Clone)]
pub struct SessionView {
    pub intent: IntentLog,
    pub provider: Arc<dyn Provider>,
    pub model: ModelSpec,
}

impl std::fmt::Debug for SessionView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionView")
            .field("intent", &self.intent)
            .field("provider", &self.provider.name())
            .field("model", &self.model.id)
            .finish()
    }
}

/// The reviewer's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow {
        reason: String,
    },
    Block {
        reason: String,
    },
    /// No decision: the call failed, the reply made no sense, or there was
    /// nothing to judge against. The user is asked instead.
    Unavailable {
        reason: String,
    },
}

/// Decides a call in place of the user.
pub trait Classifier: Send {
    fn classify(&mut self, call: &ToolCall, ctx: &ToolContext) -> Verdict;
}

/// What the reviewer is told, and what the verdict call asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifyPrompt {
    /// The reviewer's system prompt; `{{cwd}}` takes the working directory.
    pub instructions: String,
    /// The closing words of the user turn that carries the session.
    pub request: String,
}

impl Default for ClassifyPrompt {
    fn default() -> Self {
        Self::from_file(SEED_CLASSIFY)
    }
}

impl ClassifyPrompt {
    /// Parse `classify.md`: front matter `request:` plus the instructions.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        let (fields, body) = split_front_matter(text);
        Self {
            instructions: body.trim().to_string(),
            request: fields.get("request").cloned().unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn system_prompt(&self, cwd: &Path) -> String {
        self.instructions
            .replace("{{cwd}}", &cwd.display().to_string())
            .trim_end()
            .to_string()
    }

    /// The user turn: the session, oldest first, then the pending call and
    /// the request.
    #[must_use]
    pub fn user_turn(&self, intent: &[IntentEntry], call: &ToolCall) -> String {
        let mut text = String::from("The session so far, oldest first:\n\n");
        let calls = intent
            .iter()
            .filter(|e| matches!(e, IntentEntry::Call { .. }))
            .count();
        let mut skip = calls.saturating_sub(MAX_CALLS);
        for entry in intent {
            match entry {
                IntentEntry::User(message) => {
                    text.push_str(&format!("[user]\n{}\n\n", clip(message, MAX_USER_CHARS)));
                }
                IntentEntry::Delegated(task) => text.push_str(&format!(
                    "[task delegated by another agent, not written by the user]\n{}\n\n",
                    clip(task, MAX_USER_CHARS)
                )),
                IntentEntry::Call { tool, arguments } => {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    text.push_str(&format!(
                        "[agent call] {tool} {}\n\n",
                        clip(arguments, MAX_CALL_CHARS)
                    ));
                }
            }
        }
        text.push_str(&format!(
            "The pending action:\n\n[pending call] {} {}\n\n{}",
            call.name,
            clip(&call.arguments.to_string(), MAX_PENDING_CHARS),
            self.request
        ));
        text
    }
}

/// `text` cut to `max` characters, with a mark where it was cut.
fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}… [cut]", &text[..at]),
        None => text.to_string(),
    }
}

/// Read a verdict from the reviewer's reply: the first word decides, and
/// only an explicit `ALLOW` or `BLOCK` counts. The reason is the rest of the
/// first line or the next line.
#[must_use]
pub fn parse_classification(reply: &str) -> Verdict {
    let mut lines = reply.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or_default();
    let first = first.trim_start_matches(['*', '`', '#', ' ']);
    let token: String = first
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_uppercase();
    let tail = first[token.len()..]
        .trim_start_matches(['*', '`', ':', '.', '-', '—', ' '])
        .trim();
    let reason = if tail.is_empty() {
        lines.next().unwrap_or_default().to_string()
    } else {
        tail.to_string()
    };
    match token.as_str() {
        "ALLOW" => Verdict::Allow { reason },
        "BLOCK" => Verdict::Block { reason },
        _ => Verdict::Unavailable {
            reason: format!(
                "the reviewer's reply had no verdict: {:?}",
                clip(reply, 200)
            ),
        },
    }
}

/// What a host needs to build the reviewer of each run it spawns: the texts,
/// and the model to review with when it is not the session's.
#[derive(Clone, Default)]
pub struct ReviewerSetup {
    pub prompt: ClassifyPrompt,
    pub model: Option<(Arc<dyn Provider>, ModelSpec)>,
}

impl std::fmt::Debug for ReviewerSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewerSetup")
            .field("model", &self.model.as_ref().map(|(_, model)| &model.id))
            .finish_non_exhaustive()
    }
}

impl ReviewerSetup {
    /// The reviewer of a run that `cancel` stops.
    #[must_use]
    pub fn classifier(&self, cancel: CancelToken) -> ModelClassifier {
        let classifier = ModelClassifier::new(self.prompt.clone(), cancel);
        match &self.model {
            Some((provider, model)) => classifier.with_model(Arc::clone(provider), model.clone()),
            None => classifier,
        }
    }
}

/// A reviewer that asks a model: its own, or the session's.
pub struct ModelClassifier {
    prompt: ClassifyPrompt,
    /// A model configured for reviewing; `None` reviews with the session's.
    own: Option<(Arc<dyn Provider>, ModelSpec)>,
    cancel: CancelToken,
}

impl ModelClassifier {
    #[must_use]
    pub fn new(prompt: ClassifyPrompt, cancel: CancelToken) -> Self {
        Self {
            prompt,
            own: None,
            cancel,
        }
    }

    /// Review with `model` on `provider` instead of the session's model.
    #[must_use]
    pub fn with_model(mut self, provider: Arc<dyn Provider>, model: ModelSpec) -> Self {
        self.own = Some((provider, model));
        self
    }
}

impl Classifier for ModelClassifier {
    fn classify(&mut self, call: &ToolCall, ctx: &ToolContext) -> Verdict {
        let Some(session) = &ctx.session else {
            return Verdict::Unavailable {
                reason: "the call comes from outside a session".into(),
            };
        };
        let intent = session.intent.snapshot();
        if !intent
            .iter()
            .any(|e| matches!(e, IntentEntry::User(_) | IntentEntry::Delegated(_)))
        {
            return Verdict::Unavailable {
                reason: "there is no request to judge the call against".into(),
            };
        }
        let (provider, model) = match &self.own {
            Some((provider, model)) => (provider, model),
            None => (&session.provider, &session.model),
        };
        let mut model = model.clone();
        model.thinking = ThinkingLevel::Off;
        model.max_tokens = Some(VERDICT_TOKENS);
        let system_prompt = self.prompt.system_prompt(&ctx.cwd);
        let messages = [Message::User(UserMessage::text(
            self.prompt.user_turn(&intent, call),
        ))];
        let request = Request {
            model: &model,
            system_prompt: &system_prompt,
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let reply = provider.stream(&request, &mut |_| {}, &self.cancel);
        if matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted) {
            return Verdict::Unavailable {
                reason: reply
                    .error_message
                    .unwrap_or_else(|| "the review call did not finish".into()),
            };
        }
        parse_classification(&reply.plain_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{AssistantContent, AssistantMessage, Usage};
    use serde_json::{json, Value};

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments,
            extra_content: None,
        }
    }

    #[test]
    fn the_seed_parses_and_takes_the_directory() {
        let prompt = ClassifyPrompt::default();
        assert!(prompt.request.starts_with("Judge the pending action"));
        assert!(prompt
            .instructions
            .starts_with("You are the permission reviewer"));
        let system = prompt.system_prompt(Path::new("/work/proj"));
        assert!(system.contains("/work/proj"));
        assert!(!system.contains("{{cwd}}"));
    }

    #[test]
    fn a_verdict_needs_an_explicit_word() {
        assert_eq!(
            parse_classification("ALLOW\nBuilding is part of the task."),
            Verdict::Allow {
                reason: "Building is part of the task.".into()
            }
        );
        assert_eq!(
            parse_classification("**BLOCK**: force push rewrites history"),
            Verdict::Block {
                reason: "force push rewrites history".into()
            }
        );
        assert!(matches!(
            parse_classification("block — sends the key out"),
            Verdict::Block { .. }
        ));
        assert!(matches!(
            parse_classification("I think it is fine"),
            Verdict::Unavailable { .. }
        ));
        assert!(matches!(
            parse_classification(""),
            Verdict::Unavailable { .. }
        ));
    }

    #[test]
    fn the_turn_shows_user_words_and_calls_but_marks_a_delegated_task() {
        let parent = IntentLog::new();
        parent.push(IntentEntry::User("fix the build".into()));
        parent.push_calls([call("bash", json!({"command": "cargo build"}))].iter());
        let log = IntentLog::delegated(&parent);
        log.push(IntentEntry::Delegated("run the tests".into()));
        let entries = log.snapshot();
        // The parent's calls are not the subagent's.
        assert_eq!(
            entries,
            [
                IntentEntry::User("fix the build".into()),
                IntentEntry::Delegated("run the tests".into())
            ]
        );
        let turn = ClassifyPrompt::default()
            .user_turn(&entries, &call("bash", json!({"command": "cargo test"})));
        assert!(turn.contains("[user]\nfix the build"));
        assert!(turn.contains("not written by the user]\nrun the tests"));
        assert!(turn.contains("[pending call] bash {\"command\":\"cargo test\"}"));
        assert!(turn.ends_with(&ClassifyPrompt::default().request));
    }

    #[test]
    fn only_the_latest_calls_are_shown_and_long_text_is_cut() {
        let log = IntentLog::new();
        log.push(IntentEntry::User("x".repeat(MAX_USER_CHARS + 10)));
        for n in 0..MAX_CALLS + 5 {
            log.push_calls([call("read", json!({"path": format!("f{n}")}))].iter());
        }
        let turn = ClassifyPrompt::default().user_turn(&log.snapshot(), &call("bash", json!({})));
        assert!(turn.contains("… [cut]"));
        assert!(!turn.contains("\"f4\""));
        assert!(turn.contains("\"f5\""));
        assert!(turn.contains(&format!("\"f{}\"", MAX_CALLS + 4)));
    }

    /// A request as the canned provider saw it: model id, user turn,
    /// reasoning level and output bound.
    type Seen = (String, String, ThinkingLevel, Option<u64>);

    /// Answers every request with `reply` and records what it was sent.
    struct Canned {
        reply: AssistantMessage,
        seen: Mutex<Vec<Seen>>,
    }

    impl Canned {
        fn new(text: &str) -> Arc<Self> {
            Arc::new(Self {
                reply: AssistantMessage {
                    content: vec![AssistantContent::Text { text: text.into() }],
                    stop_reason: StopReason::Stop,
                    usage: Usage::default(),
                    provider: "canned".into(),
                    model: "m".into(),
                    error_message: None,
                    timestamp: 0,
                },
                seen: Mutex::new(Vec::new()),
            })
        }
    }

    impl Provider for Canned {
        fn name(&self) -> &str {
            "canned"
        }

        fn stream(
            &self,
            request: &Request<'_>,
            _on_event: &mut dyn FnMut(crate::provider::StreamEvent),
            _cancel: &CancelToken,
        ) -> AssistantMessage {
            let Message::User(user) = &request.messages[0] else {
                panic!("one user turn");
            };
            self.seen.lock().unwrap().push((
                request.model.id.clone(),
                user.plain_text(),
                request.thinking,
                request.model.max_tokens,
            ));
            self.reply.clone()
        }
    }

    fn spec(id: &str) -> ModelSpec {
        ModelSpec {
            provider: "canned".into(),
            id: id.into(),
            context_window: 8192,
            max_tokens: None,
            thinking: ThinkingLevel::High,
        }
    }

    fn session_ctx(provider: Arc<Canned>, intent: IntentLog) -> ToolContext {
        let mut ctx = ToolContext::new("/proj");
        ctx.session = Some(SessionView {
            intent,
            provider,
            model: spec("session-model"),
        });
        ctx
    }

    #[test]
    fn the_model_reviewer_asks_the_session_model_unless_given_its_own() {
        let pending = call("bash", json!({"command": "cargo build"}));
        let mut reviewer = ModelClassifier::new(ClassifyPrompt::default(), CancelToken::new());
        // Nothing to judge against: a call from outside a session, or before
        // the user said anything.
        assert!(matches!(
            reviewer.classify(&pending, &ToolContext::new("/proj")),
            Verdict::Unavailable { .. }
        ));
        let session = Canned::new("ALLOW\nbuilding is the task");
        assert!(matches!(
            reviewer.classify(&pending, &session_ctx(session.clone(), IntentLog::new())),
            Verdict::Unavailable { .. }
        ));

        let intent = IntentLog::new();
        intent.push(IntentEntry::User("build it".into()));
        let ctx = session_ctx(session.clone(), intent);
        assert_eq!(
            reviewer.classify(&pending, &ctx),
            Verdict::Allow {
                reason: "building is the task".into()
            }
        );
        let seen = session.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        let (model, turn, thinking, max_tokens) = &seen[0];
        assert_eq!(model, "session-model");
        assert!(turn.contains("[user]\nbuild it"));
        assert_eq!(*thinking, ThinkingLevel::Off);
        assert_eq!(*max_tokens, Some(VERDICT_TOKENS));

        let own = Canned::new("BLOCK\nnot asked for");
        let mut reviewer = ModelClassifier::new(ClassifyPrompt::default(), CancelToken::new())
            .with_model(own.clone(), spec("reviewer-model"));
        assert!(matches!(
            reviewer.classify(&pending, &ctx),
            Verdict::Block { .. }
        ));
        assert_eq!(own.seen.lock().unwrap()[0].0, "reviewer-model");
        assert_eq!(session.seen.lock().unwrap().len(), 1);
    }
}
