//! The agent loop: turns, tool execution, steering and follow-up queues.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::cancel::CancelToken;
use crate::compaction::{
    context_tokens, is_context_overflow_error, should_compact, split_point, CompactionPolicy,
    CompactionPrompts, CompactionReason, MIN_SUMMARY_CHARS,
};
use crate::goal::{parse_verdict, GoalPrompt, GoalVerdict};
use crate::handoff::HandoffPrompt;
use crate::message::{
    AssistantMessage, Message, StopReason, ToolCall, ToolResultMessage, UserMessage,
};
use crate::provider::{ModelSpec, Provider, Request, StreamEvent, ThinkingLevel};
use crate::tool::{ToolContext, ToolRegistry, ToolUpdate};

/// How many queued messages one drain point delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueMode {
    /// Deliver one message per turn boundary, so the model reacts to each.
    #[default]
    OneAtATime,
    /// Deliver everything queued at once.
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

/// Extension points of the loop. All methods have permissive defaults.
///
/// Hooks run on the agent thread, so a permission prompt may block here
/// while it waits for the user's answer.
pub trait Hooks: Send {
    fn before_tool_call(&mut self, _call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
        ToolDecision::Allow
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
}

impl ChainedHooks {
    #[must_use]
    pub fn new(hooks: Vec<Box<dyn Hooks>>) -> Self {
        Self { hooks }
    }
}

impl Hooks for ChainedHooks {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        let mut current = call.clone();
        let mut replaced = false;
        for hook in &mut self.hooks {
            match hook.before_tool_call(&current, ctx) {
                ToolDecision::Allow => {}
                ToolDecision::Replace { arguments } => {
                    current.arguments = arguments;
                    replaced = true;
                }
                ToolDecision::Approve { arguments } => {
                    return ToolDecision::Approve {
                        arguments: arguments.or_else(|| replaced.then_some(current.arguments)),
                    };
                }
                block @ ToolDecision::Block { .. } => return block,
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
    MessageStart,
    MessageUpdate(StreamEvent),
    /// A message was appended to the transcript.
    MessageEnd(Message),
    ToolExecutionStart {
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
    CompactionStart {
        reason: CompactionReason,
    },
    /// The transcript now starts with a summary message followed by the
    /// `kept` most recent messages; consumers mirroring the transcript must
    /// apply the same replacement.
    Compacted {
        summary: String,
        kept: usize,
        tokens_before: u64,
    },
    CompactionFailed {
        error: String,
    },
    /// The autonomous-goal judge ran (`/goal`) and returned its verdict: the
    /// goal is reached, or more work is needed, with a one-line reason.
    GoalJudged {
        done: bool,
        reason: String,
    },
    /// The judge call did not succeed; the goal loop stops.
    GoalJudgeFailed {
        error: String,
    },
    /// A `/handoff` brief was produced (or the call failed): the forward-looking
    /// brief for a fresh session, or an error to report.
    Handoff {
        brief: Result<String, String>,
    },
    /// The loop stopped early on a pause request with work still pending, so
    /// the run can be resumed. Not emitted on a natural finish.
    Paused,
    AgentEnd,
}

/// Transcript owner and loop driver. Single-threaded by design: the runtime
/// moves it onto a worker thread and talks to it through [`QueueHandle`]
/// and [`CancelToken`].
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    model: ModelSpec,
    thinking: ThinkingLevel,
    system_prompt: String,
    cwd: PathBuf,
    messages: Vec<Message>,
    queues: QueueHandle,
    config: AgentConfig,
    compaction: CompactionPolicy,
    compaction_prompts: CompactionPrompts,
    goal_prompt: GoalPrompt,
    handoff_prompt: HandoffPrompt,
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
            thinking: ThinkingLevel::default(),
            system_prompt: String::new(),
            cwd,
            messages: Vec::new(),
            queues: QueueHandle::default(),
            config: AgentConfig::default(),
            compaction: CompactionPolicy::default(),
            compaction_prompts: CompactionPrompts::default(),
            goal_prompt: GoalPrompt::default(),
            handoff_prompt: HandoffPrompt::default(),
        }
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
        self
    }

    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = prompt.into();
    }

    pub fn set_model(&mut self, model: ModelSpec) {
        self.model = model;
    }

    pub fn set_thinking(&mut self, level: ThinkingLevel) {
        self.thinking = level;
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
    pub fn thinking(&self) -> ThinkingLevel {
        self.thinking
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
        let initial = self.drain_steering(emit);
        self.run_from(initial, true, hooks, cancel, emit);
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
        emit(AgentEvent::TurnStart);
        for message in pending {
            self.push(Message::User(message), emit);
        }

        if should_compact(&self.messages, self.model.context_window, &self.compaction) {
            // A failed compaction is reported through events; the turn still
            // runs and may hit the overflow path below.
            let _ = self.compact(CompactionReason::Threshold, None, cancel, emit);
        }

        emit(AgentEvent::MessageStart);
        let mut retried_after_overflow = false;
        let assistant = loop {
            let reply = self.call_model(cancel, emit);
            let overflow = reply.stop_reason == StopReason::Error
                && reply
                    .error_message
                    .as_deref()
                    .is_some_and(is_context_overflow_error);
            if overflow
                && !retried_after_overflow
                && self.compaction.enabled
                && self
                    .compact(CompactionReason::Overflow, None, cancel, emit)
                    .is_ok()
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

    /// Run `calls` in order, pushing each result. Stops before a call once a
    /// pause has been asked for, leaving it and the rest unanswered for a
    /// resume to run; returns whether it stopped so. `truncated` calls come
    /// from a reply cut off by the output limit and are refused, not run.
    fn run_calls(
        &mut self,
        calls: &[ToolCall],
        truncated: bool,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> bool {
        for call in calls {
            if !truncated && !cancel.is_cancelled() && self.queues.is_paused() {
                return true;
            }
            let result = if truncated {
                ToolResultMessage::error(
                    call,
                    "The response was cut off by the output limit, so the tool call arguments may be incomplete. The call was not executed.",
                )
            } else {
                self.execute_call(call, hooks, cancel, emit)
            };
            let result = hooks.after_tool_call(call, result);
            emit(AgentEvent::ToolExecutionEnd {
                result: result.clone(),
            });
            self.push(Message::ToolResult(result), emit);
        }
        false
    }

    /// The tool calls of the last assistant message that have no result yet:
    /// those a pause stopped before, including across a restart, since the
    /// transcript itself records them.
    fn unanswered_calls(&self) -> Vec<ToolCall> {
        let Some(index) = self
            .messages
            .iter()
            .rposition(|m| matches!(m, Message::Assistant(_)))
        else {
            return Vec::new();
        };
        let Message::Assistant(assistant) = &self.messages[index] else {
            return Vec::new();
        };
        let answered: Vec<&str> = self.messages[index + 1..]
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

    fn execute_call(
        &self,
        call: &ToolCall,
        hooks: &mut dyn Hooks,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> ToolResultMessage {
        emit(AgentEvent::ToolExecutionStart { call: call.clone() });
        let ctx = ToolContext {
            cwd: self.cwd.clone(),
        };

        if cancel.is_cancelled() {
            return ToolResultMessage::error(call, "The run was cancelled before this tool ran.");
        }

        let Some(tool) = self.tools.get(&call.name) else {
            log::warn!("model requested unknown tool `{}`", call.name);
            return ToolResultMessage::error(
                call,
                format!(
                    "Unknown tool `{}`. Available tools: {}.",
                    call.name,
                    self.tools.names().join(", ")
                ),
            );
        };

        let effective = match hooks.before_tool_call(call, &ctx) {
            ToolDecision::Allow | ToolDecision::Approve { arguments: None } => call.clone(),
            ToolDecision::Replace { arguments }
            | ToolDecision::Approve {
                arguments: Some(arguments),
            } => ToolCall {
                arguments,
                ..call.clone()
            },
            ToolDecision::Block { reason } => {
                return ToolResultMessage::error(call, format!("Tool call blocked: {reason}"));
            }
        };

        let tool_call_id = call.id.clone();
        let mut on_update = |update: ToolUpdate| {
            emit(AgentEvent::ToolExecutionUpdate {
                tool_call_id: tool_call_id.clone(),
                update,
            });
        };
        tool.execute(&effective, &ctx, &mut on_update, cancel)
    }

    fn call_model(
        &self,
        cancel: &CancelToken,
        emit: &mut dyn FnMut(AgentEvent),
    ) -> AssistantMessage {
        let request = Request {
            model: &self.model,
            system_prompt: &self.system_prompt,
            messages: &self.messages,
            tools: &self.tools.specs(),
            thinking: self.thinking,
        };
        self.provider.stream(
            &request,
            &mut |event| emit(AgentEvent::MessageUpdate(event)),
            cancel,
        )
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
        if split == 0 {
            let error = "too few messages to compact".to_string();
            emit(AgentEvent::CompactionFailed {
                error: error.clone(),
            });
            return Err(error);
        }
        emit(AgentEvent::CompactionStart { reason });
        let tokens_before = context_tokens(&self.messages);

        let mut to_summarize = self.messages[..split].to_vec();
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
        let reply = self.provider.stream(&request, &mut |_| {}, cancel);
        let summary = reply.plain_text();
        if matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted)
            || summary.trim().chars().count() < MIN_SUMMARY_CHARS
        {
            let error = reply.error_message.unwrap_or_else(|| {
                format!(
                    "the model returned a degenerate summary: {:?}",
                    summary.trim()
                )
            });
            emit(AgentEvent::CompactionFailed {
                error: error.clone(),
            });
            return Err(error);
        }

        let tail = self.messages.split_off(split);
        let kept = tail.len();
        self.messages = vec![self.compaction_prompts.summary_message(&summary)];
        self.messages.extend(tail);
        emit(AgentEvent::Compacted {
            summary,
            kept,
            tokens_before,
        });
        Ok(())
    }

    /// Ask the judge whether `goal` is reached, given the work so far. A
    /// read-only call on a copy of the transcript with the goal prompts and no
    /// tools; the transcript is untouched. Emits [`AgentEvent::GoalJudged`]
    /// with the verdict, or [`AgentEvent::GoalJudgeFailed`] and an `Err` when
    /// the call does not succeed.
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
        let reply = self.provider.stream(&request, &mut |_| {}, cancel);
        if matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted) {
            let error = reply
                .error_message
                .unwrap_or_else(|| "the judge call did not complete".to_string());
            emit(AgentEvent::GoalJudgeFailed {
                error: error.clone(),
            });
            return Err(error);
        }
        let verdict = parse_verdict(&reply.plain_text());
        emit(AgentEvent::GoalJudged {
            done: verdict.done,
            reason: verdict.reason.clone(),
        });
        Ok(verdict)
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
        self.messages.push(message);
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
    use crate::message::{AssistantContent, Usage};
    use crate::tool::Tool;

    /// Replays scripted assistant messages and records every request's
    /// transcript, so tests can assert what the model would have seen.
    #[derive(Default)]
    pub struct ScriptedProvider {
        responses: Mutex<VecDeque<AssistantMessage>>,
        pub seen: Mutex<Vec<Vec<Message>>>,
        /// Cancel the run from inside the stream on the n-th call (0-based).
        pub cancel_on_call: Option<usize>,
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
                    AssistantContent::Thinking { text } => {
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
                    })
                })
                .collect(),
            stop_reason,
            usage: Usage::default(),
            provider: "scripted".into(),
            model: "test".into(),
            error_message: None,
            timestamp: 0,
        }
    }

    pub fn model() -> ModelSpec {
        ModelSpec {
            provider: "scripted".into(),
            id: "test".into(),
            context_window: 8192,
            max_tokens: Some(1024),
            reasoning: false,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::test_support::*;
    use super::*;

    fn agent(provider: ScriptedProvider, tools: ToolRegistry) -> (Agent, Arc<ScriptedProvider>) {
        let provider = Arc::new(provider);
        let agent = Agent::new(provider.clone(), tools, model(), PathBuf::from("/tmp"));
        (agent, provider)
    }

    #[test]
    fn text_only_prompt_is_one_turn() {
        let (mut agent, provider) = agent(
            ScriptedProvider::new(vec![text_reply("hello")]),
            ToolRegistry::new(),
        );
        let events = collect(&mut agent, "hi", &mut NoHooks);

        assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
        assert_eq!(provider.seen_requests().len(), 1);
        assert!(matches!(events.first(), Some(AgentEvent::AgentStart)));
        assert!(matches!(events.last(), Some(AgentEvent::AgentEnd)));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::TurnStart))
                .count(),
            1
        );
        assert!(
            events.contains(&AgentEvent::MessageUpdate(StreamEvent::TextDelta(
                "hello".into()
            )))
        );
    }

    #[test]
    fn the_judge_reports_a_verdict_and_leaves_the_transcript_alone() {
        // A "not done" verdict, with the reason on the second line.
        let (agent, provider) = agent(
            ScriptedProvider::new(vec![text_reply("CONTINUE\nthe build still fails")]),
            ToolRegistry::new(),
        );
        let mut events = Vec::new();
        let verdict = agent
            .judge("get the build green", &CancelToken::new(), &mut |e| {
                events.push(e)
            })
            .expect("judge succeeds");
        assert_eq!(
            verdict,
            GoalVerdict {
                done: false,
                reason: "the build still fails".into(),
            }
        );
        assert_eq!(
            events,
            vec![AgentEvent::GoalJudged {
                done: false,
                reason: "the build still fails".into(),
            }]
        );
        // The judge is a read-only call: the transcript is untouched, and the
        // request carried only the appended verdict question.
        assert!(agent.messages().is_empty());
        let seen = provider.seen_requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(roles(&seen[0]), vec!["user"]);
    }

    #[test]
    fn a_done_verdict_ends_the_goal() {
        let (agent, _provider) = agent(
            ScriptedProvider::new(vec![text_reply("DONE: everything compiles and tests pass")]),
            ToolRegistry::new(),
        );
        let verdict = agent
            .judge("ship it", &CancelToken::new(), &mut |_| {})
            .expect("judge succeeds");
        assert!(verdict.done);
        assert_eq!(verdict.reason, "everything compiles and tests pass");
    }

    #[test]
    fn tool_call_result_feeds_the_next_turn() {
        let echo = Arc::new(EchoTool::default());
        let (mut agent, provider) = agent(
            ScriptedProvider::new(vec![
                tool_reply(
                    vec![("c1", "echo", json!({ "text": "ping" }))],
                    StopReason::ToolUse,
                ),
                text_reply("done"),
            ]),
            registry_with(echo.clone()),
        );
        let events = collect(&mut agent, "go", &mut NoHooks);

        assert_eq!(
            roles(agent.messages()),
            vec!["user", "assistant", "tool_result", "assistant"]
        );
        assert_eq!(*echo.executed.lock().unwrap(), vec!["c1"]);
        let second_request = &provider.seen_requests()[1];
        assert!(matches!(
            &second_request[2],
            Message::ToolResult(r) if r.plain_text() == "echo: ping" && !r.is_error
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolExecutionUpdate { tool_call_id, update: ToolUpdate::Output(o) }
                if tool_call_id == "c1" && o == "ping"
        )));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::TurnEnd))
                .count(),
            2
        );
    }

    #[test]
    fn unknown_tool_returns_error_result_and_continues() {
        let (mut agent, _) = agent(
            ScriptedProvider::new(vec![
                tool_reply(vec![("c1", "nope", json!({}))], StopReason::ToolUse),
                text_reply("recovered"),
            ]),
            registry_with(Arc::new(EchoTool::default())),
        );
        collect(&mut agent, "go", &mut NoHooks);

        let Message::ToolResult(result) = &agent.messages()[2] else {
            panic!("expected tool result");
        };
        assert!(result.is_error);
        assert!(result.plain_text().contains("Unknown tool `nope`"));
        assert!(result.plain_text().contains("echo"));
        assert_eq!(agent.messages().len(), 4);
    }

    #[test]
    fn hook_can_block_a_tool_call() {
        struct DenyEcho;
        impl Hooks for DenyEcho {
            fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
                if call.name == "echo" {
                    ToolDecision::Block {
                        reason: "user denied".into(),
                    }
                } else {
                    ToolDecision::Allow
                }
            }
        }

        let echo = Arc::new(EchoTool::default());
        let (mut agent, _) = agent(
            ScriptedProvider::new(vec![
                tool_reply(
                    vec![("c1", "echo", json!({ "text": "x" }))],
                    StopReason::ToolUse,
                ),
                text_reply("ok"),
            ]),
            registry_with(echo.clone()),
        );
        collect(&mut agent, "go", &mut DenyEcho);

        assert!(echo.executed.lock().unwrap().is_empty());
        let Message::ToolResult(result) = &agent.messages()[2] else {
            panic!("expected tool result");
        };
        assert!(result.is_error);
        assert!(result.plain_text().contains("user denied"));
    }

    #[test]
    fn length_stop_fails_tool_calls_without_running_them() {
        let echo = Arc::new(EchoTool::default());
        let (mut agent, _) = agent(
            ScriptedProvider::new(vec![
                tool_reply(
                    vec![("c1", "echo", json!({ "text": "x" }))],
                    StopReason::Length,
                ),
                text_reply("retry"),
            ]),
            registry_with(echo.clone()),
        );
        collect(&mut agent, "go", &mut NoHooks);

        assert!(echo.executed.lock().unwrap().is_empty());
        let Message::ToolResult(result) = &agent.messages()[2] else {
            panic!("expected tool result");
        };
        assert!(result.is_error);
        assert!(result.plain_text().contains("cut off"));
    }

    #[test]
    fn provider_error_halts_the_run() {
        let (mut agent, _) = agent(
            ScriptedProvider::new(vec![AssistantMessage::failed(
                "scripted",
                "test",
                StopReason::Error,
                "boom",
            )]),
            ToolRegistry::new(),
        );
        let events = collect(&mut agent, "go", &mut NoHooks);

        assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
        assert!(matches!(events.last(), Some(AgentEvent::AgentEnd)));
        let Message::Assistant(reply) = &agent.messages()[1] else {
            panic!("expected assistant");
        };
        assert_eq!(reply.error_message.as_deref(), Some("boom"));
    }

    #[test]
    fn cancellation_during_stream_skips_tool_execution() {
        let echo = Arc::new(EchoTool::default());
        let mut provider = ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "x" }))],
                StopReason::ToolUse,
            ),
            text_reply("never"),
        ]);
        provider.cancel_on_call = Some(0);
        let (mut agent, provider) = agent(provider, registry_with(echo.clone()));
        collect(&mut agent, "go", &mut NoHooks);

        assert!(echo.executed.lock().unwrap().is_empty());
        assert_eq!(provider.seen_requests().len(), 1);
        let Message::ToolResult(result) = &agent.messages()[2] else {
            panic!("expected tool result");
        };
        assert!(result.is_error);
        assert!(result.plain_text().contains("cancelled"));
    }

    #[test]
    fn steering_queued_mid_run_is_delivered_before_the_next_model_call() {
        let echo = Arc::new(EchoTool::default());
        let mut registry = ToolRegistry::new();
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "x" }))],
                StopReason::ToolUse,
            ),
            text_reply("adjusted"),
        ]));
        let mut agent = Agent::new(provider.clone(), registry.clone(), model(), "/tmp".into());
        let steering_echo = Arc::new(EchoTool {
            executed: Default::default(),
            steer_on_execute: Some((agent.queues(), "actually, stop after this".into())),
            pause_on_execute: None,
        });
        registry.insert(steering_echo);
        *agent.tools_mut() = registry;
        drop(echo);

        let events = collect(&mut agent, "go", &mut NoHooks);

        assert_eq!(
            roles(agent.messages()),
            vec!["user", "assistant", "tool_result", "user", "assistant"]
        );
        let second_request = &provider.seen_requests()[1];
        assert!(matches!(
            &second_request[3],
            Message::User(u) if u.plain_text() == "actually, stop after this"
        ));
        assert!(events.contains(&AgentEvent::QueueUpdate {
            steering: 0,
            follow_up: 0
        }));
    }

    #[test]
    fn pause_stops_after_the_current_step_and_resume_continues() {
        let mut registry = ToolRegistry::new();
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_reply(
                vec![("c1", "echo", json!({ "text": "x" }))],
                StopReason::ToolUse,
            ),
            text_reply("done after resume"),
        ]));
        let mut agent = Agent::new(provider.clone(), registry.clone(), model(), "/tmp".into());
        let echo = Arc::new(EchoTool {
            executed: Default::default(),
            steer_on_execute: None,
            pause_on_execute: Some(agent.queues()),
        });
        registry.insert(echo);
        *agent.tools_mut() = registry;

        // The tool asks to pause: the loop runs that step, then stops before the
        // next model call, emitting Paused.
        let events = collect(&mut agent, "go", &mut NoHooks);
        assert!(events.contains(&AgentEvent::Paused));
        assert_eq!(
            roles(agent.messages()),
            vec!["user", "assistant", "tool_result"]
        );
        assert_eq!(provider.seen_requests().len(), 1);

        // Resuming continues on the existing transcript — the model is called
        // again with no new user message — and finishes.
        let mut resumed = Vec::new();
        agent.resume(&mut NoHooks, &CancelToken::new(), &mut |e| resumed.push(e));
        assert!(!resumed.contains(&AgentEvent::Paused));
        assert_eq!(provider.seen_requests().len(), 2);
        assert_eq!(
            roles(agent.messages()),
            vec!["user", "assistant", "tool_result", "assistant"]
        );
    }

    /// Two calls in one step, the first asking to pause: the pause stops
    /// before the second instead of waiting out the whole step.
    fn pausing_agent(
        replies: Vec<AssistantMessage>,
    ) -> (Agent, Arc<EchoTool>, Arc<ScriptedProvider>) {
        let mut registry = ToolRegistry::new();
        let provider = Arc::new(ScriptedProvider::new(replies));
        let mut agent = Agent::new(provider.clone(), registry.clone(), model(), "/tmp".into());
        let echo = Arc::new(EchoTool {
            executed: Default::default(),
            steer_on_execute: None,
            pause_on_execute: Some(agent.queues()),
        });
        registry.insert(echo.clone());
        *agent.tools_mut() = registry;
        (agent, echo, provider)
    }

    fn two_calls() -> AssistantMessage {
        tool_reply(
            vec![
                ("c1", "echo", json!({ "text": "a" })),
                ("c2", "echo", json!({ "text": "b" })),
            ],
            StopReason::ToolUse,
        )
    }

    #[test]
    fn a_pause_stops_between_the_calls_of_a_step_and_resume_runs_the_rest() {
        let (mut agent, echo, provider) =
            pausing_agent(vec![two_calls(), text_reply("done after resume")]);
        let events = collect(&mut agent, "go", &mut NoHooks);
        assert!(events.contains(&AgentEvent::Paused));
        // Only the first call ran; the second waits, unanswered.
        assert_eq!(*echo.executed.lock().unwrap(), vec!["c1"]);
        assert_eq!(
            roles(agent.messages()),
            vec!["user", "assistant", "tool_result"]
        );

        // Resuming runs the waiting call first, then asks the model.
        let mut resumed = Vec::new();
        agent.resume(&mut NoHooks, &CancelToken::new(), &mut |e| resumed.push(e));
        assert_eq!(*echo.executed.lock().unwrap(), vec!["c1", "c2"]);
        assert_eq!(provider.seen_requests().len(), 2);
        assert_eq!(
            roles(agent.messages()),
            vec![
                "user",
                "assistant",
                "tool_result",
                "tool_result",
                "assistant"
            ]
        );
    }

    #[test]
    fn a_new_request_over_a_pause_answers_the_calls_it_left() {
        let (mut agent, echo, provider) = pausing_agent(vec![two_calls(), text_reply("fresh")]);
        collect(&mut agent, "go", &mut NoHooks);
        // A new request instead of a resume: the waiting call is not run, but
        // still gets a result, so the transcript stays valid.
        collect(&mut agent, "something else", &mut NoHooks);
        assert_eq!(*echo.executed.lock().unwrap(), vec!["c1"]);
        let seen = provider.seen_requests();
        assert_eq!(
            roles(&seen[1]),
            vec!["user", "assistant", "tool_result", "tool_result", "user"]
        );
        assert!(agent.messages().iter().any(|m| matches!(
            m,
            Message::ToolResult(r) if r.tool_call_id == "c2" && r.is_error
        )));
    }

    #[test]
    fn one_at_a_time_delivers_a_single_steering_message_per_turn() {
        let (mut agent, provider) = agent(
            ScriptedProvider::new(vec![
                text_reply("first"),
                text_reply("second"),
                text_reply("third"),
            ]),
            ToolRegistry::new(),
        );
        agent.queues().steer(UserMessage::text("s1"));
        agent.queues().steer(UserMessage::text("s2"));

        collect(&mut agent, "go", &mut NoHooks);

        // Prompt + s1 share the first turn (steering is polled before the
        // first call), s2 gets its own turn, then nothing is left.
        let seen = provider.seen_requests();
        assert_eq!(seen.len(), 2);
        assert_eq!(roles(&seen[0]), vec!["user", "user"]);
        assert_eq!(roles(&seen[1]), vec!["user", "user", "assistant", "user"]);
        assert_eq!(agent.queues().lens(), (0, 0));
    }

    #[test]
    fn all_mode_drains_every_steering_message_at_once() {
        let (mut agent, provider) = agent(
            ScriptedProvider::new(vec![text_reply("first")]),
            ToolRegistry::new(),
        );
        agent.set_config(AgentConfig {
            steering_mode: QueueMode::All,
            follow_up_mode: QueueMode::All,
        });
        agent.queues().steer(UserMessage::text("s1"));
        agent.queues().steer(UserMessage::text("s2"));

        collect(&mut agent, "go", &mut NoHooks);

        let seen = provider.seen_requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(roles(&seen[0]), vec!["user", "user", "user"]);
    }

    #[test]
    fn follow_up_runs_only_after_the_agent_would_stop() {
        let echo = Arc::new(EchoTool::default());
        let (mut agent, provider) = agent(
            ScriptedProvider::new(vec![
                tool_reply(
                    vec![("c1", "echo", json!({ "text": "x" }))],
                    StopReason::ToolUse,
                ),
                text_reply("finished task one"),
                text_reply("finished task two"),
            ]),
            registry_with(echo),
        );
        agent.queues().follow_up(UserMessage::text("now task two"));

        let events = collect(&mut agent, "task one", &mut NoHooks);

        let seen = provider.seen_requests();
        assert_eq!(seen.len(), 3);
        // The follow-up is absent while tool calls are still being served.
        assert_eq!(roles(&seen[1]), vec!["user", "assistant", "tool_result"]);
        assert!(matches!(
            seen[2].last(),
            Some(Message::User(u)) if u.plain_text() == "now task two"
        ));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::AgentEnd))
                .count(),
            1
        );
    }

    #[test]
    fn should_stop_after_turn_ends_the_run_early() {
        struct StopAfterFirst;
        impl Hooks for StopAfterFirst {
            fn should_stop_after_turn(&mut self, _message: &AssistantMessage) -> bool {
                true
            }
        }

        let (mut agent, provider) = agent(
            ScriptedProvider::new(vec![
                tool_reply(
                    vec![("c1", "echo", json!({ "text": "x" }))],
                    StopReason::ToolUse,
                ),
                text_reply("unreachable"),
            ]),
            registry_with(Arc::new(EchoTool::default())),
        );
        collect(&mut agent, "go", &mut StopAfterFirst);

        assert_eq!(provider.seen_requests().len(), 1);
        assert_eq!(
            roles(agent.messages()),
            vec!["user", "assistant", "tool_result"]
        );
    }

    #[test]
    fn threshold_compaction_runs_before_the_model_call() {
        let (mut agent, provider) = agent(
            ScriptedProvider::new(vec![
                text_reply("first answer"),
                text_reply(
                    "SUMMARY OF EARLIER WORK: the user sent a long prompt and got an answer.",
                ),
                text_reply("second answer"),
            ]),
            ToolRegistry::new(),
        );
        // A 200-token window with the reserve capped at a quarter leaves a
        // 150-token threshold; the 1 000-char prompt (~250 tokens) crosses it
        // on the second run.
        agent.set_model(ModelSpec {
            context_window: 200,
            ..model()
        });
        agent.set_compaction(CompactionPolicy {
            reserve_tokens: 8092,
            keep_recent_tokens: 1,
            ..Default::default()
        });
        let long_prompt = "x".repeat(1000);
        collect(&mut agent, &long_prompt, &mut NoHooks);
        let events = collect(&mut agent, "next", &mut NoHooks);

        assert!(events.contains(&AgentEvent::CompactionStart {
            reason: CompactionReason::Threshold
        }));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::Compacted { summary, kept: 1, .. } if summary.starts_with("SUMMARY OF EARLIER WORK")
        )));
        let seen = provider.seen_requests();
        assert_eq!(seen.len(), 3);
        // The summary call ends with the summary request and carries no tail.
        assert!(matches!(
            seen[1].last(),
            Some(Message::User(u)) if u.plain_text() == CompactionPrompts::default().request
        ));
        // The new prompt is already in the transcript and is the kept tail,
        // so the summarised part is the first exchange.
        assert_eq!(roles(&seen[1]), vec!["user", "assistant", "user"]);
        // The real call starts from the summary and keeps the new prompt.
        assert!(matches!(
            &seen[2][0],
            Message::User(u) if u.plain_text().starts_with("Summary of the earlier conversation")
        ));
        assert_eq!(roles(&seen[2]), vec!["user", "user"]);
        assert_eq!(roles(agent.messages()), vec!["user", "user", "assistant"]);
    }

    #[test]
    fn overflow_error_compacts_and_retries_once() {
        let (agent, provider) = agent(
            ScriptedProvider::new(vec![
                AssistantMessage::failed(
                    "scripted",
                    "test",
                    StopReason::Error,
                    "HTTP 400: maximum context length exceeded",
                ),
                text_reply("SUMMARY: the old question was answered; the new question is pending."),
                text_reply("recovered"),
            ]),
            ToolRegistry::new(),
        );
        let mut agent = agent.with_messages(vec![
            Message::User(UserMessage::text("old question")),
            Message::Assistant(text_reply("old answer")),
        ]);
        let events = collect(&mut agent, "new question", &mut NoHooks);

        assert!(events.contains(&AgentEvent::CompactionStart {
            reason: CompactionReason::Overflow
        }));
        assert_eq!(provider.seen_requests().len(), 3);
        let Message::Assistant(last) = agent.messages().last().unwrap() else {
            panic!("expected assistant");
        };
        assert_eq!(last.plain_text(), "recovered");
        assert!(
            agent.messages().iter().all(|m| !matches!(
                m,
                Message::Assistant(a) if a.stop_reason == StopReason::Error
            )),
            "the failed reply never entered the transcript"
        );
    }

    #[test]
    fn failed_compaction_leaves_the_transcript_alone() {
        let (agent, _) = agent(
            ScriptedProvider::new(vec![AssistantMessage::failed(
                "scripted",
                "test",
                StopReason::Error,
                "boom",
            )]),
            ToolRegistry::new(),
        );
        let mut agent = agent.with_messages(vec![
            Message::User(UserMessage::text("a")),
            Message::Assistant(text_reply("b")),
        ]);
        let mut events = Vec::new();
        let result = agent.compact(
            CompactionReason::Manual,
            None,
            &CancelToken::new(),
            &mut |e| events.push(e),
        );
        assert!(result.is_err());
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::CompactionFailed { .. })));
        assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);

        let mut empty = Agent::new(
            Arc::new(ScriptedProvider::default()),
            ToolRegistry::new(),
            model(),
            PathBuf::from("/tmp"),
        );
        assert!(empty
            .compact(
                CompactionReason::Manual,
                None,
                &CancelToken::new(),
                &mut |_| {}
            )
            .is_err());
    }

    #[test]
    fn clear_queue_returns_pending_messages() {
        let queues = QueueHandle::default();
        queues.steer(UserMessage::text("a"));
        queues.follow_up(UserMessage::text("b"));
        queues.follow_up(UserMessage::text("c"));
        assert!(queues.has_pending());

        let (steering, follow_up) = queues.clear();
        assert_eq!(steering.len(), 1);
        assert_eq!(follow_up.len(), 2);
        assert!(!queues.has_pending());
    }
    #[test]
    fn chained_hooks_thread_replacements_and_stop_at_a_verdict() {
        struct Rewriter;
        impl Hooks for Rewriter {
            fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
                ToolDecision::Replace {
                    arguments: serde_json::json!({ "command": format!("{} --dry-run", call.arguments["command"].as_str().unwrap()) }),
                }
            }
            fn after_tool_call(
                &mut self,
                _call: &ToolCall,
                mut result: ToolResultMessage,
            ) -> ToolResultMessage {
                result.content.push(crate::ToolResultContent::Text {
                    text: " +rewriter".into(),
                });
                result
            }
        }
        struct Judge(Vec<String>);
        impl Hooks for Judge {
            fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
                let command = call.arguments["command"].as_str().unwrap().to_string();
                self.0.push(command.clone());
                if command.contains("rm") {
                    ToolDecision::Block {
                        reason: "no rm".into(),
                    }
                } else if command.contains("ls") {
                    ToolDecision::Approve { arguments: None }
                } else {
                    ToolDecision::Allow
                }
            }
            fn should_stop_after_turn(&mut self, _message: &AssistantMessage) -> bool {
                true
            }
        }
        struct Never;
        impl Hooks for Never {
            fn before_tool_call(&mut self, _call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
                panic!("a verdict must stop the chain");
            }
        }
        let call = |command: &str| ToolCall {
            id: "c".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": command }),
        };
        let ctx = ToolContext {
            cwd: PathBuf::from("/p"),
        };

        // Replace flows into the next hook and out of the chain.
        let mut chain = ChainedHooks::new(vec![Box::new(Rewriter), Box::new(Judge(vec![]))]);
        assert_eq!(
            chain.before_tool_call(&call("cargo build"), &ctx),
            ToolDecision::Replace {
                arguments: serde_json::json!({ "command": "cargo build --dry-run" })
            }
        );
        // Approve keeps the rewritten arguments and skips the rest.
        let mut chain = ChainedHooks::new(vec![
            Box::new(Rewriter),
            Box::new(Judge(vec![])),
            Box::new(Never),
        ]);
        assert_eq!(
            chain.before_tool_call(&call("ls"), &ctx),
            ToolDecision::Approve {
                arguments: Some(serde_json::json!({ "command": "ls --dry-run" }))
            }
        );
        assert!(matches!(
            chain.before_tool_call(&call("rm x"), &ctx),
            ToolDecision::Block { .. }
        ));
        let result =
            chain.after_tool_call(&call("ls"), ToolResultMessage::text(&call("ls"), "out"));
        assert_eq!(result.plain_text(), "out +rewriter");
        assert!(chain.should_stop_after_turn(&text_reply("x")));
        let mut plain = ChainedHooks::new(vec![Box::new(NoHooks)]);
        assert_eq!(
            plain.before_tool_call(&call("x"), &ctx),
            ToolDecision::Allow
        );
        assert!(!plain.should_stop_after_turn(&text_reply("x")));
    }
}
