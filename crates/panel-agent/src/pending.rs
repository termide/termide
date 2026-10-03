//! The card the panel shows while it waits for an answer: a permission
//! request, the model's questions, a project command, an undo, a rewind, a
//! plan or a handoff brief.

use std::time::Instant;

use termide_agent_core::{
    shell_parts, CommandScript, Decision, Mode, PermissionAnswer, PermissionEnvelope, PersistScope,
    QuestionAnswer, QuestionEnvelope, QuestionReply, SuggestionEnvelope, SuggestionReply,
};
use termide_core::PanelEvent;
use termide_ui::{ChoiceAction, ChoiceForm};

use crate::rewind::{RewindPoint, RewindScope};
use crate::{millis, AgentPanel, Item, NoticeKind};

/// What a card in the panel is asking: the agent's permission request, the
/// model's questions to the user, or whether a command script that came with
/// the project may run.
pub(crate) enum Pending {
    Permission {
        envelope: PermissionEnvelope,
        form: ChoiceForm,
        /// What each of the form's rows answers, in their order.
        answers: Vec<PermissionAnswer>,
    },
    /// The model's questions, asked one card at a time; `answers` holds those
    /// already given, and `form` asks the next.
    Question {
        envelope: QuestionEnvelope,
        answers: Vec<QuestionAnswer>,
        form: ChoiceForm,
    },
    Command {
        script: CommandScript,
        args: String,
        form: ChoiceForm,
    },
    Undo {
        form: ChoiceForm,
    },
    /// A rewind to before `point`'s message: what to put back.
    Rewind {
        form: ChoiceForm,
        point: RewindPoint,
    },
    /// Plan mode: the agent answered, carry the plan out or keep planning?
    Plan {
        form: ChoiceForm,
    },
    /// A `/handoff` brief is ready: save it to a file, or start a new session
    /// from it. The brief is kept until the choice is made.
    Handoff {
        form: ChoiceForm,
        brief: String,
    },
    /// A command the `suggest_command` tool offered. It runs only on `[Run]`,
    /// through the panel's own runner — never as a tool call the agent makes —
    /// and `[Run]` is withheld where plan mode or a `deny` rule covers the
    /// command, so the card cannot walk through the user's own rules.
    Suggestion {
        envelope: SuggestionEnvelope,
        form: ChoiceForm,
        /// What each row of the form answers, in their order.
        answers: Vec<SuggestionReply>,
    },
}

impl Pending {
    pub(crate) fn form(&self) -> &ChoiceForm {
        match self {
            Pending::Permission { form, .. }
            | Pending::Question { form, .. }
            | Pending::Command { form, .. }
            | Pending::Undo { form }
            | Pending::Rewind { form, .. }
            | Pending::Plan { form }
            | Pending::Handoff { form, .. }
            | Pending::Suggestion { form, .. } => form,
        }
    }

    pub(crate) fn form_mut(&mut self) -> &mut ChoiceForm {
        match self {
            Pending::Permission { form, .. }
            | Pending::Question { form, .. }
            | Pending::Command { form, .. }
            | Pending::Undo { form }
            | Pending::Rewind { form, .. }
            | Pending::Plan { form }
            | Pending::Handoff { form, .. }
            | Pending::Suggestion { form, .. } => form,
        }
    }
}

/// The card for question `index` of `envelope`: who asks and the topic in the
/// title, with the position among several; the question itself as the
/// detail; the choices with their descriptions, checkboxes when several can
/// be picked; a row for an answer of the user's own, and one that declines.
pub(crate) fn question_form(envelope: &QuestionEnvelope, index: usize) -> ChoiceForm {
    let t = termide_i18n::t();
    let question = &envelope.questions[index];
    let mut title = t.agent_question_title().to_string();
    if !question.header.is_empty() {
        title.push_str(&format!(": {}", question.header));
    }
    if envelope.questions.len() > 1 {
        title.push_str(&format!(" ({}/{})", index + 1, envelope.questions.len()));
    }
    let (labels, descriptions) = question
        .options
        .iter()
        .map(|option| (option.label.clone(), option.description.clone()))
        .unzip();
    let mut form = ChoiceForm::new(title, labels)
        .with_detail(&question.question)
        .with_descriptions(descriptions)
        .with_custom(t.agent_question_own_answer())
        .with_cancel(t.agent_question_decline());
    if question.multi_select {
        form = form.with_multi(t.agent_question_submit());
    }
    form
}

/// A permission card's detail: what the agent wants to do and, for a
/// command of several parts, the parts the question is about, one a line,
/// those no rule can be recorded for marked as answered this time only.
pub(crate) fn permission_detail(request: &termide_agent_core::PermissionRequest) -> String {
    let several =
        termide_agent_core::shell_parts(&request.subject, std::path::Path::new("/")).len() > 1;
    if request.tool != "bash" || !several || request.parts.is_empty() {
        return request.subject.clone();
    }
    let once = termide_i18n::t().agent_perm_part_once();
    let parts: Vec<String> = request
        .parts
        .iter()
        .map(|part| match part.pattern {
            Some(_) => format!("• {}", part.text),
            None => format!("• {} ({once})", part.text),
        })
        .collect();
    format!("{}\n\n{}", request.subject, parts.join("\n"))
}

/// The card for a command the `suggest_command` tool offered: the command in
/// full and unmodified, why it is offered and where it would run, marked as
/// the agent's suggestion so it cannot read as one the user wrote. `[Run]` is
/// the only row that runs it; where plan mode or a `deny` rule covers the
/// command, `[Run]` and `[Edit first]` are left off and only `[Copy]` and the
/// dismiss row remain. Returns the form with the answer of each option row.
pub(crate) fn suggestion_form(
    envelope: &SuggestionEnvelope,
    cwd: &std::path::Path,
) -> (ChoiceForm, Vec<SuggestionReply>) {
    let t = termide_i18n::t();
    let mut detail = envelope.suggestion.command.clone();
    if !envelope.suggestion.why.is_empty() {
        detail.push('\n');
        detail.push_str(&t.agent_suggest_why_fmt(&envelope.suggestion.why));
    }
    detail.push('\n');
    detail.push_str(&t.agent_suggest_cwd_fmt(&cwd.display().to_string()));
    detail.push('\n');
    detail.push_str(t.agent_suggest_by_agent());
    if let Some(reason) = envelope.denied.as_ref() {
        detail.push('\n');
        detail.push_str(reason);
    }
    let mut rows: Vec<(SuggestionReply, String)> = Vec::new();
    if envelope.denied.is_none() {
        rows.push((SuggestionReply::Run, t.agent_suggest_run().to_string()));
        rows.push((SuggestionReply::Edit, t.agent_suggest_edit().to_string()));
    }
    rows.push((SuggestionReply::Copied, t.agent_suggest_copy().to_string()));
    let (answers, options): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
    let form = ChoiceForm::new(t.agent_suggest_title(), options)
        .with_detail(detail)
        .with_cancel(t.agent_suggest_dismiss());
    (form, answers)
}

impl AgentPanel {
    pub(crate) fn poll_permissions(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();
        while let Ok(envelope) = self.permission_rx.try_recv() {
            if self.pending.is_some() {
                // Prompts are sequential on the agent thread; a second one
                // cannot arrive before the first is answered. Deny defensively.
                let _ = envelope.reply.send(PermissionAnswer::Deny);
                continue;
            }
            // The question is asked in the panel, not in an app-wide modal:
            // with several panels open a modal does not say who is asking.
            // The status line still announces it for an unfocused panel.
            let request = &envelope.request;
            // The card shows the intent in its title and what exactly the agent
            // wants to do in the detail block below. MCP tools and others
            // without a path or command have no subject, so no detail.
            let has_subject = !request.subject.is_empty();
            let t = termide_i18n::t();
            let base = t.agent_permission_run_fmt(&request.tool);
            let title = if has_subject {
                format!("{base}:")
            } else {
                base.clone()
            };
            // The status line, for an unfocused panel, still names the subject.
            let status = if has_subject {
                format!("{base}: {}", request.subject)
            } else {
                base
            };
            // The answers the form offers, each with its row. The rows that
            // outlast this call name the rules they record, and show only
            // when there is one to record; "always" only where the
            // configured rules count, and neither "session" nor "always" for
            // a command too broad or too destructive to trust sight unseen.
            let pattern = &request.suggested_pattern;
            let remember = request.can_remember();
            let mut rows: Vec<(PermissionAnswer, String)> = vec![(
                PermissionAnswer::AllowOnce,
                t.agent_perm_allow_once().to_string(),
            )];
            if remember && request.can_allow_session {
                rows.push((
                    PermissionAnswer::AllowSession,
                    format!("{} ({pattern})", t.agent_perm_allow_session()),
                ));
            }
            if remember && request.can_persist {
                rows.push((
                    PermissionAnswer::AllowAlways,
                    format!("{} ({pattern})", t.agent_perm_allow_always()),
                ));
                rows.push((
                    PermissionAnswer::AllowAlwaysGlobal,
                    format!("{} ({pattern})", t.agent_perm_allow_always_global()),
                ));
            }
            rows.push((PermissionAnswer::Deny, t.agent_perm_deny().to_string()));
            if remember {
                rows.push((
                    PermissionAnswer::DenySession,
                    format!("{} ({pattern})", t.agent_perm_deny_session()),
                ));
            }
            let (answers, options): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
            let mut form = ChoiceForm::new(title, options)
                .with_custom(t.agent_perm_deny_reason())
                .with_cancel(t.agent_perm_stop());
            if has_subject {
                form = form.with_detail(permission_detail(request));
            }
            events.push(PanelEvent::SetStatusMessage {
                message: status,
                is_error: false,
            });
            self.pending = Some(Pending::Permission {
                envelope,
                form,
                answers,
            });
            self.raise_attention(true);
            // The question pauses the running call until it is answered.
            self.begin_permission_wait();
        }
        events
    }

    /// How long the running call has waited on permission answers so far.
    pub(crate) fn running_tool_wait(&self) -> u32 {
        self.transcript
            .items()
            .iter()
            .rev()
            .find_map(|item| match item {
                Item::Tool {
                    result: None,
                    waited_ms,
                    ..
                } => Some(waited_ms.unwrap_or(0)),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// Show the model's questions as they arrive, one card at a time. Like a
    /// permission prompt, they pause the running call until answered.
    pub(crate) fn poll_questions(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();
        while let Ok(envelope) = self.question_rx.try_recv() {
            if self.pending.is_some() || envelope.questions.is_empty() {
                // Calls run one at a time, so a second set cannot arrive
                // while one is shown; decline it defensively.
                let _ = envelope.reply.send(QuestionReply::Declined);
                continue;
            }
            let form = question_form(&envelope, 0);
            events.push(PanelEvent::SetStatusMessage {
                message: envelope.questions[0].question.clone(),
                is_error: false,
            });
            self.pending = Some(Pending::Question {
                envelope,
                answers: Vec::new(),
                form,
            });
            self.raise_attention(true);
            self.begin_permission_wait();
        }
        events
    }

    /// Record the answer to the question on the card, then ask the next one,
    /// or send them all back once the last is answered.
    pub(crate) fn answer_question(&mut self, answer: QuestionAnswer) {
        let Some(Pending::Question {
            envelope,
            mut answers,
            ..
        }) = self.pending.take()
        else {
            return;
        };
        answers.push(answer);
        if answers.len() < envelope.questions.len() {
            let form = question_form(&envelope, answers.len());
            self.pending = Some(Pending::Question {
                envelope,
                answers,
                form,
            });
            return;
        }
        self.end_permission_wait();
        let _ = envelope.reply.send(QuestionReply::Answered(answers));
    }

    /// A permission question is up: the running call's wait starts counting.
    fn begin_permission_wait(&mut self) {
        let before = self.running_tool_wait();
        self.permission_wait = Some((Instant::now(), before));
        self.transcript.set_tool_wait(before, true);
    }

    /// The permission question is gone (answered, or dropped by a stop):
    /// the call's wait keeps its length and rests.
    pub(crate) fn end_permission_wait(&mut self) {
        if let Some((start, before)) = self.permission_wait.take() {
            self.transcript
                .set_tool_wait(before.saturating_add(millis(start.elapsed())), false);
        }
    }

    /// Why the card must withhold `[Run]`: plan mode runs nothing that could
    /// change something, and a `deny` rule is the user's own boundary. Parts
    /// are checked one by one, as the permission layer judges a command, so a
    /// `deny` on any part of a pipeline holds the whole card.
    fn suggestion_denied(&self, command: &str) -> Option<String> {
        let t = termide_i18n::t();
        if self.mode.get() == Mode::Plan {
            return Some(t.agent_suggest_denied_plan().to_string());
        }
        let denied = shell_parts(command, &self.cwd).iter().any(|part| {
            self.rules.evaluate("bash", &part.text) == Some(Decision::Deny)
                || self.rules.evaluate("bash", &part.resolved) == Some(Decision::Deny)
        });
        denied.then(|| t.agent_suggest_denied_rule().to_string())
    }

    /// Show a command the `suggest_command` tool offered, one card at a time;
    /// like a permission prompt it pauses the running call until answered. A
    /// card that cannot arrive while another is up is declined defensively, so
    /// the tool learns at once that nothing ran.
    pub(crate) fn poll_suggestions(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();
        while let Ok(mut envelope) = self.suggestion_rx.try_recv() {
            if self.pending.is_some() {
                let _ = envelope.reply.send(SuggestionReply::Declined);
                continue;
            }
            envelope.denied = self.suggestion_denied(&envelope.suggestion.command);
            let (form, answers) = suggestion_form(&envelope, &self.cwd);
            events.push(PanelEvent::SetStatusMessage {
                message: termide_i18n::t().agent_suggest_title().to_string(),
                is_error: false,
            });
            self.pending = Some(Pending::Suggestion {
                envelope,
                form,
                answers,
            });
            self.raise_attention(true);
            // The call blocks on the card, so its wait shows as a pause, as a
            // permission question's does.
            self.begin_permission_wait();
        }
        events
    }

    /// Answer the offered command. `[Run]` only says so here: the tool runs it
    /// on the agent thread through the runner in its context — the same one
    /// `$` uses — so what reaches the shell is what was on the card, and its
    /// output comes back as the call's result. `[Edit first]` puts it in the
    /// input to change before the user runs it themselves.
    pub(crate) fn answer_suggestion(
        &mut self,
        reply: SuggestionReply,
        envelope: SuggestionEnvelope,
    ) {
        let command = envelope.suggestion.command.clone();
        // The card is answered, so the call's wait rests; what follows — the
        // command running, or nothing at all — is not a wait on the user.
        self.end_permission_wait();
        match reply {
            SuggestionReply::Run => {
                // The tool runs it; the panel does not run it a second time.
                let _ = envelope.reply.send(SuggestionReply::Run);
            }
            SuggestionReply::Edit => {
                let _ = envelope.reply.send(SuggestionReply::Edit);
                self.shell_mode = true;
                self.set_input(&command);
            }
            SuggestionReply::Copied => {
                self.copy_text(&command);
                let _ = envelope.reply.send(SuggestionReply::Copied);
            }
            SuggestionReply::Declined => {
                let _ = envelope.reply.send(SuggestionReply::Declined);
            }
        }
    }

    /// Answer the outstanding question; `false` when there is none.
    pub fn answer_permission(&mut self, answer: PermissionAnswer) -> bool {
        let Some(Pending::Permission { envelope, .. }) = self.pending.take() else {
            return false;
        };
        // Mirror a lasting grant into the panel's own rules so rebuilding the
        // agent (undo, a model or agent switch) carries it, not just the hooks
        // on the worker thread. "Always" is also written to the configuration
        // by the persist callback, where it is on offer; "for this session"
        // lives only here. Neither is recorded where the request does not
        // offer it, so an answer cannot smuggle a grant the card withheld.
        let request = &envelope.request;
        for pattern in request.patterns() {
            match answer {
                PermissionAnswer::AllowAlways | PermissionAnswer::AllowAlwaysGlobal
                    if request.can_persist =>
                {
                    self.rules.add(&request.tool, &pattern, Decision::Allow);
                }
                PermissionAnswer::AllowSession if !request.can_allow_session => {}
                PermissionAnswer::AllowAlways
                | PermissionAnswer::AllowAlwaysGlobal
                | PermissionAnswer::AllowSession => {
                    self.session_rules
                        .add(&request.tool, &pattern, Decision::Allow);
                }
                PermissionAnswer::DenySession => {
                    self.session_rules
                        .add(&request.tool, &pattern, Decision::Deny);
                }
                _ => {}
            }
        }
        self.end_permission_wait();
        let _ = envelope.reply.send(answer);
        true
    }

    /// In plan mode, once the agent has answered: offer to carry the plan
    /// out, in accept-edits or asking, or to keep planning.
    pub(crate) fn offer_plan(&mut self) {
        if self.external || self.mode.get() != Mode::Plan || self.pending.is_some() {
            return;
        }
        // The run's closing line sits after the answer; look past it.
        let last = self
            .transcript
            .items()
            .iter()
            .rev()
            .find(|item| !matches!(item, Item::RunEnd { .. }));
        let answered = matches!(
            last,
            Some(Item::Assistant { text, error: None, .. }) if !text.trim().is_empty()
        );
        if !answered {
            return;
        }
        let t = termide_i18n::t();
        let form = ChoiceForm::new(
            t.agent_plan_carry_title(),
            vec![
                t.agent_plan_accept_edits().to_string(),
                t.agent_plan_configured().to_string(),
            ],
        )
        .with_cancel(t.agent_plan_keep());
        self.pending = Some(Pending::Plan { form });
    }

    /// The plan was accepted: leave plan mode for `mode` and send the
    /// request that carries it out.
    pub(crate) fn carry_out_plan(&mut self, mode: Mode) -> Vec<PanelEvent> {
        let mut events = vec![self.set_mode(mode)];
        let request = self.plan_prompt.request.trim().to_string();
        if request.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_plan_no_request(),
                NoticeKind::Warn,
            );
        } else {
            events.extend(self.send(request));
        }
        events.push(PanelEvent::NeedsRedraw);
        events
    }

    /// Turn what the card reported into an answer. For a permission,
    /// `Cancelled` denies and stops the run: the user wants out, not just a
    /// "no" to this one call; for the model's question it declines and stops
    /// the run alike. For a command script, the rows are run once,
    /// run for the session, run always (a rule is written) and don't run.
    /// `false` for `NotHandled`.
    pub(crate) fn apply_form_action(&mut self, action: ChoiceAction) -> bool {
        match (&self.pending, action) {
            (_, ChoiceAction::Handled) => {}
            (_, ChoiceAction::NotHandled) => return false,
            (Some(Pending::Permission { answers, .. }), ChoiceAction::Chosen(index)) => {
                let answer = answers
                    .get(index)
                    .cloned()
                    .unwrap_or(PermissionAnswer::Deny);
                self.answer_permission(answer);
            }
            (Some(Pending::Permission { .. }), ChoiceAction::Custom(reason)) => {
                self.answer_permission(PermissionAnswer::DenyWithReason(reason));
            }
            (Some(Pending::Permission { .. }), ChoiceAction::Cancelled) => {
                self.answer_permission(PermissionAnswer::Deny);
                self.abort();
            }
            (Some(Pending::Question { form, .. }), ChoiceAction::Chosen(index)) => {
                let chosen = form.options().get(index).cloned().into_iter().collect();
                self.answer_question(QuestionAnswer {
                    chosen,
                    custom: None,
                });
            }
            (Some(Pending::Question { .. }), ChoiceAction::Custom(text)) => {
                self.answer_question(QuestionAnswer {
                    chosen: Vec::new(),
                    custom: Some(text),
                });
            }
            (Some(Pending::Question { form, .. }), ChoiceAction::Submitted { chosen, custom }) => {
                let options = form.options();
                let chosen = chosen
                    .iter()
                    .filter_map(|&index| options.get(index).cloned())
                    .collect();
                self.answer_question(QuestionAnswer { chosen, custom });
            }
            // Declining stops the run, as it does for a permission: the user
            // takes over and says what they want in their own message.
            (Some(Pending::Question { .. }), ChoiceAction::Cancelled) => {
                if let Some(Pending::Question { envelope, .. }) = self.pending.take() {
                    self.end_permission_wait();
                    let _ = envelope.reply.send(QuestionReply::Declined);
                }
                self.abort();
            }
            // Only a question's card lets several rows be picked.
            (_, ChoiceAction::Submitted { .. }) => {}
            (Some(Pending::Command { .. }), ChoiceAction::Chosen(index)) => {
                let Some(Pending::Command { script, args, .. }) = self.pending.take() else {
                    return true;
                };
                match index {
                    1 => {
                        self.allowed_commands.insert(script.name.clone());
                    }
                    2 => {
                        self.rules.add("command", &script.name, Decision::Allow);
                        if let Some(persist) = self.persist_rule {
                            persist(
                                "command",
                                &script.name,
                                Decision::Allow,
                                PersistScope::Project,
                            );
                        }
                    }
                    3 => return true,
                    _ => {}
                }
                self.start_command(script, args);
            }
            (Some(Pending::Command { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Undo { .. }), ChoiceAction::Chosen(_)) => {
                self.pending = None;
                let events = self.perform_undo();
                self.pending_events.extend(events);
            }
            (Some(Pending::Undo { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Rewind { .. }), ChoiceAction::Chosen(index)) => {
                let Some(Pending::Rewind { point, .. }) = self.pending.take() else {
                    return true;
                };
                let events = self.rewind(&point, RewindScope::of_choice(index));
                self.pending_events.extend(events);
            }
            (Some(Pending::Rewind { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Plan { .. }), ChoiceAction::Chosen(index)) => {
                self.pending = None;
                let mode = if index == 0 {
                    Mode::Edit
                } else {
                    Mode::Configured
                };
                let events = self.carry_out_plan(mode);
                self.pending_events.extend(events);
            }
            (Some(Pending::Plan { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Handoff { .. }), ChoiceAction::Chosen(index)) => {
                let brief = match self.pending.take() {
                    Some(Pending::Handoff { brief, .. }) => brief,
                    _ => return true,
                };
                if index == 0 {
                    self.save_handoff(&brief);
                } else {
                    let events = self.handoff_to_new_session(brief);
                    self.pending_events.extend(events);
                }
            }
            (Some(Pending::Handoff { .. }), ChoiceAction::Cancelled | ChoiceAction::Custom(_)) => {
                self.pending = None;
            }
            (Some(Pending::Suggestion { .. }), ChoiceAction::Chosen(index)) => {
                let Some(Pending::Suggestion {
                    envelope, answers, ..
                }) = self.pending.take()
                else {
                    return true;
                };
                let reply = answers
                    .into_iter()
                    .nth(index)
                    .unwrap_or(SuggestionReply::Declined);
                self.answer_suggestion(reply, envelope);
            }
            (
                Some(Pending::Suggestion { .. }),
                ChoiceAction::Cancelled | ChoiceAction::Custom(_),
            ) => {
                // Esc or the dismiss row declines: the command does not run,
                // and the tool is told.
                if let Some(Pending::Suggestion { envelope, .. }) = self.pending.take() {
                    self.end_permission_wait();
                    let _ = envelope.reply.send(SuggestionReply::Declined);
                }
            }
            (None, _) => {}
        }
        true
    }
}
