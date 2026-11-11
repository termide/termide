//! The agent loop: turns, tool execution, steering and follow-up queues.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::ask::UserAsker;
use crate::cancel::{CancelToken, PauseGate};
use crate::classifier::{IntentEntry, IntentLog, SessionView};
use crate::compaction::{
    context_tokens, is_context_overflow_error, should_compact, split_point, CompactionCall,
    CompactionPolicy, CompactionPrompts, CompactionReason, MIN_SUMMARY_CHARS, REJECTED_REPLY_CHARS,
};
use crate::fit::{chars_for_tokens, fit, FitStep};
use crate::goal::{parse_verdict, GoalOutcome, GoalPrompt, GoalVerdict};
use crate::handoff::HandoffPrompt;
use crate::loop_guard::{LoopGuard, LoopPrompt, LoopReport};
use crate::message::{
    AssistantContent, AssistantMessage, Message, StopReason, ToolCall, ToolResultContent,
    ToolResultMessage, Usage, UserContent, UserMessage,
};
use crate::permissions::{ChannelPrompter, DecidedBy, PermissionNote};
use crate::provider::{ModelSpec, Provider, Request, StreamEvent, ThinkingLevel};
use crate::shell::ShellRunner;
use crate::spend::Spender;
use crate::suggest::CommandSuggester;
use crate::tool::{ToolContext, ToolRegistry, ToolUpdate};

/// How many queued messages one drain point delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueMode {
    /// Deliver everything queued as one message: messages typed while the
    /// agent works are usually one thought written in pieces.
    #[default]
    Merged,
    /// Deliver one message per turn boundary, so the model reacts to each.
    OneAtATime,
    /// Deliver everything queued at once, each as its own message.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgentConfig {
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
}

#[derive(Debug, Default)]
struct Queues {
    steering: VecDeque<UserMessage>,
    follow_up: VecDeque<UserMessage>,
    /// A graceful pause was asked for: the loop stops before the next tool
    /// call or model call, leaving the run resumable, unlike an abrupt
    /// cancel.
    paused: bool,
    /// The model (with its reasoning level) the next step runs on, set while
    /// a run is in flight; the loop takes it at the step boundary.
    next_model: Option<ModelSpec>,
}

/// Thread-safe handle to the steering and follow-up queues.
///
/// Steering messages interrupt the current work at the next turn boundary;
/// follow-up messages wait until the agent has nothing left to do. Both are
/// pushed from the UI thread while the loop runs elsewhere.
#[derive(Debug, Clone, Default)]
pub struct QueueHandle {
    inner: Arc<Mutex<Queues>>,
}

impl QueueHandle {
    pub fn steer(&self, message: UserMessage) {
        self.lock().steering.push_back(message);
    }

    pub fn follow_up(&self, message: UserMessage) {
        self.lock().follow_up.push_back(message);
    }

    /// Drop everything queued and hand it back so the UI can restore the
    /// text into its input box.
    pub fn clear(&self) -> (Vec<UserMessage>, Vec<UserMessage>) {
        let mut queues = self.lock();
        (
            queues.steering.drain(..).collect(),
            queues.follow_up.drain(..).collect(),
        )
    }

    /// `(steering, follow_up)` queue lengths.
    #[must_use]
    pub fn lens(&self) -> (usize, usize) {
        let queues = self.lock();
        (queues.steering.len(), queues.follow_up.len())
    }

    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.lens() != (0, 0)
    }

    /// Ask the loop to pause at the next turn boundary — graceful, unlike a
    /// cancel: the current step finishes and the run can be resumed.
    pub fn pause(&self) {
        self.lock().paused = true;
    }

    /// Whether a pause has been requested (without clearing it).
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.lock().paused
    }

    /// Take the pause request, clearing it.
    fn take_pause(&self) -> bool {
        std::mem::take(&mut self.lock().paused)
    }

    /// Clear any pause request before a run or resume starts.
    pub fn clear_pause(&self) {
        self.lock().paused = false;
    }

    /// Have the next step run on `model`: the loop takes it at the step
    /// boundary, as it takes steering, so a run in flight switches model or
    /// reasoning level without stopping.
    pub fn set_next_model(&self, model: ModelSpec) {
        self.lock().next_model = Some(model);
    }

    fn take_next_model(&self) -> Option<ModelSpec> {
        self.lock().next_model.take()
    }

    fn take_steering(&self, mode: QueueMode) -> Vec<UserMessage> {
        let mut queues = self.lock();
        take(&mut queues.steering, mode)
    }

    fn take_follow_up(&self, mode: QueueMode) -> Vec<UserMessage> {
        let mut queues = self.lock();
        take(&mut queues.follow_up, mode)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queues> {
        // A poisoned queue only means a UI thread panicked mid-push; the
        // data is still a valid VecDeque, so keep serving it.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn take(queue: &mut VecDeque<UserMessage>, mode: QueueMode) -> Vec<UserMessage> {
    match mode {
        QueueMode::Merged => UserMessage::take_merged(queue).into_iter().collect(),
        QueueMode::OneAtATime => queue.pop_front().into_iter().collect(),
        QueueMode::All => queue.drain(..).collect(),
    }
}

/// Verdict of [`Hooks::before_tool_call`].
#[derive(Debug, Clone, PartialEq)]
pub enum ToolDecision {
    /// No objection; later hooks in a chain still have their say.
    Allow,
    /// Run the call with these arguments instead of the model's; later hooks
    /// see the new arguments.
    Replace { arguments: Value },
    /// Run the call and ask nobody else: a hook standing in for the
    /// permission prompt. `arguments` replaces the model's when given.
    Approve { arguments: Option<Value> },
    /// Skip the call; `reason` is returned to the model as an error result.
    Block { reason: String },
}

/// Run one tool call as the loop does: the hooks' `before_tool_call` first
/// (which may block it, rewrite its arguments or approve it), then the tool
/// itself. Shared with whatever else runs termide's tools for a model — the
/// MCP server that serves them to an external agent — so a call is judged the
/// same wherever it comes from. `after_tool_call` is the caller's, since the
/// loop runs it for results that never reach a tool too.
pub fn execute_tool(
    tools: &ToolRegistry,
    call: &ToolCall,
    hooks: &mut dyn Hooks,
    ctx: &ToolContext,
    cancel: &CancelToken,
    on_update: &mut dyn FnMut(ToolUpdate),
) -> ToolResultMessage {
    match judge_tool_call(tools, call, hooks, ctx, cancel) {
        Judgment::Run(judged) => run_judged_call(tools, judged, ctx, cancel, on_update),
        Judgment::Done(result) => result,
    }
}

/// A call the hooks let run: with the arguments they settled on, and who
/// decided.
#[derive(Debug)]
pub struct JudgedCall {
    pub call: ToolCall,
    pub permission: Option<Box<PermissionNote>>,
}

/// What the hooks made of a call: run it, or its result is already known.
#[derive(Debug)]
pub enum Judgment {
    Run(JudgedCall),
    /// The result of a call that does not run: cancelled, unknown or blocked.
    Done(ToolResultMessage),
}

/// The first half of [`execute_tool`]: the hooks judge the call. Apart from
/// the run so that a host serving several calls at once holds its hooks
/// only while they judge, not while a tool runs.
pub fn judge_tool_call(
    tools: &ToolRegistry,
    call: &ToolCall,
    hooks: &mut dyn Hooks,
    ctx: &ToolContext,
    cancel: &CancelToken,
) -> Judgment {
    if cancel.is_cancelled() {
        return Judgment::Done(ToolResultMessage::error(
            call,
            "The run was cancelled before this tool ran.",
        ));
    }
    let Some(tool) = tools.get(&call.name) else {
        log::warn!("model requested unknown tool `{}`", call.name);
        return Judgment::Done(ToolResultMessage::error(
            call,
            format!(
                "Unknown tool `{}`. Available tools: {}.",
                call.name,
                tools.names().join(", ")
            ),
        ));
    };
    let ctx = &ToolContext {
        tool: tool.call_traits(call, ctx),
        ..ctx.clone()
    };
    // A call bound to fail is not worth a prompt or a reviewer's verdict.
    if let Err(message) = tool.check(call, ctx) {
        return Judgment::Done(ToolResultMessage::error(call, message));
    }
    let decision = hooks.before_tool_call(call, ctx);
    let permission = hooks.take_permission().map(Box::new);
    let effective = match decision {
        ToolDecision::Allow | ToolDecision::Approve { arguments: None } => call.clone(),
        ToolDecision::Replace { arguments }
        | ToolDecision::Approve {
            arguments: Some(arguments),
        } => ToolCall {
            arguments,
            ..call.clone()
        },
        ToolDecision::Block { reason } => {
            let mut result = ToolResultMessage::error(call, format!("Tool call blocked: {reason}"));
            result.permission = permission;
            return Judgment::Done(result);
        }
    };
    Judgment::Run(JudgedCall {
        call: effective,
        permission,
    })
}

/// The second half of [`execute_tool`]: run a call the hooks let through.
pub fn run_judged_call(
    tools: &ToolRegistry,
    judged: JudgedCall,
    ctx: &ToolContext,
    cancel: &CancelToken,
    on_update: &mut dyn FnMut(ToolUpdate),
) -> ToolResultMessage {
    let Some(tool) = tools.get(&judged.call.name) else {
        return ToolResultMessage::error(&judged.call, "The tool is gone.");
    };
    let mut result = tool.execute(&judged.call, ctx, on_update, cancel);
    result.permission = judged.permission;
    result
}

/// Whether a call may run side by side with its neighbours of the same
/// kind: it changes nothing on this machine — a read, a search, a web page,
/// a look-only shell command — or hands work to a subagent, and it asks the
/// user nothing. A call that may change something runs alone, so the model's
/// order holds between a change and what reads it. A command is read in
/// the language of the shell its tool runs it in, from `tools`.
#[must_use]
pub fn runs_alongside(call: &ToolCall, tools: &ToolRegistry) -> bool {
    match call.name.as_str() {
        "task" => true,
        "question" | "suggest_command" | "suggest_goal" | "suggest_loop" => false,
        _ => {
            let shell = tools
                .get(&call.name)
                .map(|tool| tool.traits().shell)
                .unwrap_or_default();
            crate::permissions::is_read_only_call(call, shell)
        }
    }
}

/// Extension points of the loop. All methods have permissive defaults.
///
/// Hooks run on the agent thread, so a permission prompt may block here
/// while it waits for the user's answer.
pub trait Hooks: Send {
    fn before_tool_call(&mut self, _call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
        ToolDecision::Allow
    }

    /// Who decided the call `before_tool_call` judged last, for the record
    /// kept with its result; taking it clears it. A hook that does not judge
    /// permissions has nothing to say.
    fn take_permission(&mut self) -> Option<PermissionNote> {
        None
    }

    /// Inspect or rewrite a result before it enters the transcript. Runs for
    /// every result, including blocked and failed calls.
    fn after_tool_call(
        &mut self,
        _call: &ToolCall,
        result: ToolResultMessage,
    ) -> ToolResultMessage {
        result
    }

    /// Return `true` to end the run after this turn even if the model asked
    /// for more tool calls or messages are queued.
    fn should_stop_after_turn(&mut self, _message: &AssistantMessage) -> bool {
        false
    }
}

/// The default hooks: allow everything, never stop early.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoHooks;

impl Hooks for NoHooks {}

/// Hooks run one after another: `before_tool_call` stops at the first
/// `Block` or `Approve`, threading a `Replace` into the calls the rest see;
/// `after_tool_call` passes the result through each; the turn stops when
/// any says so.
pub struct ChainedHooks {
    hooks: Vec<Box<dyn Hooks>>,
    note: Option<PermissionNote>,
}

impl ChainedHooks {
    #[must_use]
    pub fn new(hooks: Vec<Box<dyn Hooks>>) -> Self {
        Self { hooks, note: None }
    }
}

impl Hooks for ChainedHooks {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        let mut current = call.clone();
        let mut replaced = false;
        self.note = None;
        for hook in &mut self.hooks {
            let decision = hook.before_tool_call(&current, ctx);
            // The hook that settles the call says who decided; one that does
            // not judge permissions (a command hook) is recorded as a hook.
            let settled = |hook: &mut Box<dyn Hooks>, allowed: bool| {
                hook.take_permission()
                    .or_else(|| Some(PermissionNote::new(DecidedBy::Hook, allowed)))
            };
            match decision {
                ToolDecision::Allow => {
                    if let Some(note) = hook.take_permission() {
                        self.note = Some(note);
                    }
                }
                ToolDecision::Replace { arguments } => {
                    current.arguments = arguments;
                    replaced = true;
                }
                ToolDecision::Approve { arguments } => {
                    self.note = settled(hook, true);
                    return ToolDecision::Approve {
                        arguments: arguments.or_else(|| replaced.then_some(current.arguments)),
                    };
                }
                block @ ToolDecision::Block { .. } => {
                    self.note = settled(hook, false);
                    return block;
                }
            }
        }
        if replaced {
            ToolDecision::Replace {
                arguments: current.arguments,
            }
        } else {
            ToolDecision::Allow
        }
    }

    fn take_permission(&mut self) -> Option<PermissionNote> {
        self.note.take()
    }

    fn after_tool_call(&mut self, call: &ToolCall, result: ToolResultMessage) -> ToolResultMessage {
        self.hooks
            .iter_mut()
            .fold(result, |result, hook| hook.after_tool_call(call, result))
    }

    fn should_stop_after_turn(&mut self, message: &AssistantMessage) -> bool {
        self.hooks
            .iter_mut()
            .any(|hook| hook.should_stop_after_turn(message))
    }
}

/// What the loop reports while it runs. Every transcript change is announced
/// through `MessageEnd`, so a consumer can mirror the transcript from events
/// alone.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    /// The model call started; `MessageUpdate`s follow until `MessageEnd`.
    MessageStart {
        /// Rough size of the prompt the model is about to read, in tokens:
        /// the last reported usage plus an estimate for what was appended
        /// after it. `None` from a backend that cannot tell.
        prompt_tokens: Option<u64>,
    },
    MessageUpdate(StreamEvent),
    /// A message was appended to the transcript.
    MessageEnd(Message),
    ToolExecutionStart {
        call: ToolCall,
    },
    /// A started call's arguments grew: a backend that streams them (Claude
    /// Code over ACP) shows the call on its first keys and fills in the rest
    /// as they arrive. `call` carries the same id and the arguments so far.
    ToolCallUpdate {
        call: ToolCall,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        update: ToolUpdate,
    },
    ToolExecutionEnd {
        result: ToolResultMessage,
    },
    TurnEnd,
    /// Queue lengths after the loop drained messages: `(steering, follow_up)`.
    QueueUpdate {
        steering: usize,
        follow_up: usize,
    },
    /// A compaction began; `prompt_tokens` estimates the part of the
    /// transcript the summary call reads.
    CompactionStart {
        reason: CompactionReason,
        prompt_tokens: u64,
    },
    /// Stream progress of the summary call: prefill progress, retries and
    /// the summary's deltas. Not part of the transcript.
    CompactionUpdate(StreamEvent),
    /// The transcript now starts with a summary message followed by the
    /// `kept` most recent messages; consumers mirroring the transcript must
    /// apply the same replacement. `tokens_after` estimates the new context.
    Compacted {
        summary: String,
        kept: usize,
        tokens_before: u64,
        tokens_after: u64,
        call: CompactionCall,
    },
    /// The compaction did not happen and the transcript is untouched.
    /// `call` is how the summary call went and `reply` the text it was
    /// refused for (cut to [`REJECTED_REPLY_CHARS`]), when a call was made.
    CompactionFailed {
        error: String,
        tokens_before: u64,
        call: Option<CompactionCall>,
        reply: String,
    },
    /// The context was cut down by kinds of content, with no summary, after
    /// a compaction did not happen: see [`crate::fit::fit`]. `budget` and
    /// `keep_tail` (characters) are what it was fitted with, so a log can
    /// replay it; consumers mirroring the transcript apply the same fitting.
    Fitted {
        reason: CompactionReason,
        budget: usize,
        keep_tail: usize,
        steps: Vec<(FitStep, usize)>,
        tokens_before: u64,
        tokens_after: u64,
    },
    /// The autonomous-goal judge ran (`/goal`) and returned its verdict: the
    /// goal is reached, cannot be reached, or more work is needed, with a
    /// one-line reason. `reply` is the judge's answer as it came, for the
    /// session log.
    GoalJudged {
        outcome: GoalOutcome,
        reason: String,
        reply: String,
    },
    /// The judge call's stream, for its live progress (`/goal`).
    GoalJudgeUpdate(StreamEvent),
    /// The judge call did not succeed, or its answer held no verdict
    /// (`reply`, empty when there was no answer): the check is tried again,
    /// then the goal pauses.
    GoalJudgeFailed {
        error: String,
        reply: String,
    },
    /// A side call — the goal judge's, a handoff brief's — spent `usage`
    /// on `model`; the session log records it. Not part of the transcript.
    Spent {
        by: Spender,
        model: String,
        usage: Usage,
    },
    /// A `/handoff` brief was produced (or the call failed): the forward-looking
    /// brief for a fresh session, or an error to report.
    Handoff {
        brief: Result<String, String>,
    },
    /// A model call went round in circles and was cut short (see
    /// [`crate::loop_guard`]). With `retrying` the loop nudges the model and
    /// calls it once more without reasoning; without, the second call looped
    /// too and the run stops for the user to decide.
    ThinkingLoop {
        report: LoopReport,
        retrying: bool,
    },
    /// A model call ran to the output limit with nothing to show — no
    /// answer, no tool call: a loop the stream did not reveal (a server may
    /// hold a tool call back until it closes). With `retrying` the loop tells
    /// the model and calls it once more; without, a second call failed so
    /// too (or looped) and the run stops for the user to decide.
    EmptyCutOff {
        retrying: bool,
    },
    /// The loop stopped early on a pause request with work still pending, so
    /// the run can be resumed. Not emitted on a natural finish.
    Paused,
    AgentEnd,
    /// An external agent opened a session of its own for the conversation,
    /// which the log records so a reopened session can resume it. Never from
    /// the built-in loop.
    ExternalSession {
        agent: String,
        session_id: String,
    },
}

/// Transcript owner and loop driver. Single-threaded by design: the runtime
/// moves it onto a worker thread and talks to it through [`QueueHandle`]
/// and [`CancelToken`].
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    model: ModelSpec,
    system_prompt: String,
    cwd: PathBuf,
    messages: Vec<Message>,
    queues: QueueHandle,
    config: AgentConfig,
    compaction: CompactionPolicy,
    compaction_prompts: CompactionPrompts,
    goal_prompt: GoalPrompt,
    handoff_prompt: HandoffPrompt,
    /// The nudge sent after a call that went round in circles.
    loop_prompt: LoopPrompt,
    /// Whom the `question` tool asks; `None` when no one is watching.
    asker: Option<UserAsker>,
    /// Whom a delegate's permission requests go to; `None` when no one is
    /// watching.
    prompter: Option<ChannelPrompter>,
    /// Whom the `suggest_command` tool offers commands to; `None` when no one
    /// is watching to confirm one.
    suggester: Option<CommandSuggester>,
    /// How a confirmed command reaches the shell; `None` when there is no way
    /// to run one.
    shell_run: Option<ShellRunner>,
    /// What the `auto` mode reviewer judges calls against. Built from the
    /// transcript when a run first needs it, then kept up as messages come.
    intent: IntentLog,
    intent_seeded: bool,
    /// Whether the user messages of this loop are a task another agent
    /// wrote, not the user's words.
    delegated: bool,
    /// The id of the session log this loop's conversation is written to,
    /// when there is one; tools see it in [`SessionView::id`].
    session_id: Option<String>,
    /// The agent panel the loop runs in, by its number.
    panel: Option<u64>,
    /// A pause held by another — the panel that delegated this loop's task,
    /// or none when nothing else holds it. The loop waits at its step
    /// boundaries while it is set, and goes on where it stood once lifted.
    pause_gate: Option<PauseGate>,
    /// A note for the model at each call, about the session's state; see
    /// [`ContextNote`].
    context_note: Option<ContextNote>,
}

/// What the model is told at each of its calls about the state of the
/// session — the terminals the agent left open — given the session's id.
/// The note goes at the end of the request only, never into the
/// transcript, so it is always current and the cached prefix stays as it
/// was. `None` when there is nothing to tell.
pub type ContextNote = Arc<dyn Fn(Option<&str>) -> Option<String> + Send + Sync>;

/// `messages` with `note` added to the text of the last one.
fn with_note(messages: &[Message], note: &str) -> Vec<Message> {
    let mut messages = messages.to_vec();
    let text = format!("<system-reminder>\n{note}\n</system-reminder>");
    match messages.last_mut() {
        Some(Message::User(user)) => user.content.push(UserContent::Text { text }),
        Some(Message::ToolResult(result)) => {
            result.content.push(ToolResultContent::Text { text });
        }
        // The model's own turn last: no call is made on it.
        Some(Message::Assistant(_)) | None => {}
    }
    messages
}

impl Agent {
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: ToolRegistry,
        model: ModelSpec,
        cwd: PathBuf,
    ) -> Self {
        Self {
            provider,
            tools,
            model,
            system_prompt: String::new(),
            cwd,
            messages: Vec::new(),
            queues: QueueHandle::default(),
            config: AgentConfig::default(),
            compaction: CompactionPolicy::default(),
            compaction_prompts: CompactionPrompts::default(),
            goal_prompt: GoalPrompt::default(),
            handoff_prompt: HandoffPrompt::default(),
            loop_prompt: LoopPrompt::default(),
            asker: None,
            prompter: None,
            suggester: None,
            shell_run: None,
            intent: IntentLog::new(),
            intent_seeded: false,
            delegated: false,
            session_id: None,
            panel: None,
            pause_gate: None,
            context_note: None,
        }
    }

    /// Tell the model what `note` says at each of its calls.
    #[must_use]
    pub fn with_context_note(mut self, note: ContextNote) -> Self {
        self.context_note = Some(note);
        self
    }

    /// Hold the loop at its step boundaries while `gate` is paused: a panel
    /// that delegated this task pauses it with the panel's own pause, and
    /// lifts it with the panel's resume.
    #[must_use]
    pub fn with_pause_gate(mut self, gate: PauseGate) -> Self {
        self.pause_gate = Some(gate);
        self
    }

    /// Work on a task another agent delegated: the reviewer judges this
    /// loop's calls against `intent` (the user's words before the task), and
    /// the prompts this loop receives count as the delegating agent's.
    #[must_use]
    pub fn with_delegated_intent(mut self, intent: IntentLog) -> Self {
        self.intent = intent;
        self.intent_seeded = true;
        self.delegated = true;
        self
    }

    /// The log the reviewer judges this loop's calls against.
    #[must_use]
    pub fn intent(&self) -> IntentLog {
        self.intent.clone()
    }

    /// Let tools put questions to the user through `asker`.
    #[must_use]
    pub fn with_asker(mut self, asker: UserAsker) -> Self {
        self.asker = Some(asker);
        self
    }

    /// Let the delegates of this loop's `task` calls ask the user through
    /// `prompter` what the rules do not settle.
    #[must_use]
    pub fn with_prompter(mut self, prompter: ChannelPrompter) -> Self {
        self.prompter = Some(prompter);
        self
    }

    /// The id of the session log the conversation is written to, so a tool
    /// can tell that log from the others (`recall` searches only what a
    /// compaction took out of it: the rest is in the context already).
    #[must_use]
    pub fn with_session_id(mut self, id: Option<String>) -> Self {
        self.session_id = id;
        self
    }

    /// The agent panel the loop runs in, by its number: its tools tell
    /// which panel a call comes from.
    #[must_use]
    pub fn with_panel(mut self, panel: u64) -> Self {
        self.panel = Some(panel);
        self
    }

    /// Let tools offer commands to the user through `suggester`.
    #[must_use]
    pub fn with_suggester(mut self, suggester: CommandSuggester) -> Self {
        self.suggester = Some(suggester);
        self
    }

    /// Let a command the user confirmed reach the shell through `shell_run`.
    #[must_use]
    pub fn with_shell_run(mut self, shell_run: ShellRunner) -> Self {
        self.shell_run = Some(shell_run);
        self
    }

    /// Add `message` to the context without starting a run: what a command
    /// the user ran by hand (`$` in the input) leaves behind, so the next
    /// request reads with the command and its output already in view. The
    /// message joins the reviewer's log as the user's own, which is what it
    /// is — they typed the command.
    pub fn append_context(&mut self, message: Message) {
        if self.intent_seeded {
            self.record_intent(&message);
        }
        self.messages.push(message);
    }

    #[must_use]
    pub fn with_compaction(mut self, policy: CompactionPolicy) -> Self {
        self.compaction = policy;
        self
    }

    pub fn set_compaction(&mut self, policy: CompactionPolicy) {
        self.compaction = policy;
    }

    #[must_use]
    pub fn compaction(&self) -> &CompactionPolicy {
        &self.compaction
    }

    #[must_use]
    pub fn with_compaction_prompts(mut self, prompts: CompactionPrompts) -> Self {
        self.compaction_prompts = prompts;
        self
    }

    #[must_use]
    pub fn compaction_prompts(&self) -> &CompactionPrompts {
        &self.compaction_prompts
    }

    #[must_use]
    pub fn with_goal_prompt(mut self, prompt: GoalPrompt) -> Self {
        self.goal_prompt = prompt;
        self
    }

    #[must_use]
    pub fn goal_prompt(&self) -> &GoalPrompt {
        &self.goal_prompt
    }

    #[must_use]
    pub fn with_loop_prompt(mut self, prompt: LoopPrompt) -> Self {
        self.loop_prompt = prompt;
        self
    }

    #[must_use]
    pub fn with_handoff_prompt(mut self, prompt: HandoffPrompt) -> Self {
        self.handoff_prompt = prompt;
        self
    }

    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    #[must_use]
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    #[must_use]
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self.intent_seeded = false;
        self
    }

    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = prompt.into();
    }

    pub fn set_model(&mut self, model: ModelSpec) {
        self.model = model;
    }

    /// Take up the model a run in flight was switched to
    /// ([`QueueHandle::set_next_model`]), if any.
    pub fn adopt_next_model(&mut self) {
        if let Some(model) = self.queues.take_next_model() {
            self.model = model;
        }
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    pub fn set_config(&mut self, config: AgentConfig) {
        self.config = config;
    }

    #[must_use]
    pub fn model(&self) -> &ModelSpec {
        &self.model
    }

    #[must_use]
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    #[must_use]
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }

    /// Shared handle for pushing steering and follow-up messages.
    #[must_use]
    pub fn queues(&self) -> QueueHandle {
        self.queues.clone()
    }

    /// Drop the transcript; queued messages are kept.
    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.intent.clear();
        self.intent_seeded = true;
    }

    /// Clear a plan's exploration from the transcript, see
    /// [`crate::prune::prune_to_decisions`]. The reviewer's record of the
    /// user's words is kept as it is.
    pub fn prune_to_decisions(&mut self) {
        self.messages = crate::prune::prune_to_decisions(&self.messages);
    }

    /// Run the loop for one user prompt until the agent has nothing left to
    /// do, is cancelled, or hits an error.
    pub fn run(
        &mut self,
        prompt: UserMessage,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) {
        self.seed_intent();
        // Calls a pause left unrun are not resumed by a new request; each
        // still needs a result, or the provider rejects the transcript.
        for call in self.unanswered_calls() {
            let result = ToolResultMessage::error(
                &call,
                "Not run: the run was paused before this call, and a new request took its place.",
            );
            self.push(Message::ToolResult(result), emit);
        }
        let mut initial = vec![prompt];
        initial.extend(self.drain_steering(emit));
        self.run_from(initial, false, hooks, cancel, emit);
    }

    /// Resume a paused run: continue the loop on the existing transcript with
    /// no new user message — first running the tool calls the pause left
    /// unrun, then letting the model answer the results (or carry on where
    /// it left off).
    pub fn resume(
        &mut self,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) {
        self.seed_intent();
        let initial = self.drain_steering(emit);
        self.run_from(initial, true, hooks, cancel, emit);
    }

    /// Try the request a failed reply left unanswered again: the model is
    /// called on the transcript as it stands, with no new user message. A
    /// failed reply that made no calls is left out of the model's context —
    /// its text, if any, is a fragment the next reply writes anew, and a
    /// trailing assistant message would read as a prefill. The calls of one
    /// that made some did not run and will not: each gets a result saying so,
    /// as the provider requires one for every call.
    pub fn retry(
        &mut self,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) {
        self.seed_intent();
        let failed = match self.messages.last() {
            Some(Message::Assistant(reply)) if reply.stop_reason == StopReason::Error => {
                Some(reply.tool_calls().next().is_some())
            }
            _ => None,
        };
        if failed == Some(false) {
            self.messages.pop();
        } else if failed == Some(true) {
            for call in self.unanswered_calls() {
                let result = ToolResultMessage::error(
                    &call,
                    "Not run: the reply failed before it finished, and the request was tried again.",
                );
                self.push(Message::ToolResult(result), emit);
            }
        }
        let initial = self.drain_steering(emit);
        self.run_from(initial, false, hooks, cancel, emit);
    }

    /// The loop shared by [`Agent::run`] and [`Agent::resume`]: drive turns
    /// until the work is done, cancelled, or a pause stops it between steps.
    fn run_from(
        &mut self,
        mut pending: Vec<UserMessage>,
        resuming: bool,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) {
        // A pause left over from a previous run must not stop this one before
        // it starts.
        self.queues.clear_pause();
        emit(AgentEvent::AgentStart);

        // A resumed run first runs what the pause left of the last step.
        if resuming {
            let calls = self.unanswered_calls();
            if self.run_calls(&calls, false, hooks, cancel, emit) && self.queues.take_pause() {
                emit(AgentEvent::Paused);
                self.queues.clear_pause();
                emit(AgentEvent::AgentEnd);
                return;
            }
        }

        'outer: loop {
            loop {
                let outcome = self.turn(std::mem::take(&mut pending), hooks, cancel, emit);
                match outcome {
                    TurnOutcome::Halt => break 'outer,
                    TurnOutcome::Continue { had_tool_calls } => {
                        pending.extend(self.drain_steering(emit));
                        let more = had_tool_calls || !pending.is_empty();
                        // A graceful pause stops between steps, but only when
                        // there is still work to do — a finished turn ends on
                        // its own and needs no pause.
                        if more && self.queues.take_pause() {
                            emit(AgentEvent::Paused);
                            break 'outer;
                        }
                        if !had_tool_calls && pending.is_empty() {
                            break;
                        }
                    }
                }
            }

            pending = self.drain_follow_up(emit);
            if pending.is_empty() {
                break;
            }
        }

        self.queues.clear_pause();
        emit(AgentEvent::AgentEnd);
    }

    fn turn(
        &mut self,
        pending: Vec<UserMessage>,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> TurnOutcome {
        // A pause held by the panel that delegated this task stops the loop
        // here, before the next request goes out; lifted, it goes on where it
        // stood. A stop while it waits ends the turn.
        if let Some(gate) = &self.pause_gate {
            if !gate.wait(cancel) {
                return TurnOutcome::Halt;
            }
        }
        emit(AgentEvent::TurnStart);
        for message in pending {
            self.push(Message::User(message), emit);
        }
        // A model or reasoning level picked during the run applies from here.
        self.adopt_next_model();

        if should_compact(&self.messages, self.model.context_window, &self.compaction) {
            // A failed compaction is reported through events, and the
            // context is cut down by kinds instead; the turn still runs and
            // may hit the overflow path below.
            if self
                .compact(CompactionReason::Threshold, None, cancel, emit)
                .is_err()
            {
                let _ = self.fit_context(CompactionReason::Threshold, emit);
            }
        }

        emit(AgentEvent::MessageStart {
            prompt_tokens: Some(context_tokens(&self.messages)),
        });
        let mut retried_after_overflow = false;
        // After a call that went round in circles, the next one is made
        // without reasoning; after one that ran to the output limit with
        // nothing to show, the model is told so. A second such call, of
        // either kind, stops the run.
        let mut thinking = self.model.thinking;
        let mut retried = false;
        let assistant = loop {
            let (reply, looped) = self.call_model(thinking, cancel, emit);
            if let Some(report) = looped {
                self.push(
                    Message::Assistant(cut_reasoning(reply, report.loop_start)),
                    emit,
                );
                emit(AgentEvent::ThinkingLoop {
                    report: report.clone(),
                    retrying: !retried,
                });
                if retried {
                    emit(AgentEvent::TurnEnd);
                    return TurnOutcome::Halt;
                }
                retried = true;
                thinking = ThinkingLevel::Off;
                let nudge = self.loop_prompt.render(&report);
                self.push(Message::User(UserMessage::text(nudge)), emit);
                emit(AgentEvent::MessageStart {
                    prompt_tokens: Some(context_tokens(&self.messages)),
                });
                continue;
            }
            if is_empty_cut_off(&reply) && !cancel.is_cancelled() {
                self.push(Message::Assistant(cut_reasoning(reply, usize::MAX)), emit);
                emit(AgentEvent::EmptyCutOff { retrying: !retried });
                if retried {
                    emit(AgentEvent::TurnEnd);
                    return TurnOutcome::Halt;
                }
                retried = true;
                self.push(Message::User(UserMessage::text(EMPTY_CUT_OFF_NUDGE)), emit);
                emit(AgentEvent::MessageStart {
                    prompt_tokens: Some(context_tokens(&self.messages)),
                });
                continue;
            }
            let overflow = reply.stop_reason == StopReason::Error
                && reply
                    .error_message
                    .as_deref()
                    .is_some_and(is_context_overflow_error);
            if overflow
                && !retried_after_overflow
                && self.compaction.enabled
                && (self
                    .compact(CompactionReason::Overflow, None, cancel, emit)
                    .is_ok()
                    || self.fit_context(CompactionReason::Overflow, emit).is_ok())
            {
                retried_after_overflow = true;
                continue;
            }
            break reply;
        };
        let stop_reason = assistant.stop_reason;
        let calls: Vec<ToolCall> = assistant.tool_calls().cloned().collect();
        self.push(Message::Assistant(assistant.clone()), emit);

        if matches!(stop_reason, StopReason::Error | StopReason::Aborted) {
            emit(AgentEvent::TurnEnd);
            return TurnOutcome::Halt;
        }

        // A pause asked for mid-step stops before the next call; the loop then
        // sees the pause and stops, and a resume runs the rest.
        self.run_calls(
            &calls,
            stop_reason == StopReason::Length,
            hooks,
            cancel,
            emit,
        );

        emit(AgentEvent::TurnEnd);

        if cancel.is_cancelled() || hooks.should_stop_after_turn(&assistant) {
            return TurnOutcome::Halt;
        }
        TurnOutcome::Continue {
            had_tool_calls: !calls.is_empty(),
        }
    }

    /// Run `calls`, pushing each result in their order. Calls that change
    /// nothing or hand work to a subagent run side by side with their
    /// neighbours of the same kind (see [`runs_alongside`]); any other runs
    /// alone, after those before it and before those after it. Stops before
    /// a call or such a group once a pause has been asked for, leaving the
    /// rest unanswered for a resume to run; returns whether it stopped so.
    /// `truncated` calls come from a reply cut off by the output limit and
    /// are refused, not run.
    fn run_calls(
        &mut self,
        calls: &[ToolCall],
        truncated: bool,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> bool {
        if truncated {
            for call in calls {
                let result = ToolResultMessage::error(
                    call,
                    "The response was cut off by the output limit, so the tool call arguments may be incomplete. The call was not executed.",
                );
                let result = hooks.after_tool_call(call, result);
                emit(AgentEvent::ToolExecutionEnd {
                    result: result.clone(),
                });
                self.push(Message::ToolResult(result), emit);
            }
            return false;
        }
        let mut start = 0;
        while start < calls.len() {
            if !cancel.is_cancelled() && self.queues.is_paused() {
                return true;
            }
            // Held by the panel that delegated the task: wait here, before the
            // next call, rather than stop and need a resume of our own.
            if let Some(gate) = &self.pause_gate {
                if !gate.wait(cancel) {
                    return true;
                }
            }
            let together = calls[start..]
                .iter()
                .take_while(|call| runs_alongside(call, &self.tools))
                .count();
            let end = start + together.max(1);
            let results = if together > 1 {
                self.run_together(&calls[start..end], hooks, cancel, emit)
            } else {
                let call = &calls[start];
                let result = self.execute_call(call, hooks, cancel, emit);
                let result = hooks.after_tool_call(call, result);
                emit(AgentEvent::ToolExecutionEnd {
                    result: result.clone(),
                });
                vec![result]
            };
            for result in results {
                self.push(Message::ToolResult(result), emit);
            }
            start = end;
        }
        false
    }

    /// Run `calls` side by side and return their results in their order.
    /// The hooks judge them one after another first — a permission prompt
    /// or the reviewer takes them in turn — then the ones let through run
    /// each on a thread of its own, their progress and ends reported as
    /// they come.
    fn run_together(
        &self,
        calls: &[ToolCall],
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> Vec<ToolResultMessage> {
        enum Report {
            Update(String, ToolUpdate),
            Done(usize, ToolResultMessage),
        }
        let ctx = self.tool_context();
        let judgments: Vec<Judgment> = calls
            .iter()
            .map(|call| {
                emit(AgentEvent::ToolExecutionStart { call: call.clone() });
                judge_tool_call(&self.tools, call, hooks, &ctx, cancel)
            })
            .collect();
        let mut results: Vec<Option<ToolResultMessage>> = vec![None; calls.len()];
        let tools = &self.tools;
        std::thread::scope(|scope| {
            let (tx, rx) = std::sync::mpsc::channel();
            for (index, judgment) in judgments.into_iter().enumerate() {
                match judgment {
                    Judgment::Done(result) => {
                        let _ = tx.send(Report::Done(index, result));
                    }
                    Judgment::Run(judged) => {
                        let (tx, ctx) = (tx.clone(), ctx.clone());
                        let id = calls[index].id.clone();
                        scope.spawn(move || {
                            let mut on_update = |update| {
                                let _ = tx.send(Report::Update(id.clone(), update));
                            };
                            let result =
                                run_judged_call(tools, judged, &ctx, cancel, &mut on_update);
                            let _ = tx.send(Report::Done(index, result));
                        });
                    }
                }
            }
            drop(tx);
            for report in rx {
                match report {
                    Report::Update(tool_call_id, update) => {
                        emit(AgentEvent::ToolExecutionUpdate {
                            tool_call_id,
                            update,
                        });
                    }
                    Report::Done(index, result) => {
                        let result = hooks.after_tool_call(&calls[index], result);
                        emit(AgentEvent::ToolExecutionEnd {
                            result: result.clone(),
                        });
                        results[index] = Some(result);
                    }
                }
            }
        });
        results
            .into_iter()
            .map(|result| result.expect("every call of the group reports its end"))
            .collect()
    }

    /// The tool calls of the last assistant message that have no result yet:
    /// those a pause stopped before, including across a restart, since the
    /// transcript itself records them.
    fn unanswered_calls(&self) -> Vec<ToolCall> {
        unanswered_calls(&self.messages)
    }

    fn execute_call(
        &self,
        call: &ToolCall,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> ToolResultMessage {
        emit(AgentEvent::ToolExecutionStart { call: call.clone() });
        let tool_call_id = call.id.clone();
        let mut on_update = |update: ToolUpdate| {
            emit(AgentEvent::ToolExecutionUpdate {
                tool_call_id: tool_call_id.clone(),
                update,
            });
        };
        let ctx = self.tool_context();
        execute_tool(&self.tools, call, hooks, &ctx, cancel, &mut on_update)
    }

    /// What the loop's tools run in.
    fn tool_context(&self) -> ToolContext {
        ToolContext {
            cwd: self.cwd.clone(),
            asker: self.asker.clone(),
            prompter: self.prompter.clone(),
            suggester: self.suggester.clone(),
            shell_run: self.shell_run.clone(),
            session: Some(SessionView {
                id: self.session_id.clone(),
                intent: self.intent.clone(),
                provider: Arc::clone(&self.provider),
                model: self.model.clone(),
            }),
            withdrawn: None,
            panel: self.panel,
            tool: crate::ToolTraits::default(),
            resume: None,
            running: None,
        }
    }

    /// Call the model on the transcript at `thinking`. The streamed reasoning
    /// and answer are watched for a loop; one cuts the call short and comes
    /// back as the report beside the (partial) reply. A stop asked for by the
    /// user is never reported as a loop.
    fn call_model(
        &self,
        thinking: ThinkingLevel,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> (AssistantMessage, Option<LoopReport>) {
        let noted = self
            .context_note
            .as_ref()
            .and_then(|note| note(self.session_id.as_deref()))
            .map(|note| with_note(&self.messages, &note));
        let request = Request {
            model: &self.model,
            system_prompt: &self.system_prompt,
            messages: noted.as_deref().unwrap_or(&self.messages),
            tools: &self.tools.specs(),
            thinking,
        };
        let call_cancel = cancel.child();
        let mut reasoning = LoopGuard::new();
        let mut answer = LoopGuard::new();
        let mut cut = false;
        let reply = self.provider.stream(
            &request,
            &mut |event| {
                let looped = match &event {
                    StreamEvent::ThinkingDelta(delta) => reasoning.feed(delta),
                    StreamEvent::TextDelta(delta) => answer.feed(delta),
                    _ => false,
                };
                emit(AgentEvent::MessageUpdate(event));
                if looped && !call_cancel.is_cancelled() {
                    cut = true;
                    call_cancel.cancel();
                }
            },
            &call_cancel,
        );
        // Only the guard stops the call alone: any other stop of it is a
        // stop of the run.
        if call_cancel.is_cancelled() && !cut {
            cancel.cancel();
        }
        if cancel.is_cancelled() || !cut {
            return (reply, None);
        }
        let report = reasoning.report().or_else(|| {
            // A loop in the answer keeps no reasoning: none of it is cut.
            answer.report().map(|report| LoopReport {
                loop_start: usize::MAX,
                ..report
            })
        });
        (reply, report)
    }

    /// Replace the older part of the transcript with a model-written summary,
    /// keeping the most recent messages verbatim. Fails when there is too
    /// little to summarise or the summary call does not succeed; the
    /// transcript is untouched on failure.
    /// Summarise the older part of the transcript with the compaction
    /// prompts; `focus` is what the user asked `/compact` to concentrate on.
    pub fn compact(
        &mut self,
        reason: CompactionReason,
        focus: Option<&str>,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> Result<(), String> {
        let keep_tokens = self
            .compaction
            .keep_recent_tokens
            .min(self.model.context_window / 4);
        let split = split_point(&self.messages, keep_tokens);
        let tokens_before = context_tokens(&self.messages);
        if split == 0 {
            let error = "too few messages to compact".to_string();
            emit(AgentEvent::CompactionFailed {
                error: error.clone(),
                tokens_before,
                call: None,
                reply: String::new(),
            });
            return Err(error);
        }
        // What is summarised must fit the window itself, with room for the
        // summary: a history already past it is cut down by kinds first,
        // its latest part kept whole.
        let room = chars_for_tokens(
            self.model.context_window.saturating_sub(
                self.compaction
                    .reserve_tokens
                    .min(self.model.context_window / 4),
            ),
        );
        let input = fit(&self.messages[..split], room, room / 4);
        let input_cut = input.steps.clone();
        let mut to_summarize = input.messages;
        emit(AgentEvent::CompactionStart {
            reason,
            prompt_tokens: context_tokens(&to_summarize),
        });

        to_summarize.push(Message::User(UserMessage::text(
            self.compaction_prompts.request.clone(),
        )));
        let system_prompt = self.compaction_prompts.system_prompt(focus);
        let request = Request {
            model: &self.model,
            system_prompt: &system_prompt,
            messages: &to_summarize,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let started = std::time::Instant::now();
        let reply = self.provider.stream(
            &request,
            &mut |event| emit(AgentEvent::CompactionUpdate(event)),
            cancel,
        );
        let call = CompactionCall {
            reason,
            stop_reason: reply.stop_reason,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            usage: reply.usage,
            thinking_chars: reply
                .content
                .iter()
                .map(|block| match block {
                    AssistantContent::Thinking { text, .. } => text.chars().count(),
                    _ => 0,
                })
                .sum(),
            tool_calls: reply.tool_calls().map(|call| call.name.clone()).collect(),
            input_cut,
        };
        let summary = reply.plain_text();
        // A reply that calls a tool went on with the work instead of
        // summarising it, whatever text it has: a line of narration passes
        // for a summary otherwise.
        let went_on = reply.stop_reason == StopReason::ToolUse || !call.tool_calls.is_empty();
        if matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted)
            || went_on
            || summary.trim().chars().count() < MIN_SUMMARY_CHARS
        {
            let error = reply.error_message.unwrap_or_else(|| {
                if went_on {
                    format!(
                        "the model called a tool instead of summarising: {:?}",
                        summary.trim()
                    )
                } else {
                    format!(
                        "the model returned a degenerate summary: {:?}",
                        summary.trim()
                    )
                }
            });
            emit(AgentEvent::CompactionFailed {
                error: error.clone(),
                tokens_before,
                call: Some(call),
                reply: summary.chars().take(REJECTED_REPLY_CHARS).collect(),
            });
            return Err(error);
        }

        let mut tail = self.messages.split_off(split);
        // A kept reply's usage measured the context before the summary
        // replaced its head; left in place, `context_tokens` would keep
        // reporting the old size until the next reply.
        for message in &mut tail {
            if let Message::Assistant(assistant) = message {
                assistant.usage = Usage::default();
            }
        }
        let kept = tail.len();
        self.messages = vec![self.compaction_prompts.summary_message(&summary)];
        self.messages.extend(tail);
        emit(AgentEvent::Compacted {
            summary,
            kept,
            tokens_before,
            tokens_after: context_tokens(&self.messages),
            call,
        });
        Ok(())
    }

    /// Cut the context down to half the window by kinds of content, with no
    /// model call: what a compaction falls back on when its summary call
    /// fails or is refused. The recent messages within the compaction's
    /// kept budget stay whole. Fails when nothing could be left out.
    pub fn fit_context(
        &mut self,
        reason: CompactionReason,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> Result<(), String> {
        let window = self.model.context_window;
        let budget = chars_for_tokens(window / 2);
        let keep_tail = chars_for_tokens(self.compaction.keep_recent_tokens.min(window / 4));
        let tokens_before = context_tokens(&self.messages);
        let fitted = fit(&self.messages, budget, keep_tail);
        if !fitted.changed() {
            return Err("nothing to leave out".to_string());
        }
        self.messages = fitted.messages;
        emit(AgentEvent::Fitted {
            reason,
            budget,
            keep_tail,
            steps: fitted.steps,
            tokens_before,
            tokens_after: context_tokens(&self.messages),
        });
        Ok(())
    }

    /// Ask the judge whether `goal` is reached, given the work so far. A
    /// read-only call on a copy of the transcript with the goal prompts and no
    /// tools; the transcript is untouched. Emits [`AgentEvent::GoalJudged`]
    /// with the verdict, or [`AgentEvent::GoalJudgeFailed`] and an `Err` when
    /// the call does not succeed or its answer holds no verdict.
    pub fn judge(
        &self,
        goal: &str,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> Result<GoalVerdict, String> {
        let mut messages = self.messages.clone();
        messages.push(Message::User(UserMessage::text(
            self.goal_prompt.request.clone(),
        )));
        let system_prompt = self.goal_prompt.system_prompt(goal);
        let request = Request {
            model: &self.model,
            system_prompt: &system_prompt,
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let reply = self.provider.stream(
            &request,
            &mut |event| emit(AgentEvent::GoalJudgeUpdate(event)),
            cancel,
        );
        if reply.usage.total() > 0 {
            emit(AgentEvent::Spent {
                by: Spender::GoalJudge,
                model: self.model.id.clone(),
                usage: reply.usage,
            });
        }
        if matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted) {
            let error = reply
                .error_message
                .unwrap_or_else(|| "the judge call did not complete".to_string());
            emit(AgentEvent::GoalJudgeFailed {
                error: error.clone(),
                reply: String::new(),
            });
            return Err(error);
        }
        let text = reply.plain_text();
        match parse_verdict(&text) {
            Ok(verdict) => {
                emit(AgentEvent::GoalJudged {
                    outcome: verdict.outcome,
                    reason: verdict.reason.clone(),
                    reply: text,
                });
                Ok(verdict)
            }
            Err(error) => {
                emit(AgentEvent::GoalJudgeFailed {
                    error: error.clone(),
                    reply: text,
                });
                Err(error)
            }
        }
    }

    /// Write a handoff brief: a forward-looking summary of the unfinished work
    /// for a fresh session or another agent. A read-only call on a copy of the
    /// transcript with the handoff prompts and no tools; the transcript is
    /// untouched. Emits [`AgentEvent::Handoff`] with the brief, or an error.
    pub fn handoff(&self, cancel: &CancelToken, emit: &mut dyn FnMut(AgentEvent)) {
        let mut messages = self.messages.clone();
        messages.push(Message::User(UserMessage::text(
            self.handoff_prompt.request.clone(),
        )));
        let request = Request {
            model: &self.model,
            system_prompt: &self.handoff_prompt.instructions,
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        let reply = self.provider.stream(&request, &mut |_| {}, cancel);
        if reply.usage.total() > 0 {
            emit(AgentEvent::Spent {
                by: Spender::Handoff,
                model: self.model.id.clone(),
                usage: reply.usage,
            });
        }
        let brief = reply.plain_text();
        let result = if matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted)
            || brief.trim().is_empty()
        {
            Err(reply
                .error_message
                .unwrap_or_else(|| "the handoff call did not produce a brief".to_string()))
        } else {
            Ok(brief.trim().to_string())
        };
        emit(AgentEvent::Handoff { brief: result });
    }

    fn push(&mut self, message: Message, emit: &mut dyn FnMut(AgentEvent)) {
        emit(AgentEvent::MessageEnd(message.clone()));
        if self.intent_seeded {
            self.record_intent(&message);
        }
        self.messages.push(message);
    }

    /// Build the reviewer's log from the transcript, once: a resumed session
    /// arrives as messages, and the builder that sets them may run before
    /// the one that sets the compaction prompts a summary is told by.
    fn seed_intent(&mut self) {
        if self.intent_seeded {
            return;
        }
        self.intent.clear();
        self.intent_seeded = true;
        for message in std::mem::take(&mut self.messages) {
            self.record_intent(&message);
            self.messages.push(message);
        }
    }

    /// Note `message` in the reviewer's log: what the user wrote, the commands
    /// they ran by hand, and the calls the agent made. Results and the agent's
    /// text are left out, and so is a compaction summary, which the model
    /// wrote. A hand-run command is logged as the command alone — its output
    /// is content that came from outside and must not read as intent.
    fn record_intent(&self, message: &Message) {
        match message {
            Message::User(user) => {
                if user.ran.is_none() && self.compaction_prompts.is_summary(&user.plain_text()) {
                    return;
                }
                self.intent
                    .push(IntentEntry::of_message(user, self.delegated));
            }
            Message::Assistant(assistant) => self.intent.push_calls(assistant.tool_calls()),
            Message::ToolResult(_) => {}
        }
    }

    fn drain_steering(&self, emit: &mut dyn FnMut(AgentEvent)) -> Vec<UserMessage> {
        let taken = self.queues.take_steering(self.config.steering_mode);
        if !taken.is_empty() {
            self.emit_queue_update(emit);
        }
        taken
    }

    fn drain_follow_up(&self, emit: &mut dyn FnMut(AgentEvent)) -> Vec<UserMessage> {
        let taken = self.queues.take_follow_up(self.config.follow_up_mode);
        if !taken.is_empty() {
            self.emit_queue_update(emit);
        }
        taken
    }

    fn emit_queue_update(&self, emit: &mut dyn FnMut(AgentEvent)) {
        let (steering, follow_up) = self.queues.lens();
        emit(AgentEvent::QueueUpdate {
            steering,
            follow_up,
        });
    }
}

enum TurnOutcome {
    Continue { had_tool_calls: bool },
    Halt,
}

/// What the model is told after a call that ran to the output limit with
/// nothing to show.
const EMPTY_CUT_OFF_NUDGE: &str = "Your last reply ran to the output limit and nothing usable came of it: no answer and no complete tool call. Do not write it again. Take a smaller step now: one tool call with short arguments (a long file goes in parts), or give your answer.";

/// A reply cut off by the output limit with no answer and no tool call:
/// whatever the model wrote went nowhere.
fn is_empty_cut_off(reply: &AssistantMessage) -> bool {
    reply.stop_reason == StopReason::Length
        && reply.plain_text().trim().is_empty()
        && reply.tool_calls().next().is_none()
}

/// What of a reply cut short enters the transcript: its reasoning, up to
/// `keep` bytes (where a loop began), unsealed (a cut reasoning is no
/// provider's to read back), and nothing else — a half-written answer or
/// call is not one. It ends as cut short by length, not as a failure:
/// nothing went wrong with the request.
fn cut_reasoning(mut reply: AssistantMessage, keep: usize) -> AssistantMessage {
    let mut left = keep;
    reply.content = reply
        .content
        .into_iter()
        .filter_map(|block| match block {
            AssistantContent::Thinking {
                mut text,
                redacted: false,
                ..
            } => {
                let mut keep = left.min(text.len());
                while !text.is_char_boundary(keep) {
                    keep -= 1;
                }
                left -= keep;
                text.truncate(keep);
                let text = text.trim_end().to_string();
                (!text.is_empty()).then_some(AssistantContent::Thinking {
                    text,
                    signature: None,
                    redacted: false,
                })
            }
            _ => None,
        })
        .collect();
    reply.stop_reason = StopReason::Length;
    reply.error_message = None;
    reply.failure = None;
    reply
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("provider", &self.provider.name())
            .field("model", &self.model.id)
            .field("tools", &self.tools.names())
            .field("messages", &self.messages.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Scripted provider and tools shared by the loop and runtime tests.

    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};

    use super::*;
    use crate::message::AssistantContent;
    use crate::tool::Tool;

    /// Replays scripted assistant messages and records every request's
    /// transcript, so tests can assert what the model would have seen.
    #[derive(Default)]
    pub struct ScriptedProvider {
        responses: Mutex<VecDeque<AssistantMessage>>,
        pub seen: Mutex<Vec<Vec<Message>>>,
        /// Cancel the run from inside the stream on the n-th call (0-based).
        pub cancel_on_call: Option<usize>,
        /// The reasoning level each call asked for.
        pub thinking_seen: Mutex<Vec<ThinkingLevel>>,
    }

    impl ScriptedProvider {
        pub fn new(responses: Vec<AssistantMessage>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                ..Self::default()
            }
        }

        pub fn seen_requests(&self) -> Vec<Vec<Message>> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Provider for ScriptedProvider {
        fn name(&self) -> &str {
            "scripted"
        }

        fn stream(
            &self,
            request: &Request<'_>,
            on_event: &mut dyn FnMut(StreamEvent),
            cancel: &CancelToken,
        ) -> AssistantMessage {
            let call_index = {
                let mut seen = self.seen.lock().unwrap();
                seen.push(request.messages.to_vec());
                seen.len() - 1
            };
            self.thinking_seen.lock().unwrap().push(request.thinking);
            if self.cancel_on_call == Some(call_index) {
                cancel.cancel();
            }
            let scripted = self.responses.lock().unwrap().pop_front();
            let Some(message) = scripted else {
                return AssistantMessage::failed(
                    "scripted",
                    &request.model.id,
                    StopReason::Error,
                    "script exhausted",
                );
            };
            for block in &message.content {
                match block {
                    AssistantContent::Text { text } => {
                        on_event(StreamEvent::TextDelta(text.clone()))
                    }
                    AssistantContent::Thinking { text, .. } => {
                        on_event(StreamEvent::ThinkingDelta(text.clone()))
                    }
                    AssistantContent::ToolCall(call) => {
                        on_event(StreamEvent::ToolCallStart {
                            id: call.id.clone(),
                            name: call.name.clone(),
                        });
                        on_event(StreamEvent::ToolCallEnd {
                            id: call.id.clone(),
                        });
                    }
                }
            }
            message
        }
    }

    pub fn text_reply(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![AssistantContent::Text { text: text.into() }],
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            provider: "scripted".into(),
            model: "test".into(),
            error_message: None,
            failure: None,
            timestamp: 0,
        }
    }

    pub fn tool_reply(
        calls: Vec<(&str, &str, Value)>,
        stop_reason: StopReason,
    ) -> AssistantMessage {
        AssistantMessage {
            content: calls
                .into_iter()
                .map(|(id, name, arguments)| {
                    AssistantContent::ToolCall(ToolCall {
                        id: id.into(),
                        name: name.into(),
                        arguments,
                        extra_content: None,
                    })
                })
                .collect(),
            stop_reason,
            usage: Usage::default(),
            provider: "scripted".into(),
            model: "test".into(),
            error_message: None,
            failure: None,
            timestamp: 0,
        }
    }

    pub fn model() -> ModelSpec {
        ModelSpec {
            provider: "scripted".into(),
            id: "test".into(),
            context_window: 8192,
            max_tokens: Some(1024),
            thinking: ThinkingLevel::Off,
        }
    }

    /// Echoes its `text` argument back and records that it ran. Optionally
    /// pushes a steering message when executed, to exercise mid-run queues.
    #[derive(Default)]
    pub struct EchoTool {
        pub executed: Mutex<Vec<String>>,
        pub steer_on_execute: Option<(QueueHandle, String)>,
        /// Requests a pause when executed, to exercise the mid-run pause.
        pub pause_on_execute: Option<QueueHandle>,
    }

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echo the text back"
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }
        fn execute(
            &self,
            call: &ToolCall,
            _ctx: &ToolContext,
            on_update: &mut dyn FnMut(ToolUpdate),
            _cancel: &CancelToken,
        ) -> ToolResultMessage {
            let text = call.arguments["text"].as_str().unwrap_or("").to_string();
            self.executed.lock().unwrap().push(call.id.clone());
            on_update(ToolUpdate::Output(text.clone()));
            if let Some((queues, message)) = &self.steer_on_execute {
                queues.steer(UserMessage::text(message.clone()));
            }
            if let Some(queues) = &self.pause_on_execute {
                queues.pause();
            }
            ToolResultMessage::text(call, format!("echo: {text}"))
        }
    }

    pub fn registry_with(tool: Arc<EchoTool>) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.insert(tool);
        registry
    }

    pub fn collect(agent: &mut Agent, prompt: &str, hooks: &mut dyn Hooks) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        agent.run(
            UserMessage::text(prompt),
            hooks,
            &CancelToken::new(),
            &mut |event| events.push(event),
        );
        events
    }

    pub fn roles(messages: &[Message]) -> Vec<&'static str> {
        messages
            .iter()
            .map(|m| match m {
                Message::User(_) => "user",
                Message::Assistant(_) => "assistant",
                Message::ToolResult(_) => "tool_result",
            })
            .collect()
    }
}

/// The tool calls of the last assistant message in `messages` that have no
/// result after it: those a pause stopped before, or a run cut off left —
/// the transcript itself records them, so this holds across a restart.
#[must_use]
pub fn unanswered_calls(messages: &[Message]) -> Vec<ToolCall> {
    let Some(index) = messages
        .iter()
        .rposition(|m| matches!(m, Message::Assistant(_)))
    else {
        return Vec::new();
    };
    let Message::Assistant(assistant) = &messages[index] else {
        return Vec::new();
    };
    let answered: Vec<&str> = messages[index + 1..]
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(result) => Some(result.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assistant
        .tool_calls()
        .filter(|call| !answered.contains(&call.id.as_str()))
        .cloned()
        .collect()
}
#[cfg(test)]
mod tests;
