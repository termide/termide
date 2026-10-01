//! The model, mode, agent, connection and prompt pickers, and the switches
//! they drive.

use std::sync::Arc;

use termide_agent_core::{Mode, ModelInfo, ModelSpec, ThinkingLevel};
use termide_core::{InputAction, PanelEvent, SelectAction};

use crate::runtime::spawn_model_list;
use crate::{
    AgentPanel, NoticeKind, AGENT_ACTION, MODEL_ACTION, MODEL_INPUT_ACTION, MODE_ACTION,
    PROMPTS_ACTION, REASONING_ACTION,
};

/// Picker prefix: `●` on the current entry, blank otherwise.
pub(crate) fn current_mark(current: bool) -> &'static str {
    if current {
        "● "
    } else {
        "  "
    }
}

impl AgentPanel {
    /// Offer the endpoint's models. The list is fetched off the UI thread
    /// and the picker opens from `tick()` when it arrives; an endpoint that
    /// cannot list models falls back to a typed id.
    pub(crate) fn request_model_list(&mut self) -> Vec<PanelEvent> {
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        if self.model_fetch.is_some() {
            return vec![];
        }
        self.model_fetch = Some(spawn_model_list(Arc::clone(&self.provider)));
        vec![PanelEvent::SetStatusMessage {
            message: termide_i18n::t().agent_models_loading().to_string(),
            is_error: false,
        }]
    }

    pub(crate) fn model_picker(&mut self, result: Result<Vec<ModelInfo>, String>) -> PanelEvent {
        let t = termide_i18n::t();
        let mut models = match result {
            Ok(models) => models,
            Err(error) => {
                self.notice(
                    termide_i18n::t().agent_notice_model_list_unavailable_fmt(&error.to_string()),
                    NoticeKind::Info,
                );
                Vec::new()
            }
        };
        if models.is_empty() {
            return self.model_input();
        }
        if !models.iter().any(|m| m.id == self.model.id) {
            models.insert(
                0,
                ModelInfo {
                    id: self.model.id.clone(),
                    context_window: None,
                },
            );
        }
        let mut options: Vec<String> = models
            .iter()
            .map(|m| format!("{}{}", current_mark(m.id == self.model.id), m.id))
            .collect();
        options.push(format!("  {}", t.agent_model_other()));
        self.model_choices = models;
        PanelEvent::ShowSelect {
            title: t.agent_change_model().to_string(),
            options,
            on_select: SelectAction::Custom(MODEL_ACTION.to_string()),
        }
    }

    /// The model picker for an external (ACP) agent: the models it advertised,
    /// current marked, switched over ACP rather than through the built-in loop.
    pub(crate) fn acp_model_picker(&mut self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let models = self.runtime.available_models();
        if models.is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_no_model_choices(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let current = self.runtime.current_model();
        let options = models
            .iter()
            .map(|m| {
                let mark = current_mark(current.as_deref() == Some(m.id.as_str()));
                if m.name == m.id {
                    format!("{mark}{}", m.id)
                } else {
                    format!("{mark}{} · {}", m.name, m.id)
                }
            })
            .collect();
        self.acp_models = models;
        vec![PanelEvent::ShowSelect {
            title: t.agent_change_model().to_string(),
            options,
            on_select: SelectAction::Custom(MODEL_ACTION.to_string()),
        }]
    }

    pub(crate) fn model_input(&self) -> PanelEvent {
        PanelEvent::ShowInput {
            prompt: termide_i18n::t().agent_model_prompt().to_string(),
            initial_value: self.model.id.clone(),
            on_submit: InputAction::Custom(MODEL_INPUT_ACTION.to_string()),
        }
    }

    pub(crate) fn mode_picker(&self) -> PanelEvent {
        let t = termide_i18n::t();
        let current = self.mode.get();
        let options = Mode::ALL
            .iter()
            .map(|mode| {
                let text = match mode {
                    Mode::Ask => t.agent_mode_ask(),
                    Mode::Plan => t.agent_mode_plan(),
                    Mode::Edit => t.agent_mode_edit(),
                    Mode::Configured => t.agent_mode_configured(),
                    Mode::Auto => t.agent_mode_auto(),
                    Mode::All => t.agent_mode_all(),
                };
                format!("{}{text}", current_mark(*mode == current))
            })
            .collect();
        PanelEvent::ShowSelect {
            title: t.agent_change_mode().to_string(),
            options,
            on_select: SelectAction::Custom(MODE_ACTION.to_string()),
        }
    }

    /// Switch the permission mode. The handle is shared with the hooks, so
    /// a run in flight sees the new mode at its next tool call.
    pub(crate) fn set_mode(&mut self, mode: Mode) -> PanelEvent {
        let was_plan = self.mode.get() == Mode::Plan;
        self.mode.set(mode);
        self.rules.mode = mode;
        self.catalog.set_mode(mode);
        self.runtime.set_mode(mode);
        if was_plan != (mode == Mode::Plan) {
            self.sync_system_prompt();
        }
        PanelEvent::SetStatusMessage {
            message: format!(
                "{}: {}",
                termide_i18n::t().agent_change_mode(),
                mode.label()
            ),
            is_error: false,
        }
    }

    /// Offer the prompt templates; choosing one puts `/<name> ` into the
    /// input so arguments can follow.
    pub(crate) fn prompt_picker(&mut self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let prompts = self.catalog.prompts();
        if prompts.is_empty() {
            return vec![PanelEvent::SetStatusMessage {
                message: t.agent_no_prompts().to_string(),
                is_error: false,
            }];
        }
        let options = prompts
            .iter()
            .map(|p| {
                let mut line = format!("/{}", p.name);
                if !p.argument_hint.is_empty() {
                    line.push(' ');
                    line.push_str(&p.argument_hint);
                }
                if !p.description.is_empty() {
                    line.push_str(" · ");
                    line.push_str(&p.description);
                }
                line
            })
            .collect();
        self.prompt_choices = prompts;
        vec![PanelEvent::ShowSelect {
            title: t.agent_prompts().to_string(),
            options,
            on_select: SelectAction::Custom(PROMPTS_ACTION.to_string()),
        }]
    }

    pub(crate) fn agent_picker(&mut self) -> PanelEvent {
        let t = termide_i18n::t();
        let entries = self.catalog.list();
        let options = entries
            .iter()
            .map(|entry| {
                let mark = current_mark(entry.name == self.agent);
                if entry.description.is_empty() {
                    format!("{mark}{}", entry.name)
                } else {
                    format!("{mark}{} · {}", entry.name, entry.description)
                }
            })
            .collect();
        self.agent_choices = entries.into_iter().map(|entry| entry.name).collect();
        PanelEvent::ShowSelect {
            title: t.agent_change_agent().to_string(),
            options,
            on_select: SelectAction::Custom(AGENT_ACTION.to_string()),
        }
    }

    /// Continue the session as another agent: its prompt and tools, and its
    /// model and mode when the definition names them. Refused while a run
    /// is in flight.
    pub(crate) fn switch_agent(&mut self, name: &str) -> bool {
        if name == self.agent {
            return true;
        }
        let Some(profile) = self.catalog.resolve_without(name, &self.toolset_off) else {
            self.notice(
                termide_i18n::t().agent_notice_no_agent_fmt(name),
                NoticeKind::Warn,
            );
            return false;
        };
        // An external agent, or leaving one: the runtime is rebuilt on the
        // same session log, which is replayed into the transcript only.
        if profile.backend.is_some() || self.external {
            if self.is_busy() {
                self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
                return false;
            }
            if let Some(session) = &mut self.session {
                if let Err(error) = session.append_agent_change(name) {
                    log::warn!("agent session write failed: {error}");
                }
            }
            self.agent = name.to_string();
            self.system_prompt = profile.system_prompt;
            self.tools = profile.tools;
            self.late_tools = profile.late_tools;
            self.backend = profile.backend;
            if let Some(mode) = profile.mode {
                self.rules.mode = mode;
            }
            if let Some(id) = profile.model {
                self.model.id = id;
            }
            let session = self.session.take();
            self.switch_session(session);
            if !self.is_fresh() {
                self.notice(
                    termide_i18n::t().agent_notice_agent_fmt(name),
                    NoticeKind::Info,
                );
            }
            return true;
        }
        let model = match profile.model {
            Some(id) if id != self.model.id => ModelSpec {
                id,
                ..self.model.clone()
            },
            _ => self.model.clone(),
        };
        let mode_after = profile.mode.unwrap_or(self.mode.get());
        let prompt = if mode_after == Mode::Plan {
            self.plan_prompt.apply(&profile.system_prompt)
        } else {
            profile.system_prompt.clone()
        };
        let tools = profile.tools.clone();
        let worker_model = model.clone();
        if let Err(error) = self.runtime.update(Box::new(move |agent| {
            agent.set_system_prompt(prompt);
            *agent.tools_mut() = tools;
            agent.set_model(worker_model);
        })) {
            self.notice(
                termide_i18n::t().agent_notice_cannot_switch_agent_fmt(&error.to_string()),
                NoticeKind::Warn,
            );
            return false;
        }
        if model.id != self.model.id {
            if let Some(session) = &mut self.session {
                if let Err(error) = session.append_model_change(
                    self.provider_kind.as_str(),
                    &model.id,
                    Some(model.context_window),
                ) {
                    log::warn!("agent session write failed: {error}");
                }
            }
        }
        self.model = model;
        self.system_prompt = profile.system_prompt;
        self.tools = profile.tools;
        self.late_tools = profile.late_tools;
        self.waiting_tools.clear();
        self.leaving_tools.clear();
        // The new agent's prompt is built without what the session switched
        // off (the cache is lost anyway), and it offers its own lists.
        self.mcp_arrived.clear();
        self.offered_tools = profile.offered;
        self.offered_skills = profile.skills;
        self.context_off = self.toolset_off.clone();
        self.sync_blocked();
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_agent_change(name) {
                log::warn!("agent session write failed: {error}");
            }
        }
        if let Some(mode) = profile.mode {
            self.mode.set(mode);
            self.rules.mode = mode;
            self.catalog.set_mode(mode);
        }
        self.agent = name.to_string();
        if !self.is_fresh() {
            self.notice(
                termide_i18n::t().agent_notice_agent_fmt(name),
                NoticeKind::Info,
            );
        }
        true
    }

    /// Run the session on connection `name`: its endpoint and its
    /// model, the rest of the session kept. The agent restarts on the same
    /// log, so a built-in loop carries the conversation over; a CLI agent
    /// does not, so switching to or from one is refused once it has begun.
    pub(crate) fn switch_connection(&mut self, name: &str) -> bool {
        if name == self.connection {
            return true;
        }
        let t = termide_i18n::t();
        let Some(connections) = self.connections.clone() else {
            return false;
        };
        if self.is_busy() {
            self.notice(t.agent_notice_busy(), NoticeKind::Warn);
            return false;
        }
        let Some(choice) = connections.build(name, &self.agent) else {
            self.notice(t.agent_notice_no_connection_fmt(name), NoticeKind::Warn);
            return false;
        };
        if (choice.backend.is_some() || self.external) && !self.is_fresh() {
            self.notice(t.agent_notice_connection_before_first(), NoticeKind::Warn);
            return false;
        }
        connections.activate(&choice);
        self.provider = Arc::clone(&choice.provider);
        self.provider_kind = choice.kind.clone();
        self.connection = choice.name.clone();
        // The connection's model replaces the one in use: another endpoint seldom
        // serves the same id.
        let model = ModelSpec {
            id: choice.model.clone(),
            context_window: choice.context_window,
            ..self.model.clone()
        };
        self.configured_model = model.clone();
        self.model = model;
        self.model_choices.clear();
        self.provider_backend = choice.backend.clone();
        self.backend = self.provider_backend.clone().or_else(|| {
            self.catalog
                .resolve(&self.agent)
                .and_then(|profile| profile.backend)
        });
        if let Some(session) = &mut self.session {
            let written = session.append_connection_change(name).and_then(|_| {
                session.append_model_change(
                    &choice.kind,
                    &self.model.id,
                    Some(self.model.context_window),
                )
            });
            if let Err(error) = written {
                log::warn!("agent session write failed: {error}");
            }
        }
        let fresh = self.is_fresh();
        let session = self.session.take();
        self.switch_session(session);
        // The silent window probe asks the new endpoint.
        self.context_probe = (!self.external).then(|| spawn_model_list(Arc::clone(&self.provider)));
        if !fresh {
            self.notice(t.agent_notice_connection_fmt(name), NoticeKind::Info);
        }
        true
    }

    /// Continue the session on another model of the same endpoint. The
    /// context window follows the endpoint's figure when it gave one and
    /// stays as configured otherwise; the token limit is always the
    /// configured one. Refused while a run is in flight.
    pub(crate) fn switch_model(&mut self, id: &str, context_window: Option<u64>) -> bool {
        let id = id.trim();
        if id.is_empty() {
            return false;
        }
        let new_window = context_window.unwrap_or(self.model.context_window);
        let id_changed = id != self.model.id;
        // Re-selecting the same model still adopts a newly-known context window
        // (a provider's `max_model_len`); nothing to do only when both match.
        if !id_changed && new_window == self.model.context_window {
            return true;
        }
        let model = ModelSpec {
            id: id.to_string(),
            context_window: new_window,
            ..self.model.clone()
        };
        let worker_model = model.clone();
        if let Err(error) = self
            .runtime
            .update(Box::new(move |agent| agent.set_model(worker_model)))
        {
            self.notice(
                termide_i18n::t().agent_notice_cannot_switch_model_fmt(&error.to_string()),
                NoticeKind::Warn,
            );
            return false;
        }
        self.model = model;
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_model_change(
                self.provider_kind.as_str(),
                id,
                Some(self.model.context_window),
            ) {
                log::warn!("agent session write failed: {error}");
            }
        }
        // A silent window adoption (same id) leaves no notice; nor does a fresh
        // session, where the banner shows the new model instead.
        if id_changed && !self.is_fresh() {
            self.notice(
                termide_i18n::t().agent_notice_model_fmt(id),
                NoticeKind::Info,
            );
        }
        // Another model has no cache of this prompt: what is refused can
        // leave the context for free.
        if id_changed && self.toolset_off != self.context_off {
            self.refresh_context();
        }
        true
    }

    /// Adopt what a `list_models` result says: with the model left to the
    /// provider, its first model — for this session and the next ones the
    /// panel starts; otherwise the active model's real context window, when
    /// it is known and differs. Returns whether anything changed.
    pub(crate) fn adopt_listed_models(&mut self, models: &[ModelInfo]) -> bool {
        if self.is_busy() {
            return false;
        }
        if self.model.id.is_empty() {
            let Some(first) = models.first() else {
                return false;
            };
            let adopted = self.switch_model(&first.id, first.context_window);
            if adopted && self.configured_model.id.is_empty() {
                self.configured_model.id = self.model.id.clone();
                self.configured_model.context_window = self.model.context_window;
            }
            return adopted;
        }
        let Some(window) = models
            .iter()
            .find(|m| m.id == self.model.id)
            .and_then(|m| m.context_window)
        else {
            return false;
        };
        if window == self.model.context_window {
            return false;
        }
        let id = self.model.id.clone();
        self.switch_model(&id, Some(window))
    }

    /// The reasoning levels the model offers, lowest first; empty when it
    /// cannot be asked (an external agent, a server that decides alone).
    pub(crate) fn thinking_levels(&self) -> Vec<ThinkingLevel> {
        if self.external {
            return Vec::new();
        }
        self.provider.thinking_levels(&self.model.id)
    }

    /// The level the model actually gets: the one asked for, or the
    /// nearest it has.
    pub(crate) fn effective_thinking(&self, levels: &[ThinkingLevel]) -> ThinkingLevel {
        self.model.thinking.nearest(levels)
    }

    /// How a level reads in the status bar and the picker: an on/off model
    /// shows the switch, a graded one the level's name.
    pub(crate) fn thinking_label(level: ThinkingLevel, levels: &[ThinkingLevel]) -> String {
        let t = termide_i18n::t();
        if is_switch(levels) {
            if level == ThinkingLevel::Off {
                t.agent_chip_off()
            } else {
                t.agent_chip_on()
            }
            .to_string()
        } else {
            level.label().to_string()
        }
    }

    /// The chip's action: an on/off model flips, a graded one opens the
    /// level picker.
    pub(crate) fn reasoning_action(&mut self) -> Vec<PanelEvent> {
        let levels = self.thinking_levels();
        if levels.is_empty() {
            return Vec::new();
        }
        let current = self.effective_thinking(&levels);
        if is_switch(&levels) {
            let next = levels.into_iter().find(|level| *level != current);
            if let Some(level) = next {
                self.set_thinking(level);
            }
            return vec![PanelEvent::NeedsRedraw];
        }
        let options = levels
            .iter()
            .map(|level| format!("{}{}", current_mark(*level == current), level.label()))
            .collect();
        vec![PanelEvent::ShowSelect {
            title: termide_i18n::t().agent_change_reasoning().to_string(),
            options,
            on_select: SelectAction::Custom(REASONING_ACTION.to_string()),
        }]
    }

    /// Ask the model for `level` of reasoning. Applies to the next request
    /// and is remembered in the session log so a resume comes back with the
    /// same choice.
    pub(crate) fn set_thinking(&mut self, level: ThinkingLevel) -> bool {
        if self.external {
            return false;
        }
        if self.is_busy() {
            self.notice(termide_i18n::t().agent_notice_busy(), NoticeKind::Warn);
            return false;
        }
        let mut model = self.model.clone();
        model.thinking = level;
        let worker_model = model.clone();
        if let Err(error) = self
            .runtime
            .update(Box::new(move |agent| agent.set_model(worker_model)))
        {
            self.notice(
                termide_i18n::t().agent_notice_cannot_change_reasoning_fmt(&error.to_string()),
                NoticeKind::Warn,
            );
            return false;
        }
        self.model = model;
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_thinking_change(level) {
                log::warn!("agent session write failed: {error}");
            }
        }
        let levels = self.thinking_levels();
        let label = Self::thinking_label(self.effective_thinking(&levels), &levels);
        self.notice(
            termide_i18n::t().agent_notice_reasoning_fmt(&label),
            NoticeKind::Info,
        );
        true
    }
}

/// Whether `levels` is an on/off switch rather than a scale.
fn is_switch(levels: &[ThinkingLevel]) -> bool {
    levels.len() == 2 && levels[0] == ThinkingLevel::Off
}
