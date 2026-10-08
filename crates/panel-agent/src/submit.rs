//! Submitting the prompt: the built-in slash commands, prompt templates and
//! skills, runs and their queue, `/loop`, `/goal`, pausing, and project
//! command scripts.

use std::sync::mpsc::{self, Receiver};
use std::sync::PoisonError;
use std::time::Duration;

use termide_agent_core::{
    AgentCommand, CancelToken, CommandScript, Decision, DefinitionProblem, Message, PromptError,
    Session, SkillInfo, Timing, UserMessage,
};
use termide_core::{Panel, PanelEvent};
use termide_ui::ChoiceForm;

use crate::pending::Pending;
use crate::session_ops::discard;
use crate::{
    millis, now_hms, slash, transcript, AgentPanel, GoalTask, Item, LoopTask, NoticeKind,
    BUILTIN_COMMANDS, CLEAR_COMMAND, COMPACT_COMMAND, CONTINUE_COMMAND, FORK_COMMAND, GOAL_COMMAND,
    GOAL_MAX_ITERATIONS, HANDOFF_COMMAND, LOOP_COMMAND, LOOP_MAX_ITERATIONS, MCP_COMMAND,
    NAME_COMMAND, NEW_COMMAND, PAUSE_COMMAND, PROMPT_COMMAND, RENAME_ACTION, RENAME_COMMAND,
    SHOW_PROMPT_ACTION, UNDO_COMMAND, USAGE_COMMAND,
};

/// The work turn a `/goal` sends when the judge says the goal is not yet
/// reached: the goal restated, plus the one thing the judge found still
/// missing, so the agent keeps working from where it fell short.
pub(crate) fn goal_continuation(goal: &str, reason: &str) -> String {
    let reason = reason.trim();
    if reason.is_empty() {
        format!("The goal is not reached yet. Keep working toward it.\nGoal: {goal}")
    } else {
        format!(
            "The goal is not reached yet. Keep working toward it.\nGoal: {goal}\nStill missing: {reason}"
        )
    }
}

/// Split `/loop` arguments into an optional interval and the prompt. When the
/// first word is a duration (`30s`, `5m`, `2h`, or a bare count of seconds) it
/// is the interval and the rest is the prompt; otherwise the whole thing is the
/// prompt (a self-paced loop).
pub(crate) fn parse_loop_args(args: &str) -> (Option<Duration>, &str) {
    match args.split_once(char::is_whitespace) {
        // A duration as the first word is the interval; the rest is the prompt.
        Some((first, rest)) if parse_duration(first).is_some() => {
            (parse_duration(first), rest.trim())
        }
        // No interval (a lone word, or the first word is not a duration): the
        // whole thing is the prompt, a self-paced loop.
        _ => (None, args),
    }
}

/// Parse a duration like `30s`, `5m`, `2h`, or a bare count of seconds.
pub(crate) fn parse_duration(token: &str) -> Option<Duration> {
    let (digits, unit) = match token.chars().last() {
        Some('s') => (&token[..token.len() - 1], 1),
        Some('m') => (&token[..token.len() - 1], 60),
        Some('h') => (&token[..token.len() - 1], 3600),
        _ => (token, 1),
    };
    let n: u64 = digits.parse().ok()?;
    (n > 0).then(|| Duration::from_secs(n * unit))
}

/// A short human duration for the loop notice: `45s`, `5m`, `1m30s`.
pub(crate) fn fmt_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

/// `/<name> args` at the start of a message: the command name and the
/// rest. `skill:` may prefix the name (see `slash`). A word with further
/// slashes (`/usr/bin`) is text, not a command.
pub(crate) fn slash_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let bare = name.strip_prefix(slash::SKILL_PREFIX).unwrap_or(name);
    if bare.is_empty()
        || !bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some((name, args.trim()))
}

impl AgentPanel {
    /// Send the input box: a new run when idle, a steering message while
    /// the agent works.
    pub fn submit(&mut self) -> Vec<PanelEvent> {
        // Held pastes are spliced back in before anything reads the message, so
        // the full content is what a command, a template or the model sees.
        let text = self.expand_pastes(&self.input_area().text());
        let text = text.trim().to_string();
        if text.is_empty() {
            return vec![];
        }
        self.completion = None;
        self.history_pos = None;
        self.draft.clear();
        // In shell mode the input is a command, run in the shell here and now
        // and never sent to the model as a request: the user typed it, so it
        // needs no permission card, and what it printed joins the context as
        // theirs. Only the mode makes a command — text that merely opens with
        // `$` is a message — and it is checked before the slash arms, so a
        // path like `/usr/bin/env` is never read as a command name.
        // The mode is for this one command: the next is switched on again.
        if self.shell_mode {
            self.shell_mode = false;
            self.clear_input();
            return self.run_confirmed_command(text);
        }
        // What reaches `send` through a slash arm was expanded from this.
        let command = slash_command(&text).map(|_| text.clone());
        // An external agent's own `/compact` replaces termide's, which it
        // cannot run.
        let agent_compacts = self.external && self.agent_command(COMPACT_COMMAND).is_some();
        let text = match slash_command(&text) {
            Some((UNDO_COMMAND, _)) => {
                self.clear_input();
                return self.ask_undo();
            }
            Some((COMPACT_COMMAND, _)) if agent_compacts => text.clone(),
            Some((COMPACT_COMMAND, focus)) => {
                // Built in: summarise the older part of the session now.
                let focus = (!focus.is_empty()).then(|| focus.to_string());
                match self.runtime.compact(focus) {
                    Ok(()) => self.clear_input(),
                    Err(PromptError::Busy) => {
                        self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn)
                    }
                    Err(error) => self.notice(error.to_string(), NoticeKind::Warn),
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((NEW_COMMAND, _)) => {
                // Start fresh, leaving the current session in the list (empty
                // ones are still dropped by `switch_session`).
                self.clear_input();
                self.switch_session(None);
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((FORK_COMMAND, _)) => {
                self.clear_input();
                return self.ask_fork_session();
            }
            Some((CLEAR_COMMAND, _)) => {
                // Like `/new`, but the current session is deleted rather than
                // kept, so there is nothing to resume back to.
                if self.is_busy() {
                    self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
                    return vec![PanelEvent::NeedsRedraw];
                }
                self.clear_input();
                if let Some(old) = self.session.take() {
                    discard(old);
                }
                self.switch_session(None);
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((RENAME_COMMAND | NAME_COMMAND, args)) => {
                // With a name, rename now; without one, open the same prompt as
                // the `[≡]` menu and F2.
                self.clear_input();
                if args.is_empty() {
                    return self.handle_status_action(RENAME_ACTION);
                }
                if !self.rename_session(args) {
                    self.notice(
                        termide_i18n::t().agent_notice_no_log_to_name(),
                        NoticeKind::Warn,
                    );
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((PAUSE_COMMAND, _)) => {
                self.clear_input();
                if !self.runtime.can_pause() {
                    self.notice(PromptError::Unsupported.to_string(), NoticeKind::Info);
                } else if !self.request_pause() {
                    self.notice(
                        termide_i18n::t().agent_notice_nothing_to_pause(),
                        NoticeKind::Info,
                    );
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((CONTINUE_COMMAND, _)) => {
                self.clear_input();
                if self.paused {
                    self.resume();
                } else if self.pause_requested {
                    // The run has not reached the pause yet: withdraw it.
                    self.cancel_pause();
                } else if self.is_busy() {
                    self.notice(
                        termide_i18n::t().agent_notice_already_running(),
                        NoticeKind::Info,
                    );
                } else if self.retry_wait.is_some() || self.retry_ready {
                    // A failed request: tried again now.
                    self.retry_now();
                } else {
                    self.notice(
                        termide_i18n::t().agent_notice_nothing_to_continue(),
                        NoticeKind::Info,
                    );
                }
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((LOOP_COMMAND, args)) => {
                self.clear_input();
                let args = args.trim();
                if args.is_empty() || args == "stop" || args == "off" {
                    if self.loop_task.take().is_some() {
                        self.notice(
                            termide_i18n::t().agent_notice_loop_stopped(),
                            NoticeKind::Info,
                        );
                    } else {
                        self.notice(
                            termide_i18n::t().agent_notice_loop_usage(),
                            NoticeKind::Info,
                        );
                    }
                    return vec![PanelEvent::NeedsRedraw];
                }
                let (interval, prompt) = parse_loop_args(args);
                if prompt.is_empty() {
                    self.notice(
                        termide_i18n::t().agent_notice_loop_usage(),
                        NoticeKind::Info,
                    );
                    return vec![PanelEvent::NeedsRedraw];
                }
                let t = termide_i18n::t();
                self.notice(
                    match interval {
                        Some(d) => t.agent_notice_looping_every_fmt(&fmt_secs(d.as_secs())),
                        None => t.agent_notice_looping().to_string(),
                    },
                    NoticeKind::Info,
                );
                self.loop_task = Some(LoopTask {
                    prompt: prompt.to_string(),
                    interval,
                    next_at: None,
                    iterations: 0,
                });
                return self.loop_step();
            }
            Some((GOAL_COMMAND, args)) => {
                self.clear_input();
                let args = args.trim();
                if args.is_empty() || args == "stop" || args == "off" {
                    if self.goal_task.take().is_some() {
                        self.notice(
                            termide_i18n::t().agent_notice_goal_stopped(),
                            NoticeKind::Info,
                        );
                    } else {
                        self.notice(
                            termide_i18n::t().agent_notice_goal_usage(),
                            NoticeKind::Info,
                        );
                    }
                    return vec![PanelEvent::NeedsRedraw];
                }
                // While the agent works, the goal joins the run in flight as a
                // steering message, like `/loop`; the judge takes over once the
                // run ends.
                return self.start_goal(args.to_string(), command);
            }
            Some((HANDOFF_COMMAND, _)) => {
                self.clear_input();
                return self.start_handoff();
            }
            Some((USAGE_COMMAND, _)) => {
                self.clear_input();
                return self.session_summary();
            }
            Some((PROMPT_COMMAND, _)) => {
                self.clear_input();
                return self.handle_status_action(SHOW_PROMPT_ACTION);
            }
            Some((MCP_COMMAND, args)) => {
                self.clear_input();
                self.mcp_command(args);
                return vec![PanelEvent::NeedsRedraw];
            }
            Some((name, args)) => match slash::resolve(
                name,
                self.catalog.prompts(),
                self.catalog.commands(),
                self.catalog.skills(),
            ) {
                Some(slash::SlashTarget::Template(template)) => template.expand(args),
                Some(slash::SlashTarget::Script(script)) => {
                    // A command script: its output becomes the request, once
                    // it has run (and, for a project's script, been allowed).
                    self.clear_input();
                    self.run_command(script, args.to_string());
                    return vec![PanelEvent::NeedsRedraw];
                }
                // A skill switched off in the toolset still runs by hand: that
                // keeps it out of the model's context, not away from the user.
                Some(slash::SlashTarget::Skill(skill)) => match skill.load(args) {
                    Ok(loaded) => loaded.text,
                    Err(error) => {
                        self.notice(error, NoticeKind::Warn);
                        return vec![PanelEvent::NeedsRedraw];
                    }
                },
                // The agent's own command goes to it as typed.
                None if self.agent_command(name).is_some() => text.clone(),
                None => {
                    let mut names: Vec<String> =
                        self.catalog.prompts().into_iter().map(|p| p.name).collect();
                    names.extend(self.agent_commands().into_iter().map(|c| c.name));
                    names.extend(self.catalog.commands().into_iter().map(|c| c.name));
                    names.extend(self.slash_skills().into_iter().map(|(name, _)| name));
                    names.push(COMPACT_COMMAND.to_string());
                    if self.session_dir.is_some() {
                        names.push(NEW_COMMAND.to_string());
                        names.push(FORK_COMMAND.to_string());
                        names.push(CLEAR_COMMAND.to_string());
                        names.push(RENAME_COMMAND.to_string());
                        names.push(NAME_COMMAND.to_string());
                    }
                    if self.is_busy() {
                        names.push(PAUSE_COMMAND.to_string());
                    }
                    if self.paused || self.pause_requested {
                        names.push(CONTINUE_COMMAND.to_string());
                    }
                    names.push(LOOP_COMMAND.to_string());
                    names.push(GOAL_COMMAND.to_string());
                    names.push(HANDOFF_COMMAND.to_string());
                    names.push(USAGE_COMMAND.to_string());
                    names.push(PROMPT_COMMAND.to_string());
                    names.push(MCP_COMMAND.to_string());
                    self.notice(
                        termide_i18n::t().agent_notice_no_command_fmt(name, &names.join(", ")),
                        NoticeKind::Warn,
                    );
                    return vec![PanelEvent::NeedsRedraw];
                }
            },
            None => text,
        };
        // A model left to the provider is known once its list arrives; until
        // then there is nothing to send to, and the text stays to send later.
        if !self.external && self.model.id.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_model_pending(),
                NoticeKind::Warn,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        self.clear_input();
        self.send_as(text, command)
    }

    /// The commands the external agent offers of its own.
    pub(crate) fn agent_commands(&self) -> Vec<AgentCommand> {
        if self.external {
            self.runtime.agent_commands()
        } else {
            Vec::new()
        }
    }

    /// The external agent's own command `name`, when it offers one.
    pub(crate) fn agent_command(&self, name: &str) -> Option<AgentCommand> {
        self.agent_commands()
            .into_iter()
            .find(|command| command.name == name)
    }

    /// Send `text` as the next request: a new run when idle, a steering
    /// message while the agent works.
    pub(crate) fn send(&mut self, text: String) -> Vec<PanelEvent> {
        self.send_as(text, None)
    }

    /// [`Self::send`] for text a typed `/name args` produced: the transcript
    /// and the input history show the command, the model gets the text.
    pub(crate) fn send_as(&mut self, text: String, command: Option<String>) -> Vec<PanelEvent> {
        self.follow = true;
        let message = UserMessage::text(text).with_command(command);
        if self.is_busy() {
            // The message waits in the state strip until the agent takes it,
            // then shows as a user block.
            self.queued_texts.push_back(message.typed());
            self.runtime.steer(message);
            self.set_queued(self.runtime.queue_lens());
        } else {
            // A new request takes over from a failed one, and from a wait to
            // try it again.
            if self.retry_wait.take().is_some() {
                self.notice(
                    termide_i18n::t().agent_notice_retry_cancelled(),
                    NoticeKind::Info,
                );
            }
            self.retry_attempts = 0;
            self.retry_ready = false;
            self.restarted = false;
            // A fresh turn starts: surface the system prompt as a folded `#`
            // block when it is new or has changed since it was last shown, so it
            // sits just above this message.
            let system = self.effective_system_prompt();
            if system != self.shown_system {
                self.transcript.push(Item::System {
                    text: system.clone(),
                });
                self.shown_system = system;
            }
            // A request starts: from here on the files it touches are kept
            // for /undo, together with where the conversation stood.
            if let Some(store) = &self.checkpoints {
                let leaf = self
                    .session
                    .as_ref()
                    .and_then(Session::undo_point)
                    .map(str::to_string);
                store
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .begin_run(leaf);
            }
            match self.runtime.prompt(message) {
                Ok(()) => self.busy = true,
                Err(error) => self.notice(
                    termide_i18n::t().agent_notice_cannot_start_fmt(&error.to_string()),
                    NoticeKind::Error,
                ),
            }
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Run a command the user typed in shell mode, on a thread, so the UI stays
    /// alive while it runs. A command confirmed on a `suggest_command` card
    /// goes through the same runner, but on the agent thread inside the call
    /// that asked, so its output comes back as that call's result rather than
    /// through here. `Err`s from the runner (no shell, a spawn failure) and a
    /// non-zero exit both land in `poll_shell_job`; only the first is red.
    pub(crate) fn run_confirmed_command(&mut self, command: String) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let Some(runner) = self.shell_run.clone() else {
            self.notice(t.agent_notice_bang_unavailable(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        };
        if self.shell_job.is_some() {
            self.notice(t.agent_notice_bang_running(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let (tx, rx) = mpsc::channel();
        let cancel = CancelToken::new();
        self.shell_cancel = Some(cancel.clone());
        let status = t.agent_notice_bang_running_cmd_fmt(&command);
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let outcome = runner.run(&command, &cancel);
            let _ = tx.send((command, outcome, millis(start.elapsed())));
        });
        self.shell_job = Some(rx);
        vec![
            PanelEvent::SetStatusMessage {
                message: status,
                is_error: false,
            },
            PanelEvent::NeedsRedraw,
        ]
    }

    /// Take in a finished hand-run command: its output goes under the command
    /// in the transcript and into the model's context as the user's own.
    pub(crate) fn poll_shell_job(&mut self) -> bool {
        let taken = self.shell_job.as_ref().map(Receiver::try_recv);
        let (command, outcome, duration_ms) = match taken {
            Some(Ok(done)) => done,
            Some(Err(mpsc::TryRecvError::Empty)) | None => return false,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.shell_job = None;
                self.shell_cancel = None;
                self.notice(
                    termide_i18n::t().agent_notice_bang_dropped(),
                    NoticeKind::Error,
                );
                return true;
            }
        };
        self.shell_job = None;
        self.shell_cancel = None;
        let (output, failed) = match outcome {
            Ok(out) => (out.text, out.failed),
            Err(error) => (error, true),
        };
        let output = if output.trim().is_empty() {
            termide_i18n::t().command_report_no_output().to_string()
        } else {
            output
        };
        // The model reads it as a message the user ran: the command is their
        // intent, and the output travels with it as content, not as a request.
        let message = UserMessage::text(output)
            .with_ran(command)
            .with_ran_failed(failed);
        // Between runs the message joins the transcript through `update`;
        // during one it steers, arriving at the next boundary — dropping it
        // would leave the agent working without what the user just ran. A
        // steered message comes back as `MessageEnd` and is logged then, so
        // only this path writes it here; the log is what a session reopened
        // later reads, and without it the command and its output would be
        // gone from the next start.
        let logged = Message::User(message.clone());
        let delivered = if self.is_busy() {
            // It waits in the state strip, as a steered message does, and
            // shows as its block once the agent takes it.
            self.queued_texts.push_back(message.typed());
            self.runtime.steer(message);
            self.set_queued(self.runtime.queue_lens());
            Ok(())
        } else {
            if let Some(item) =
                transcript::user_command_item(&message, now_hms(), Some(duration_ms))
            {
                self.transcript.push(item);
            }
            if let Some(session) = &mut self.session {
                let timing = Timing::Tool {
                    duration_ms,
                    waited_ms: None,
                };
                if let Err(error) = session.append_timed_message(&logged, Some(timing)) {
                    log::warn!("agent session write failed: {error}");
                }
            }
            self.runtime.update(Box::new(move |agent| {
                agent.append_context(logged);
            }))
        };
        if let Err(error) = delivered {
            log::debug!("cannot add a hand-run command to the context: {error}");
        }
        self.pending_events.push(PanelEvent::SetStatusMessage {
            message: if failed {
                termide_i18n::t().agent_notice_bang_failed()
            } else {
                termide_i18n::t().agent_notice_bang_done()
            }
            .to_string(),
            is_error: failed,
        });
        true
    }

    /// Stop the hand-run command in flight, if there is one. Returns whether
    /// there was one, so Esc does not also abort a run.
    pub(crate) fn cancel_user_command(&mut self) -> bool {
        let Some(cancel) = self.shell_cancel.take() else {
            return false;
        };
        cancel.cancel();
        self.notice(
            termide_i18n::t().agent_notice_bang_stopped(),
            NoticeKind::Info,
        );
        true
    }

    /// Run the next `/loop` iteration: send the loop's prompt as a fresh run,
    /// unless the safety cap has been reached.
    pub(crate) fn loop_step(&mut self) -> Vec<PanelEvent> {
        if self
            .loop_task
            .as_ref()
            .is_some_and(|t| t.iterations >= LOOP_MAX_ITERATIONS)
        {
            self.loop_task = None;
            self.notice(
                termide_i18n::t().agent_notice_loop_stopped_max_fmt(LOOP_MAX_ITERATIONS),
                NoticeKind::Warn,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let Some(task) = self.loop_task.as_mut() else {
            return vec![PanelEvent::NeedsRedraw];
        };
        task.iterations += 1;
        task.next_at = None;
        let prompt = task.prompt.clone();
        self.send(prompt)
    }

    /// Start a `/goal`: work autonomously toward `goal`, a judge deciding after
    /// each turn whether it is reached. The first work turn is the goal itself,
    /// shown as `command` (the `/goal …` typed) so the text is not repeated.
    pub(crate) fn start_goal(&mut self, goal: String, command: Option<String>) -> Vec<PanelEvent> {
        self.notice(
            termide_i18n::t().agent_notice_goal_working(),
            NoticeKind::Info,
        );
        self.goal_task = Some(GoalTask {
            goal: goal.clone(),
            iterations: 0,
            judge_at: None,
            judging: false,
        });
        self.send_goal_turn_as(goal, command)
    }

    /// Send one work turn of the active goal as a fresh run and count it;
    /// stops the goal when the safety cap is reached.
    pub(crate) fn send_goal_turn(&mut self, prompt: String) -> Vec<PanelEvent> {
        self.send_goal_turn_as(prompt, None)
    }

    /// [`Self::send_goal_turn`], shown in the transcript as `command`.
    fn send_goal_turn_as(&mut self, prompt: String, command: Option<String>) -> Vec<PanelEvent> {
        let over_cap = match self.goal_task.as_mut() {
            Some(task) => {
                task.iterations += 1;
                task.judge_at = None;
                task.iterations > GOAL_MAX_ITERATIONS
            }
            None => return vec![PanelEvent::NeedsRedraw],
        };
        if over_cap {
            self.goal_task = None;
            self.notice(
                termide_i18n::t().agent_notice_goal_stopped_max_fmt(GOAL_MAX_ITERATIONS),
                NoticeKind::Warn,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        self.goal_errored = false;
        self.send_as(prompt, command)
    }

    /// Ask the judge whether the active goal is reached; the verdict arrives as
    /// a `GoalJudged` event, applied in [`AgentPanel::on_goal_verdict`].
    pub(crate) fn run_goal_judge(&mut self) -> Vec<PanelEvent> {
        let goal = match self.goal_task.as_mut() {
            Some(task) => {
                task.judge_at = None;
                task.judging = true;
                task.goal.clone()
            }
            None => return vec![PanelEvent::NeedsRedraw],
        };
        match self.runtime.judge(goal) {
            Ok(()) => self.notice(
                termide_i18n::t().agent_notice_goal_checking(),
                NoticeKind::Info,
            ),
            Err(error) => {
                self.goal_task = None;
                self.notice(
                    termide_i18n::t().agent_notice_cannot_check_goal_fmt(&error.to_string()),
                    NoticeKind::Warn,
                );
            }
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Apply the judge's verdict: finish when the goal is reached, otherwise
    /// send the next work turn with what is still missing.
    pub(crate) fn on_goal_verdict(&mut self, done: bool, reason: &str) {
        let goal = match self.goal_task.as_mut() {
            Some(task) => {
                task.judging = false;
                task.goal.clone()
            }
            None => return,
        };
        if done {
            self.goal_task = None;
            let reason = reason.trim();
            let t = termide_i18n::t();
            let msg = if reason.is_empty() {
                t.agent_notice_goal_reached().to_string()
            } else {
                t.agent_notice_goal_reached_reason_fmt(reason)
            };
            self.notice(msg, NoticeKind::Info);
            return;
        }
        let prompt = goal_continuation(&goal, reason);
        let _ = self.send_goal_turn(prompt);
    }

    pub fn abort(&mut self) {
        // Stopping also ends any running loop or goal.
        self.loop_task = None;
        self.goal_task = None;
        // A stop already under way needs no second request or notice.
        if self.is_busy() && !self.stop_requested {
            self.runtime.abort();
            self.stop_requested = true;
            self.notice(termide_i18n::t().agent_notice_stopping(), NoticeKind::Warn);
        }
    }

    /// Record the runtime's queue lengths and drop the steering texts the
    /// agent has taken (it takes them oldest first).
    pub(crate) fn set_queued(&mut self, queued: (usize, usize)) {
        self.queued = queued;
        while self.queued_texts.len() > queued.0 {
            self.queued_texts.pop_front();
        }
    }

    /// Ask the running agent to pause at its next step boundary. Returns
    /// whether a run was there to pause.
    pub(crate) fn request_pause(&mut self) -> bool {
        if !self.is_busy() || !self.runtime.can_pause() {
            return false;
        }
        // The state strip shows the pending pause until the run reaches a
        // step boundary.
        self.runtime.pause();
        self.pause_requested = true;
        true
    }

    /// Withdraw a pause asked for that the run has not reached yet.
    pub(crate) fn cancel_pause(&mut self) {
        self.runtime.cancel_pause();
        self.pause_requested = false;
    }

    /// Resume the paused run. Its clock goes on from the request, and the
    /// pause's line keeps how long the pause lasted.
    pub(crate) fn resume(&mut self) {
        match self.runtime.resume() {
            Ok(()) => {
                self.busy = true;
                self.paused = false;
                self.resuming = true;
                self.end_pause();
            }
            Err(error) => self.notice(
                termide_i18n::t().agent_notice_cannot_continue_fmt(&error.to_string()),
                NoticeKind::Warn,
            ),
        }
    }

    /// Give up a paused run: nothing is running, so there is nothing to
    /// abort; the pause ends where it stood, and so do a loop or goal it was
    /// part of. The calls it left unrun are closed by the next request.
    pub(crate) fn stop_paused(&mut self) {
        self.paused = false;
        self.run_start = None;
        self.loop_task = None;
        self.goal_task = None;
        self.end_pause();
    }

    /// Freeze the pause's line at the pause's length, once it is over.
    pub(crate) fn end_pause(&mut self) {
        if let Some(start) = self.pause_start.take() {
            self.transcript.finish_pause(millis(start.elapsed()));
        }
    }

    /// Each skill as `/` reaches it: by its own name, or as `skill:<name>`
    /// when a built-in command, a template or a script takes the name.
    pub(crate) fn slash_skills(&self) -> Vec<(String, SkillInfo)> {
        let prompts = self.catalog.prompts();
        let commands = self.catalog.commands();
        self.catalog
            .skills()
            .into_iter()
            .map(|skill| {
                let name = skill.name.as_str();
                let taken = BUILTIN_COMMANDS.contains(&name)
                    || prompts.iter().any(|p| p.name == name)
                    || commands.iter().any(|c| c.name == name);
                let reach = if taken {
                    format!("{}{name}", slash::SKILL_PREFIX)
                } else {
                    name.to_string()
                };
                (reach, skill)
            })
            .collect()
    }

    /// The `/name`s more than one kind defines, see [`slash::conflicts`].
    pub(crate) fn slash_conflicts(&self) -> Vec<slash::Conflict> {
        slash::conflicts(
            &BUILTIN_COMMANDS,
            &self.catalog.prompts(),
            &self.catalog.commands(),
            &self.catalog.skills(),
        )
    }

    /// One warning per `/name` more than one kind defines.
    pub(crate) fn notice_slash_conflicts(&mut self) {
        for conflict in self.slash_conflicts() {
            self.notice(conflict.describe(), NoticeKind::Warn);
        }
    }

    /// One warning per front-matter key or tool text nothing reads.
    pub(crate) fn notice_definition_problems(&mut self) {
        let t = termide_i18n::t();
        for problem in self.catalog.definition_problems() {
            let text = match problem {
                DefinitionProblem::UnknownKey { file, key } => {
                    t.agent_notice_unknown_key_fmt(&file.display().to_string(), &key)
                }
                DefinitionProblem::UnknownTool { file } => {
                    t.agent_notice_unknown_tool_text_fmt(&file.display().to_string())
                }
                DefinitionProblem::EmptyToolText { file } => {
                    t.agent_notice_empty_tool_text_fmt(&file.display().to_string())
                }
            };
            self.notice(text, NoticeKind::Warn);
        }
    }

    /// `/name args` names a command script: run it, or ask first when it
    /// came with the project and no rule or session grant covers it.
    pub(crate) fn run_command(&mut self, script: CommandScript, args: String) {
        let verdict = self.rules.evaluate("command", &script.name);
        if verdict == Some(Decision::Deny) {
            self.notice(
                termide_i18n::t().agent_notice_command_denied_fmt(&script.name),
                NoticeKind::Warn,
            );
            return;
        }
        let allowed = script.trusted
            || verdict == Some(Decision::Allow)
            || self.allowed_commands.contains(&script.name);
        if allowed {
            self.start_command(script, args);
            return;
        }
        let t = termide_i18n::t();
        let title = t.agent_command_run_title_fmt(&script.name, &script.path.display().to_string());
        let form = ChoiceForm::new(
            title.clone(),
            vec![
                t.agent_cmd_run_once().to_string(),
                t.agent_cmd_run_session().to_string(),
                t.agent_cmd_run_always().to_string(),
                t.agent_cmd_dont_run().to_string(),
            ],
        );
        self.pending_events.push(PanelEvent::SetStatusMessage {
            message: title,
            is_error: false,
        });
        self.pending = Some(Pending::Command { script, args, form });
    }

    /// Run the script on a thread; `tick` sends its output as the request.
    pub(crate) fn start_command(&mut self, script: CommandScript, args: String) {
        if self.command_run.is_some() {
            self.notice(
                termide_i18n::t().agent_notice_command_running(),
                NoticeKind::Warn,
            );
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cwd = self.cwd.clone();
        let name = script.name.clone();
        // The request is headed by the command as typed.
        let command = if args.is_empty() {
            format!("/{name}")
        } else {
            format!("/{name} {args}")
        };
        std::thread::spawn(move || {
            let outcome = script.run(&args, &cwd);
            let _ = tx.send((command, outcome));
        });
        self.command_run = Some(rx);
        self.pending_events.push(PanelEvent::SetStatusMessage {
            message: termide_i18n::t().agent_running_command_fmt(&name),
            is_error: false,
        });
    }

    /// Take in a finished command script: its output goes out as a request.
    pub(crate) fn poll_command(&mut self) -> bool {
        let outcome = self.command_run.as_ref().map(Receiver::try_recv);
        match outcome {
            Some(Ok((command, Ok(text)))) => {
                self.command_run = None;
                self.send_as(text, Some(command));
                true
            }
            Some(Ok((_, Err(error)))) => {
                self.command_run = None;
                self.notice(error, NoticeKind::Error);
                true
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.command_run = None;
                self.notice(
                    termide_i18n::t().agent_notice_command_dropped(),
                    NoticeKind::Error,
                );
                true
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => false,
        }
    }
}
