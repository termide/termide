//! Session operations: switching, renaming and resuming sessions, deleting
//! one, its summary, undo, and the `/handoff` brief.

use std::path::Path;
use std::sync::{Arc, PoisonError};

use crate::runtime::local_minute;
use termide_agent_core::{EntryKind, PromptError, Session, SessionSummary};
use termide_core::{ConfirmAction, InputAction, PanelEvent};
use termide_ui::ChoiceForm;

use crate::pending::Pending;
use crate::runtime::{
    checkpoint_store, session_agent, session_model, spawn_runtime, start_session, Spawned,
};
use crate::{
    format_tokens, shorten_path, truncate_title, AgentPanel, Item, NoticeKind,
    DELETE_RECENT_ACTION, DELETE_SESSION_ACTION, FORK_SESSION_ACTION, RENAME_RECENT_ACTION,
};

/// Delete a session the panel is leaving when it holds no conversation, so
/// empty sessions do not clutter the list or the disk. A session with any
/// message or a user-given name is kept.
pub(crate) fn discard_if_empty(session: Session) {
    if session.is_empty() {
        discard(session);
    }
}

/// Delete `session` from disk unconditionally (the `/clear` path). A failure is
/// logged rather than surfaced: the session is being abandoned regardless.
pub(crate) fn discard(session: Session) {
    if let Err(error) = session.discard() {
        log::warn!("could not remove agent session: {error}");
    }
}

impl AgentPanel {
    /// Replace the running agent with one continuing `session` (or a fresh
    /// one when `None`). Refuses while a run is in flight.
    pub fn switch_session(&mut self, session: Option<Session>) -> bool {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return false;
        }
        let session = session.or_else(|| {
            start_session(
                self.session_dir.as_deref(),
                &self.cwd,
                &self.connection,
                &self.provider_kind,
                &self.model,
                &self.agent,
            )
        });
        let model = session_model(&self.configured_model, session.as_ref());
        self.checkpoints = checkpoint_store(self.session_dir.as_deref(), session.as_ref());
        let (mut agent, mut system_prompt, mut tools) = (
            self.agent.clone(),
            self.system_prompt.clone(),
            self.tools.clone(),
        );
        let (toolset_off, resolved) = session_agent(
            self.catalog.as_ref(),
            &agent,
            &self.context_off,
            session.as_ref(),
        );
        if let Some((name, profile)) = resolved {
            agent = name;
            system_prompt = profile.system_prompt;
            tools = profile.tools;
            self.late_tools = profile.late_tools;
            self.backend = self.provider_backend.clone().or(profile.backend);
            self.waiting_tools.clear();
            self.leaving_tools.clear();
            self.mcp_arrived.clear();
            self.offered_tools = profile.offered;
            self.offered_skills = profile.skills;
            self.context_off = toolset_off.clone();
            if let Some(mode) = profile.mode {
                self.rules.mode = mode;
            }
        }
        self.toolset_off = toolset_off;
        self.sync_blocked();
        let blocked = Arc::clone(&self.blocked);
        let Spawned {
            runtime,
            permission_rx,
            question_rx,
            suggestion_rx,
            transcript,
            mode,
            external,
        } = spawn_runtime(
            &self.provider,
            &tools,
            &model,
            &self.cwd,
            &system_prompt,
            self.effective_rules(),
            self.compaction,
            &self.compaction_prompts,
            &self.plan_prompt,
            &self.goal_prompt,
            &self.handoff_prompt,
            &self.reviewer,
            &self.refusals,
            self.persist_rule,
            self.hooks.as_ref(),
            self.backend.as_ref(),
            self.checkpoints.clone(),
            self.fold,
            session.as_ref(),
            &blocked,
            self.shell_run.clone(),
        );
        // Dropping the old runtime cancels it and asks its worker to stop.
        self.runtime = runtime;
        self.external = external;
        self.permission_rx = permission_rx;
        self.question_rx = question_rx;
        self.suggestion_rx = suggestion_rx;
        self.pending = None;
        self.transcript = transcript;
        // The lines the servers had point into the transcript just replaced.
        self.mcp_lines.clear();
        // Leaving the current session: if it was never used, delete it so an
        // empty session does not clutter the list or the disk. On a
        // same-session rebuild (switch agent, undo) the caller has already
        // taken the session out, so there is nothing to leave here.
        if let Some(old) = self.session.take() {
            discard_if_empty(old);
        }
        self.session = session;
        self.model = model;
        self.agent = agent;
        self.system_prompt = system_prompt;
        self.tools = tools;
        self.catalog.set_mode(mode.get());
        self.mode = mode;
        self.model_choices.clear();
        self.model_fetch = None;
        self.clear_input();
        self.history_pos = None;
        self.draft.clear();
        self.completion = None;
        self.top = 0;
        self.follow = true;
        self.queued = (0, 0);
        self.queued_texts.clear();
        self.pause_requested = false;
        self.stop_requested = false;
        self.context_tokens = 0;
        self.refresh_recent_sessions();
        true
    }

    /// Re-read the sessions the welcome banner offers, the cursor back on the
    /// newest: while the session is fresh, this directory's others that hold
    /// a conversation (or a name) and no panel has open, newest first;
    /// nothing once there is a conversation to show instead.
    pub(crate) fn refresh_recent_sessions(&mut self) {
        self.load_recent_sessions();
        self.recent_selected = 0;
        self.recent_top = 0;
    }

    /// Re-read the banner's sessions once another panel opened or released
    /// one, so an open session leaves the list and a released one comes
    /// back; the cursor stays on the session it was on while that is listed.
    /// A cheap check while nothing changed, for the tick.
    pub(crate) fn follow_open_sessions(&mut self) -> bool {
        if !self.banner_shown() || Session::open_generation() == self.recent_generation {
            return false;
        }
        let selected = self
            .recent_sessions
            .get(self.recent_selected)
            .map(|summary| summary.path.clone());
        self.load_recent_sessions();
        let last = self.recent_sessions.len().saturating_sub(1);
        self.recent_selected = selected
            .and_then(|path| self.recent_sessions.iter().position(|s| s.path == path))
            .unwrap_or(self.recent_selected.min(last));
        if self.recent_sessions.is_empty() {
            self.chat_focus = false;
        }
        true
    }

    fn load_recent_sessions(&mut self) {
        self.recent_generation = Session::open_generation();
        self.recent_sessions = if self.is_fresh() {
            let current = self.session.as_ref().map(Session::path);
            self.session_list()
                .into_iter()
                .filter(|summary| {
                    Some(summary.path.as_path()) != current
                        && (summary.message_count > 0 || summary.name.is_some())
                        && !Session::is_open(&summary.path)
                })
                .collect()
        } else {
            Vec::new()
        };
    }

    /// Whether the welcome banner is up and lists recent sessions, so `Tab`
    /// can take the keyboard into that list and the wheel scrolls it.
    pub(crate) fn recent_list_shown(&self) -> bool {
        self.banner_shown() && !self.recent_sessions.is_empty()
    }

    /// Scroll the banner's list by `delta` rows. With the keyboard in the
    /// list the cursor moves along, since the list keeps it in view.
    pub(crate) fn scroll_recent(&mut self, delta: i32) {
        if self.chat_focus {
            self.move_recent_selection(delta as isize);
            return;
        }
        let max_top = self
            .recent_sessions
            .len()
            .saturating_sub(self.recent_rows.max(1));
        self.recent_top = self
            .recent_top
            .saturating_add_signed(delta as isize)
            .min(max_top);
    }

    /// Move the banner list's cursor by `delta` rows, clamped to the list,
    /// and scroll it into view.
    pub(crate) fn move_recent_selection(&mut self, delta: isize) {
        let last = self.recent_sessions.len().saturating_sub(1);
        self.recent_selected = self.recent_selected.saturating_add_signed(delta).min(last);
        self.scroll_recent_selection_into_view();
    }

    /// Bring the banner list's cursor on screen, scrolling as little as
    /// possible.
    pub(crate) fn scroll_recent_selection_into_view(&mut self) {
        let rows = self.recent_rows.max(1);
        if self.recent_selected < self.recent_top {
            self.recent_top = self.recent_selected;
        } else if self.recent_selected >= self.recent_top + rows {
            self.recent_top = self.recent_selected + 1 - rows;
        }
    }

    /// Sessions of this project, newest first.
    #[must_use]
    pub fn session_list(&self) -> Vec<SessionSummary> {
        self.session_dir
            .as_ref()
            .and_then(|dir| Session::list(dir).ok())
            .unwrap_or_default()
    }

    /// Start a `/handoff`: a read-only model call that distils the unfinished
    /// work into a brief; the verdict arrives as an `AgentEvent::Handoff`.
    pub(crate) fn start_handoff(&mut self) -> Vec<PanelEvent> {
        match self.runtime.handoff() {
            Ok(()) => self.notice(
                termide_i18n::t().agent_notice_handoff_preparing(),
                NoticeKind::Info,
            ),
            Err(PromptError::Busy) => {
                self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn)
            }
            Err(error) => self.notice(
                termide_i18n::t().agent_notice_cannot_handoff_fmt(&error.to_string()),
                NoticeKind::Warn,
            ),
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Write the handoff brief to `HANDOFF.md` in the panel's working directory,
    /// where a fresh session or another agent (including an external one that
    /// reads files) can pick it up.
    pub(crate) fn save_handoff(&mut self, brief: &str) {
        let path = self.cwd.join("HANDOFF.md");
        match std::fs::write(&path, brief) {
            Ok(()) => {
                self.notice(
                    termide_i18n::t().agent_notice_handoff_written_fmt(&path.display().to_string()),
                    NoticeKind::Info,
                );
                self.pending_events
                    .push(PanelEvent::FileChangedOnDisk(path));
            }
            Err(error) => self.notice(
                termide_i18n::t().agent_notice_cannot_write_handoff_fmt(&error.to_string()),
                NoticeKind::Warn,
            ),
        }
    }

    /// Discard the current session and start a fresh one seeded with the
    /// handoff brief as its first request, so work continues from it.
    pub(crate) fn handoff_to_new_session(&mut self, brief: String) -> Vec<PanelEvent> {
        if let Some(old) = self.session.take() {
            discard(old);
        }
        self.switch_session(None);
        self.send(format!(
            "{}\n\n{brief}",
            termide_agent_core::handoff::CONTINUATION_LEAD
        ))
    }

    /// Name the conversation, so the panel title shows it instead of the
    /// first prompt. `false` when there is no session log to record it in.
    pub fn rename_session(&mut self, name: &str) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        match session.set_name(name) {
            Ok(_) => true,
            Err(error) => {
                log::warn!("cannot rename the agent conversation: {error}");
                false
            }
        }
    }

    /// Open the session the picker offered at `index`.
    pub(crate) fn resume_choice(&mut self, index: usize) -> bool {
        let Some(summary) = self.session_choices.get(index).cloned() else {
            return false;
        };
        self.session_choices.clear();
        self.open_session(&summary)
    }

    /// Open the recent session the welcome banner offered at `index`. The
    /// keyboard goes back to the prompt, so the conversation continues there.
    pub(crate) fn open_recent_session(&mut self, index: usize) -> bool {
        let Some(summary) = self.recent_sessions.get(index).cloned() else {
            return false;
        };
        self.chat_focus = false;
        self.open_session(&summary)
    }

    /// Switch to the session `summary` lists, unless it is already open. A
    /// session another panel holds, or one gone from disk, is refused with a
    /// notice.
    fn open_session(&mut self, summary: &SessionSummary) -> bool {
        if self.session.as_ref().map(Session::path) == Some(summary.path.as_path()) {
            return true; // already open
        }
        match Session::open_exclusive(&summary.path) {
            Ok(session) => {
                self.switch_session(Some(session));
            }
            Err(error) => {
                log::warn!("cannot open {}: {error}", summary.path.display());
                self.notice(
                    termide_i18n::t().agent_notice_cannot_open_session_fmt(&error.to_string()),
                    NoticeKind::Error,
                );
            }
        }
        true
    }

    /// Offer to undo the last request: its files go back and the
    /// conversation is rewound to before it.
    pub(crate) fn ask_undo(&mut self) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let files = self
            .checkpoints
            .as_ref()
            .map(|store| {
                store
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .last_files()
            })
            .unwrap_or_default();
        if files.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_nothing_to_undo(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let names: Vec<String> = files
            .iter()
            .map(|path| {
                path.strip_prefix(&self.cwd)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .collect();
        let t = termide_i18n::t();
        let changed = if names.len() == 1 {
            names[0].clone()
        } else {
            t.agent_undo_changed_files_fmt(names.len(), &names.join(", "))
        };
        let form = ChoiceForm::new(
            t.agent_undo_confirm_fmt(&changed),
            vec![t.agent_undo_restore().to_string()],
        )
        .with_cancel(t.agent_undo_keep());
        self.pending = Some(Pending::Undo { form });
        vec![PanelEvent::NeedsRedraw]
    }

    /// Offer to delete the current session (F8, or the panel's `[≡]` menu):
    /// a confirmation modal, since it removes the log for good. The accepted
    /// answer comes back as `PanelCommand::Confirmed(DELETE_SESSION_ACTION)`.
    pub(crate) fn ask_delete_session(&mut self) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let t = termide_i18n::t();
        let name = self
            .session
            .as_ref()
            .and_then(Session::name)
            .map(str::to_string);
        let id = self
            .session
            .as_ref()
            .map(Session::path)
            .and_then(Path::file_stem)
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let label = name
            .clone()
            .unwrap_or_else(|| t.agent_delete_this_session().to_string());
        let confirm = t.agent_delete_confirm_fmt(&label);
        // The log's id under the question, after the name when it has one.
        let message = match (name, id.is_empty()) {
            (_, true) => confirm,
            (Some(name), false) => format!("{confirm}\n{name} · {id}"),
            (None, false) => format!("{confirm}\n{id}"),
        };
        vec![PanelEvent::ShowConfirm {
            message,
            on_confirm: ConfirmAction::Custom(DELETE_SESSION_ACTION.to_string()),
        }]
    }

    /// Ask to delete the recent session under the banner list's cursor (F8 or
    /// Delete while the list has the keyboard); the answer comes back as
    /// `PanelCommand::Confirmed(DELETE_RECENT_ACTION)`.
    pub(crate) fn ask_delete_recent_session(&mut self) -> Vec<PanelEvent> {
        let Some(summary) = self.recent_sessions.get(self.recent_selected) else {
            return vec![];
        };
        let confirm = termide_i18n::t().agent_delete_confirm_fmt(&truncate_title(&summary.label()));
        let id = summary
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let message = format!("{confirm}\n{} · {id}", local_minute(summary.modified));
        self.recent_to_delete = Some(summary.path.clone());
        vec![PanelEvent::ShowConfirm {
            message,
            on_confirm: ConfirmAction::Custom(DELETE_RECENT_ACTION.to_string()),
        }]
    }

    /// Delete the recent session the confirmation was asked for. It is
    /// claimed first, so one another panel opened meanwhile is left alone;
    /// the cursor stays on the row it was on.
    pub(crate) fn perform_delete_recent_session(&mut self) -> Vec<PanelEvent> {
        let Some(path) = self.recent_to_delete.take() else {
            return vec![];
        };
        let result = Session::open_exclusive(&path).and_then(Session::discard);
        let selected = self.recent_selected;
        self.load_recent_sessions();
        self.recent_selected = selected.min(self.recent_sessions.len().saturating_sub(1));
        if self.recent_sessions.is_empty() {
            self.chat_focus = false;
        }
        match result {
            Ok(()) => vec![PanelEvent::NeedsRedraw],
            Err(error) => {
                log::warn!("cannot delete {}: {error}", path.display());
                vec![
                    PanelEvent::SetStatusMessage {
                        message: error.to_string(),
                        is_error: true,
                    },
                    PanelEvent::NeedsRedraw,
                ]
            }
        }
    }

    /// Ask for a new name for the recent session under the banner list's
    /// cursor (F2 while the list has the keyboard); the answer comes back as
    /// `PanelCommand::InputSubmitted(RENAME_RECENT_ACTION)`.
    pub(crate) fn ask_rename_recent_session(&mut self) -> Vec<PanelEvent> {
        let Some(summary) = self.recent_sessions.get(self.recent_selected) else {
            return vec![];
        };
        self.recent_to_rename = Some(summary.path.clone());
        vec![PanelEvent::ShowInput {
            prompt: termide_i18n::t().agent_rename_prompt().to_string(),
            initial_value: summary.name.clone().unwrap_or_default(),
            on_submit: InputAction::Custom(RENAME_RECENT_ACTION.to_string()),
        }]
    }

    /// Rename the recent session the prompt was opened for. It is claimed
    /// first, so one another panel opened meanwhile is left alone. The rename
    /// is a new entry, so the session may move up the list; the cursor follows
    /// it.
    pub(crate) fn perform_rename_recent_session(&mut self, name: &str) -> Vec<PanelEvent> {
        let Some(path) = self.recent_to_rename.take() else {
            return vec![];
        };
        // A session with no messages is listed only for its name: cleared, it
        // is an untouched one, discarded as such rather than left unlisted.
        let result = Session::open_exclusive(&path).and_then(|mut session| {
            session.set_name(name)?;
            if session.is_empty() {
                session.discard()?;
            }
            Ok(())
        });
        let selected = self.recent_selected;
        self.load_recent_sessions();
        self.recent_selected = self
            .recent_sessions
            .iter()
            .position(|summary| summary.path == path)
            .unwrap_or_else(|| selected.min(self.recent_sessions.len().saturating_sub(1)));
        if self.recent_sessions.is_empty() {
            self.chat_focus = false;
        }
        match result {
            Ok(_) => vec![PanelEvent::NeedsRedraw],
            Err(error) => {
                log::warn!("cannot rename {}: {error}", path.display());
                vec![
                    PanelEvent::SetStatusMessage {
                        message: error.to_string(),
                        is_error: true,
                    },
                    PanelEvent::NeedsRedraw,
                ]
            }
        }
    }

    /// Discard the current session and open a fresh one in its place — the
    /// confirmed F8 delete, the same effect as `/clear`.
    pub(crate) fn perform_delete_session(&mut self) -> Vec<PanelEvent> {
        if let Some(old) = self.session.take() {
            discard(old);
        }
        self.switch_session(None);
        vec![PanelEvent::NeedsRedraw]
    }

    /// Offer to fork the current session (F5, `/fork`, or the `[≡]` menu's
    /// Fork session): the log is copied and the copy taken up in a new panel,
    /// so this one keeps working where it stands. The accepted answer comes
    /// back as `PanelCommand::Confirmed(FORK_SESSION_ACTION)`. Unlike a
    /// session switch this asks nothing of the run in flight: the copy is read
    /// off the flushed log, so a busy panel forks too and only its unfinished
    /// answer is missing from the copy.
    pub(crate) fn ask_fork_session(&mut self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let Some(path) = self.session_path() else {
            self.notice(t.agent_notice_fork_no_session(), NoticeKind::Info);
            return vec![PanelEvent::NeedsRedraw];
        };
        let name = self
            .session
            .as_ref()
            .and_then(Session::name)
            .map(str::to_string);
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let label = name
            .clone()
            .unwrap_or_else(|| t.agent_fork_this_session().to_string());
        let confirm = t.agent_fork_confirm_fmt(&label);
        // The log's id under the question, after the name when it has one.
        let message = match (name, id.is_empty()) {
            (_, true) => confirm,
            (Some(name), false) => format!("{confirm}\n{name} · {id}"),
            (None, false) => format!("{confirm}\n{id}"),
        };
        vec![PanelEvent::ShowConfirm {
            message,
            on_confirm: ConfirmAction::Custom(FORK_SESSION_ACTION.to_string()),
        }]
    }

    /// The confirmed F5: ask the app to copy this session's log and open the
    /// copy in a panel of its own. The panel copies nothing itself — the app
    /// owns panel creation and the configuration a new panel runs on — and this
    /// one goes on writing its own log, so both keep working.
    pub(crate) fn perform_fork_session(&mut self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        match self.session_path() {
            Some(path) => vec![PanelEvent::ForkAgentSession {
                session: path.to_path_buf(),
                cwd: self.cwd.clone(),
            }],
            None => {
                self.notice(t.agent_notice_fork_no_session(), NoticeKind::Info);
                vec![]
            }
        }
    }

    /// The banner's directory: ask the app to let the user pick another one
    /// to work in. Only a session nothing was sent to moves, so no message
    /// or tool call is left pointing at the directory it was made in.
    pub(crate) fn ask_change_cwd(&mut self) -> Vec<PanelEvent> {
        if !self.is_fresh() || self.is_busy() {
            return Vec::new();
        }
        vec![PanelEvent::ChangeAgentCwd {
            session: self.session_path().map(std::path::Path::to_path_buf),
            cwd: self.cwd.clone(),
        }]
    }

    /// A read-only summary of the current session, shown in an info modal
    /// (F3, the `[≡]` menu's "Session info", or `/usage`).
    pub(crate) fn session_summary(&self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let name = self
            .session
            .as_ref()
            .and_then(Session::name)
            .map(str::to_string)
            .unwrap_or_else(|| t.ai_session_untitled().to_string());
        let messages = self
            .transcript
            .items()
            .iter()
            .filter(|item| matches!(item, Item::User { .. } | Item::Assistant { .. }))
            .count();
        let mut rows: Vec<(String, String)> = vec![(t.agent_info_session().into(), name)];
        if let Some(session) = self.session.as_ref() {
            if let Some(id) = session
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            {
                rows.push((t.agent_info_log().into(), id));
            }
        }
        rows.push((t.agent_info_agent().into(), self.agent.clone()));
        rows.push((t.agent_info_provider().into(), self.provider_kind.clone()));
        rows.push((t.agent_info_model().into(), self.model.id.clone()));
        rows.push((
            t.agent_info_mode().into(),
            self.mode.get().label().to_string(),
        ));
        rows.push((
            t.agent_info_directory().into(),
            shorten_path(&self.cwd, usize::MAX),
        ));
        if let Some(session) = self.session.as_ref() {
            rows.push((
                t.agent_info_created().into(),
                local_minute(session.header().created),
            ));
            if let Some(last) = session.entries().last().map(|e| e.timestamp) {
                rows.push((t.agent_info_last_active().into(), local_minute(last)));
            }
            let compactions = session
                .entries()
                .iter()
                .filter(|e| matches!(e.kind, EntryKind::Compaction { .. }))
                .count();
            rows.push((t.agent_info_compactions().into(), compactions.to_string()));
        }
        rows.push((t.agent_info_messages().into(), messages.to_string()));
        rows.push((t.agent_info_tokens().into(), self.token_totals()));
        rows.push((
            t.agent_info_context().into(),
            format!(
                "{} / {}",
                format_tokens(self.context_tokens),
                format_tokens(self.model.context_window)
            ),
        ));
        // How much the clean mechanism has shrunk shell output this session,
        // when any ran: raw → cleaned and the percentage saved.
        if self.clean_raw_bytes > 0 {
            let saved = self.clean_raw_bytes.saturating_sub(self.clean_out_bytes);
            let percent = saved * 100 / self.clean_raw_bytes;
            rows.push((
                t.agent_info_output_cleaned().into(),
                format!(
                    "{} → {} (−{percent}%)",
                    format_bytes(self.clean_raw_bytes),
                    format_bytes(self.clean_out_bytes),
                ),
            ));
        }
        vec![PanelEvent::ShowInfo {
            title: t.agent_session_info().to_string(),
            rows,
        }]
    }

    /// Put the last request's files back, rewind the session to before it
    /// and rebuild the agent from there.
    pub(crate) fn perform_undo(&mut self) -> Vec<PanelEvent> {
        let Some(store) = self.checkpoints.clone() else {
            return vec![];
        };
        let undone = store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .undo_last();
        let undone = match undone {
            Ok(undone) => undone,
            Err(error) => {
                self.notice(
                    termide_i18n::t().agent_notice_cannot_undo_fmt(&error.to_string()),
                    NoticeKind::Error,
                );
                return vec![PanelEvent::NeedsRedraw];
            }
        };
        let mut events: Vec<PanelEvent> = undone
            .files
            .iter()
            .map(|path| PanelEvent::FileChangedOnDisk(path.clone()))
            .collect();
        if let Some(session) = &mut self.session {
            if let Err(error) = session.rewind_to(undone.leaf_before.as_deref()) {
                log::warn!("agent session rewind failed: {error}");
            }
        }
        let count = undone.files.len();
        let session = self.session.take();
        self.switch_session(session);
        let t = termide_i18n::t();
        self.notice(
            t.agent_notice_undid_fmt(count, t.pluralize(count, "file")),
            NoticeKind::Info,
        );
        events.push(PanelEvent::NeedsRedraw);
        events
    }
}

/// A byte count as `B`/`KB`/`MB`, for the output-cleaning diagnostic.
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1}MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1000 {
        format!("{}KB", (bytes + 500) / 1000)
    } else {
        format!("{bytes}B")
    }
}
