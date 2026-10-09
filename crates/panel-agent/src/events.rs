//! Runtime events: how each [`AgentEvent`] lands in the transcript, the
//! session log and the live activity indicators.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use termide_agent_core::{
    AgentEvent, Message, StopReason, StreamEvent, Timing, ToolResultMessage, ToolUpdate,
};
use termide_core::PanelEvent;
use termide_ui::ChoiceForm;

use crate::pending::Pending;
use crate::{millis, now_hms, transcript, Activity, AgentPanel, Item, NoticeKind, Phase};

/// A finished run shorter than this rings no bell: the user is most likely
/// still there, and a bell on every quick answer would be noise.
const LONG_RUN: Duration = Duration::from_secs(10);

impl AgentPanel {
    /// Wait for the user: highlight the header and, when `ring`, ask the app
    /// for the bell, once until the user has seen the panel. The app drops
    /// the request when the panel is in front of the user.
    pub(crate) fn raise_attention(&mut self, ring: bool) {
        self.attention = true;
        if ring && !self.rung {
            self.rung = true;
            self.pending_events.push(PanelEvent::RequestAttention);
        }
    }

    /// Note streamed output: enter the generating phase on the first token,
    /// then count characters for the live token estimate and speed.
    pub(crate) fn note_generation(&mut self, chars: usize) {
        let activity = self
            .activity
            .get_or_insert_with(|| Activity::new(Phase::Generating));
        activity.first_token.get_or_insert_with(Instant::now);
        if activity.phase != Phase::Generating {
            activity.enter(Phase::Generating);
        }
        activity.gen_chars += chars;
    }

    /// Switch the current activity to `phase` (starting one if idle).
    pub(crate) fn set_phase(&mut self, phase: Phase) {
        match &mut self.activity {
            Some(activity) => activity.enter(phase),
            None => self.activity = Some(Activity::new(phase)),
        }
    }

    /// A `/compact` between runs leaves no activity behind; one inside a run
    /// is replaced by the next message's.
    fn end_compaction(&mut self) {
        if !self.busy {
            self.activity = None;
        }
    }

    /// Apply one runtime event to the transcript and the session log.
    pub(crate) fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::AgentStart => {
                self.busy = true;
                self.paused = false;
                // A resumed run keeps its start, so its clock counts from the
                // request; anything else (a new prompt over a pause) starts
                // afresh.
                self.end_pause();
                if !std::mem::take(&mut self.resuming) || self.run_start.is_none() {
                    self.run_start = Some(Instant::now());
                    self.run_failed = false;
                }
                self.last_failure = None;
                self.run_paused = false;
                // A pause taken up from the log ends with any run that starts.
                self.restored = None;
                self.run_start_due = true;
            }
            AgentEvent::Paused => {
                // The run's closing line records the pause; the state strip
                // shows it until `/continue`.
                self.paused = true;
                self.run_paused = true;
                self.pause_requested = false;
            }
            AgentEvent::AgentEnd => {
                self.busy = false;
                self.activity = None;
                self.log_run_end();
                // A failure is acted on unless the user stopped the run.
                let failure = self.last_failure.take().filter(|_| !self.stop_requested);
                // A stop was the user's own doing, so they are there.
                let long = self
                    .run_start
                    .is_some_and(|start| start.elapsed() >= LONG_RUN);
                self.raise_attention(long && !self.stop_requested);
                if self.run_paused {
                    // The run waits at a pause: its line ticks the pause's
                    // length (no time of day), and the run's own clock stays
                    // for `/continue`.
                    self.pause_start = Some(Instant::now());
                    self.transcript.end_run(0, "", !self.run_failed, true);
                } else if let Some(start) = self.run_start.take() {
                    self.transcript.end_run(
                        millis(start.elapsed()),
                        &now_hms(),
                        !self.run_failed,
                        false,
                    );
                }
                self.pause_requested = false;
                self.stop_requested = false;
                self.set_queued(self.runtime.queue_lens());
                if let Some(store) = &self.checkpoints {
                    store
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .end_run();
                }
                if self.prompt_stale {
                    self.sync_system_prompt();
                }
                // A failed request is tried again, waited on or asked about;
                // one that went through ends the tries.
                let held = match failure {
                    Some((failure, error)) => {
                        self.retry_ready = true;
                        self.on_run_failed(failure, &error)
                    }
                    None => {
                        if !self.run_failed {
                            self.retry_attempts = 0;
                            self.restarted = false;
                            self.retry_ready = false;
                        }
                        false
                    }
                };
                self.offer_plan();
                // A loop schedules its next iteration once the run ends, unless
                // it was paused (then it waits for `/continue`), a card is up
                // or a failed request waits to be tried again.
                if !self.paused && self.pending.is_none() && self.retry_wait.is_none() {
                    if let Some(task) = self.loop_task.as_mut() {
                        task.next_at =
                            Some(Instant::now() + task.interval.unwrap_or(Duration::ZERO));
                        task.due_ms = task.interval.map(|wait| {
                            termide_agent_core::now_millis()
                                .saturating_add(u64::try_from(wait.as_millis()).unwrap_or(u64::MAX))
                        });
                    }
                }
                // A goal judges the finished work turn next, unless it was
                // paused or a card is up; a turn that errored stops the goal
                // rather than looping on the failure — unless the failed
                // request is still in hand, waited on or asked about.
                if self.goal_task.is_some() && self.goal_errored && !held {
                    self.goal_task = None;
                    self.notice(
                        termide_i18n::t().agent_notice_goal_stopped_failed(),
                        NoticeKind::Warn,
                    );
                } else if !self.paused
                    && self.pending.is_none()
                    && self.retry_wait.is_none()
                    && !self.goal_errored
                {
                    if let Some(task) = self.goal_task.as_mut() {
                        task.judge_at = Some(Instant::now());
                    }
                }
            }
            AgentEvent::TurnStart | AgentEvent::TurnEnd => {}
            AgentEvent::MessageStart { prompt_tokens } => {
                // The reasoning and answer blocks are created lazily on their
                // first delta, so the reasoning lands above the answer and a
                // prefill with neither shows only the live footer.
                self.activity = Some(Activity {
                    prompt_tokens,
                    ..Activity::new(Phase::Prefill)
                });
            }
            AgentEvent::MessageUpdate(StreamEvent::TextDelta(delta)) => {
                self.note_generation(delta.chars().count());
                self.transcript.stream_answer(&delta);
            }
            AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(delta)) => {
                self.note_generation(delta.chars().count());
                self.transcript.stream_thinking(&delta);
            }
            AgentEvent::MessageUpdate(StreamEvent::PrefillProgress {
                processed,
                total,
                cached,
            }) => {
                if let Some(activity) = self.activity.as_mut().filter(|a| a.phase == Phase::Prefill)
                {
                    activity.prefill = Some((processed, total, cached));
                }
            }
            AgentEvent::MessageUpdate(
                event @ (StreamEvent::Queued { .. } | StreamEvent::Admitted),
            ) => {
                if let Some(activity) = self.activity.as_mut().filter(|a| a.phase == Phase::Prefill)
                {
                    activity.note_queue(&event);
                }
            }
            AgentEvent::MessageUpdate(StreamEvent::Retry {
                attempt,
                max_attempts,
                delay_ms,
                error,
            })
            | AgentEvent::CompactionUpdate(StreamEvent::Retry {
                attempt,
                max_attempts,
                delay_ms,
                error,
            }) => self.notice(
                termide_i18n::t().agent_notice_retry_fmt(
                    attempt as usize,
                    max_attempts as usize,
                    delay_ms,
                    &error.to_string(),
                ),
                NoticeKind::Warn,
            ),
            AgentEvent::MessageUpdate(_) => {}
            AgentEvent::MessageEnd(message) => {
                // How long the message took, logged with it so a reopened
                // session shows the same figures.
                let timing = match &message {
                    Message::User(user) => {
                        // A command the user ran while the agent worked was
                        // steered in, and shows as the shell call block it
                        // would have shown had the agent been idle.
                        let item = transcript::user_command_item(user, now_hms(), None)
                            .unwrap_or_else(|| Item::User {
                                text: user.plain_text(),
                                at: now_hms(),
                                command: user.command.clone(),
                            });
                        self.transcript.push(item);
                        None
                    }
                    // An external agent's calls show as they run; the
                    // message of them, sent once one has run, is for the log
                    // alone, and must not close what streams meanwhile.
                    Message::Assistant(assistant)
                        if self.external && assistant.tool_calls().next().is_some() =>
                    {
                        None
                    }
                    Message::Assistant(assistant) => {
                        if assistant.usage.total() > 0 {
                            self.context_tokens = assistant.usage.total();
                        }
                        // What the cache served is counted apart from what is
                        // billed in full.
                        self.session_input += assistant.usage.uncached();
                        self.session_cached += assistant.usage.cache_read;
                        self.session_output += assistant.usage.output;
                        // A call that failed before its first token (no network,
                        // say) went through no prefill or generation: it has
                        // no cost to show, only its time and failure. Nor has
                        // an external agent's message: its first text arrives
                        // with the message's start and it reports no tokens, so
                        // prefill, generation and speed would all be made up.
                        let cost = self
                            .activity
                            .as_ref()
                            .filter(|_| !self.external)
                            .filter(|a| a.first_token.is_some() || assistant.usage.total() > 0)
                            .map(|a| a.cost(assistant.usage.input, assistant.usage.output));
                        let at = now_hms();
                        let error = assistant.error_message.clone();
                        // A goal work turn that errored must not be judged and
                        // retried on the failure; note it for `AgentEnd`.
                        // A goal still waiting for its first turn did not run.
                        if error.is_some()
                            && self
                                .goal_task
                                .as_ref()
                                .is_some_and(|task| task.first_turn.is_none())
                        {
                            self.goal_errored = true;
                        }
                        if error.is_some()
                            || matches!(
                                assistant.stop_reason,
                                StopReason::Error | StopReason::Aborted
                            )
                        {
                            self.run_failed = true;
                        }
                        // What a failure was, for the run's end; a refusal
                        // carries no error and is no failure to retry.
                        self.last_failure = error.as_ref().and_then(|error| {
                            assistant
                                .classify_failure()
                                .map(|failure| (failure, error.clone()))
                        });
                        // The answer always carries the wall-clock time; a
                        // reasoning block, if any, carries the prefill/generation
                        // indicators (else the answer does). A tool-only turn
                        // (reasoning, no answer text) leaves no answer block.
                        let had_thinking = self.transcript.finish_thinking(&at, cost);
                        let answer_cost = if had_thinking { None } else { cost };
                        self.transcript.finish_assistant(
                            assistant.plain_text(),
                            error,
                            answer_cost,
                            at,
                            had_thinking,
                        );
                        cost.map(|cost| Timing::Turn {
                            prefill_ms: cost.prefill_ms,
                            gen_ms: cost.gen_ms,
                        })
                    }
                    // The call's end came first and timed it.
                    Message::ToolResult(result) => self
                        .transcript
                        .tool_duration(&result.tool_call_id)
                        .map(|duration_ms| Timing::Tool {
                            duration_ms,
                            waited_ms: self.transcript.tool_wait(&result.tool_call_id),
                        }),
                };
                if let Some(session) = &mut self.session {
                    if let Err(error) = session.append_timed_message(&message, timing) {
                        log::warn!("agent session write failed: {error}");
                    }
                }
                self.log_run_start();
            }
            AgentEvent::ToolExecutionStart { call } => {
                self.set_phase(Phase::Tool);
                self.tool_starts.insert(call.id.clone(), Instant::now());
                self.transcript.push(Item::Tool {
                    call,
                    result: None,
                    live: None,
                    at: String::new(),
                    duration_ms: None,
                    waited_ms: None,
                    waiting: false,
                });
            }
            AgentEvent::ToolCallUpdate { call: fuller } => {
                self.transcript.with_tool(&fuller.id.clone(), |item| {
                    if let Item::Tool { call, .. } = item {
                        *call = fuller;
                    }
                });
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                update: ToolUpdate::Output(output),
            } => {
                self.transcript.with_tool(&tool_call_id, |item| {
                    if let Item::Tool { live, .. } = item {
                        *live = Some(output);
                    }
                });
            }
            AgentEvent::ToolExecutionEnd { result } => {
                // A subagent's tokens join the session's totals; its context
                // was its own, so the context fill stays as it is.
                if let Some(spent) = result.spent() {
                    self.session_input += spent.uncached();
                    self.session_cached += spent.cache_read;
                    self.session_output += spent.output;
                }
                if let Some(path) = changed_file(&result) {
                    self.pending_events
                        .push(PanelEvent::FileChangedOnDisk(path));
                }
                // Tally how much the output cleaning saved (bash reports the
                // raw and cleaned byte counts in its details).
                if let Some(details) = result.details.as_ref() {
                    if let (Some(raw), Some(clean)) = (
                        details.get("raw_bytes").and_then(|v| v.as_u64()),
                        details.get("cleaned_bytes").and_then(|v| v.as_u64()),
                    ) {
                        self.clean_raw_bytes += raw;
                        self.clean_out_bytes += clean;
                    }
                }
                let id = result.tool_call_id.clone();
                let finished = now_hms();
                // A wait on a permission answer is the call's pause, not its
                // run time.
                self.end_permission_wait();
                // A question's call ends only once it is answered or its run
                // stopped; a card still up then has no one waiting for it.
                if matches!(
                    self.pending,
                    Some(Pending::Question { .. } | Pending::Suggestion { .. })
                ) {
                    self.pending = None;
                }
                let waited = self.transcript.tool_wait(&id).unwrap_or(0);
                let elapsed = self
                    .tool_starts
                    .remove(&id)
                    .map(|start| millis(start.elapsed()).saturating_sub(waited));
                self.transcript.with_tool(&id, |item| {
                    if let Item::Tool {
                        result: slot,
                        live,
                        at,
                        duration_ms,
                        ..
                    } = item
                    {
                        *slot = Some(result);
                        *live = None;
                        *at = finished;
                        *duration_ms = elapsed;
                    }
                });
            }
            AgentEvent::QueueUpdate {
                steering,
                follow_up,
            } => self.set_queued((steering, follow_up)),
            AgentEvent::CompactionStart { prompt_tokens, .. } => {
                // The summary call shows the same prefill and generation lines
                // as a model message, under its own phase.
                self.activity = Some(Activity {
                    prompt_tokens: Some(prompt_tokens),
                    ..Activity::new(Phase::Compact)
                });
                self.notice(
                    termide_i18n::t().agent_notice_compacting(),
                    NoticeKind::Info,
                )
            }
            AgentEvent::CompactionUpdate(event) => {
                if let Some(activity) = self.activity.as_mut().filter(|a| a.phase == Phase::Compact)
                {
                    match event {
                        StreamEvent::TextDelta(delta) | StreamEvent::ThinkingDelta(delta) => {
                            activity.first_token.get_or_insert_with(Instant::now);
                            activity.gen_chars += delta.chars().count();
                        }
                        StreamEvent::PrefillProgress {
                            processed,
                            total,
                            cached,
                        } => activity.prefill = Some((processed, total, cached)),
                        StreamEvent::Queued { .. } | StreamEvent::Admitted => {
                            activity.note_queue(&event);
                        }
                        _ => {}
                    }
                }
            }
            AgentEvent::Compacted {
                summary,
                kept,
                tokens_before,
                tokens_after,
            } => {
                // The next reply's usage would correct the figure only once
                // the next turn finishes.
                self.context_tokens = tokens_after;
                self.end_compaction();
                self.notice(
                    termide_i18n::t().agent_notice_compacted_fmt(tokens_before, kept),
                    NoticeKind::Info,
                );
                if let Some(session) = &mut self.session {
                    if let Err(error) = session.append_compaction(&summary, tokens_before, kept) {
                        log::warn!("agent session write failed: {error}");
                    }
                }
                // The conversation is re-read anyway: what is refused can leave
                // the context almost for free once the run is between turns.
                if self.toolset_off != self.context_off {
                    self.context_stale = true;
                }
            }
            AgentEvent::CompactionFailed { error } => {
                self.end_compaction();
                self.notice(
                    termide_i18n::t().agent_notice_compaction_failed_fmt(&error.to_string()),
                    NoticeKind::Warn,
                );
            }
            AgentEvent::ExternalSession { agent, session_id } => {
                if let Some(session) = &mut self.session {
                    if let Err(error) = session.append_external_session(&agent, &session_id) {
                        log::warn!("agent session write failed: {error}");
                    }
                }
            }
            AgentEvent::GoalJudged { done, reason } => self.on_goal_verdict(done, &reason),
            AgentEvent::GoalJudgeFailed { error } => self.on_goal_judge_failed(&error),
            AgentEvent::Handoff { brief } => match brief {
                Ok(text) => {
                    // Offer the brief, with what to do with it; the text is kept
                    // on the card until the choice is made.
                    let t = termide_i18n::t();
                    let form = ChoiceForm::new(
                        t.agent_handoff_ready_title(),
                        vec![
                            t.agent_handoff_save().to_string(),
                            t.agent_handoff_new_session().to_string(),
                        ],
                    )
                    .with_detail(text.clone())
                    .with_cancel(t.agent_handoff_dismiss());
                    self.pending = Some(Pending::Handoff { form, brief: text });
                    self.raise_attention(true);
                }
                Err(error) => self.notice(
                    termide_i18n::t().agent_notice_handoff_failed_fmt(&error.to_string()),
                    NoticeKind::Warn,
                ),
            },
        }
    }

    /// The body of [`Panel::tick`]: drain the runtime's events, poll the
    /// cards, pickers and probes, and run a due `/loop` or `/goal` step.
    pub(crate) fn on_tick(&mut self) -> Vec<PanelEvent> {
        let mut changed = false;
        // A pause's line ticks its length, redrawn once a second; so does a
        // call's wait on a permission question, until the question is gone.
        if let Some(start) = self.pause_start {
            changed |= self.transcript.set_pause_length(millis(start.elapsed()));
        }
        if self.context_stale && !self.is_busy() {
            self.refresh_context();
            changed = true;
        }
        changed |= self.follow_open_sessions();
        changed |= self.drop_withdrawn_permission();
        if let Some((start, before)) = self.permission_wait {
            if matches!(
                self.pending,
                Some(
                    Pending::Permission { .. }
                        | Pending::Question { .. }
                        | Pending::Suggestion { .. }
                )
            ) {
                changed |= self
                    .transcript
                    .set_tool_wait(before.saturating_add(millis(start.elapsed())), true);
            } else {
                self.end_permission_wait();
                changed = true;
            }
        }
        for event in self.runtime.drain() {
            self.apply(event);
            changed = true;
        }
        changed |= self.take_reviewer_spent();
        let mut events = self.poll_permissions();
        events.append(&mut self.poll_questions());
        events.append(&mut self.poll_suggestions());
        events.append(&mut self.pending_events);
        let tools_changed = self.poll_late_tools();
        changed |= tools_changed;
        // A toolset checklist this panel raised and that still stands open
        // keeps up: what arrived while the user was reading it shows there
        // too, not only in the session.
        if tools_changed && self.toolset_list_open {
            events.push(self.toolset_refresh());
        }
        changed |= self.poll_command();
        changed |= self.poll_shell_job();
        let fetched = self.model_fetch.as_ref().map(Receiver::try_recv);
        match fetched {
            Some(Ok(result)) => {
                self.model_fetch = None;
                events.push(self.model_picker(result));
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.model_fetch = None;
                events.push(self.model_picker(Err(
                    termide_i18n::t().agent_model_request_dropped().to_string(),
                )));
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
        // The silent context-window probe: adopt the active model's real
        // window when it arrives, and stay quiet on failure.
        match self.context_probe.as_ref().map(Receiver::try_recv) {
            Some(Ok(Ok(models))) => {
                self.context_probe = None;
                changed |= self.adopt_listed_models(&models);
            }
            Some(Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected)) => {
                self.context_probe = None;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
        // An external agent's model becomes known once its handshake finishes
        // (its adapter starts asynchronously): adopt the current model for the
        // banner and the Model chip, and note whether it offers a choice.
        if self.external {
            let models_known = !self.runtime.available_models().is_empty();
            if !self.acp_has_models && models_known {
                self.acp_has_models = true;
                changed = true;
            }
            // The agent's settings may change on their own (a model it falls
            // back to, the efforts a new model offers).
            let options = self.runtime.config_options();
            if options != self.acp_options {
                self.acp_options = options;
                changed = true;
            }
            // Ask once for the session's settings, now the agent has stated
            // its own.
            if !self.pending_acp_choices.is_empty()
                && (models_known || !self.acp_options.is_empty())
            {
                self.apply_acp_choices();
                changed = true;
            }
            if let Some(id) = self.runtime.current_model() {
                if id != self.model.id {
                    self.model.id = id;
                    changed = true;
                }
            }
            // The context's fill and size, as the agent reports them.
            if let Some((used, size)) = self.runtime.context_usage() {
                if (used, size) != (self.context_tokens, self.model.context_window) {
                    self.context_tokens = used;
                    self.model.context_window = size;
                    changed = true;
                }
            }
        }
        // A loop whose wait has elapsed starts its next iteration once the
        // panel is free (no run in flight, no card waiting for an answer).
        let due = self
            .loop_task
            .as_ref()
            .and_then(|t| t.next_at)
            .is_some_and(|at| at <= Instant::now());
        if due && !self.is_busy() && self.pending.is_none() {
            events.extend(self.loop_step());
            changed = true;
        }
        // A failed request whose wait is over is tried again.
        changed |= self.poll_retry_wait();
        // A goal whose work turn has finished runs the judge once the panel is
        // free and no judge call is already in flight.
        let judge_due = self
            .goal_task
            .as_ref()
            .is_some_and(|t| !t.judging && t.judge_at.is_some_and(|at| at <= Instant::now()));
        if judge_due && !self.is_busy() && self.pending.is_none() {
            events.extend(self.run_goal_judge());
            changed = true;
        }
        // While the agent works, keep the ticking timer and the block's
        // spinner moving without waiting for an event (throttled to ~10 fps).
        // A `/compact` between runs leaves the runtime idle but still works;
        // an external agent still starting spins in place of its model.
        let starting = self.external && self.runtime.is_starting();
        if (self.is_busy() || self.activity.is_some() || starting)
            && self.last_anim.elapsed() >= Duration::from_millis(100)
        {
            self.last_anim = Instant::now();
            changed = true;
        }
        changed |= self.settle_restored();
        // A goal or loop started, stepped or stopped is written to the log,
        // so a session reopened later offers to carry it on — between runs,
        // so nothing comes between a request's start and its message.
        if !self.is_busy() {
            self.sync_autorun();
        }
        if changed || !events.is_empty() {
            events.push(PanelEvent::NeedsRedraw);
        }
        events
    }
}

/// The file a successful `edit` or `write` changed, from the result details,
/// so open editors can follow it without waiting for the watcher.
pub(crate) fn changed_file(result: &ToolResultMessage) -> Option<PathBuf> {
    if result.is_error || !matches!(result.tool_name.as_str(), "edit" | "write") {
        return None;
    }
    result
        .details
        .as_ref()?
        .get("path")?
        .as_str()
        .map(PathBuf::from)
}

impl AgentPanel {
    /// Add to the session's totals what the `auto` mode reviewers spent
    /// since the last look — the panel's own, its external agent's and the
    /// one of the tools it serves; a subagent's counts in its task instead.
    /// The context fill stays: a review is a call of its own. `true` when
    /// there was anything.
    pub(crate) fn take_reviewer_spent(&mut self) -> bool {
        let spent = self.reviewer.spent.take();
        if spent.total() == 0 {
            return false;
        }
        self.session_input += spent.uncached();
        self.session_cached += spent.cache_read;
        self.session_output += spent.output;
        true
    }
}
