//! What the model has in its context: the session's tools, skills and MCP
//! tools the user can switch off, the guard that refuses those still in the
//! context, and the system prompt they share.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, PoisonError, RwLock};

use termide_agent_core::{
    Hooks, LateTools, Mode, PromptError, ToolCall, ToolContext, ToolDecision,
};
use termide_core::{ChecklistGroup, ChecklistItem, ChecklistRefresh, PanelEvent};

use crate::{AgentPanel, NoticeKind};

/// What the session switched off but the model still has in its context,
/// shared with the guard that refuses it.
pub(crate) type Blocked = Arc<RwLock<BTreeSet<String>>>;

/// The checklist of the session's tools, skills and MCP tools.
pub(crate) const TOOLSET_ACTION: &str = "agent_toolset";

/// Refuses what the session switched off while it is still in the model's
/// context: a tool by its name, a skill by the name the `skill` tool loads.
pub(crate) struct ToolsetGuard {
    pub(crate) blocked: Blocked,
}

impl Hooks for ToolsetGuard {
    fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
        let blocked = self.blocked.read().unwrap_or_else(PoisonError::into_inner);
        let skill = (call.name == "skill")
            .then(|| call.arguments.get("name").and_then(|v| v.as_str()))
            .flatten()
            .map(|name| format!("skill:{name}"));
        if blocked.contains(&call.name) || skill.is_some_and(|key| blocked.contains(&key)) {
            return ToolDecision::Block {
                reason: "The user switched this off for the session; do not call it again."
                    .to_string(),
            };
        }
        ToolDecision::Allow
    }
}

impl AgentPanel {
    /// The system prompt as the agent receives it, written next to the
    /// session logs (or to the temp directory without them) so it can be
    /// opened in a viewer.
    pub(crate) fn write_system_prompt(&self) -> std::io::Result<PathBuf> {
        let dir = self.session_dir.clone().unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("system-prompt.md");
        std::fs::write(&path, self.effective_system_prompt())?;
        Ok(path)
    }

    /// The prompt the worker runs on: the agent's, plus the plan-mode
    /// instructions while that mode is on.
    pub(crate) fn effective_system_prompt(&self) -> String {
        if self.mode.get() == Mode::Plan {
            self.plan_prompt.apply(&self.system_prompt)
        } else {
            self.system_prompt.clone()
        }
    }

    /// Hand the worker the current effective prompt. During a run the
    /// update is refused; it is retried when the run ends.
    pub(crate) fn sync_system_prompt(&mut self) {
        let prompt = self.effective_system_prompt();
        match self
            .runtime
            .update(Box::new(move |agent| agent.set_system_prompt(prompt)))
        {
            Ok(()) => self.prompt_stale = false,
            Err(PromptError::Busy) => self.prompt_stale = true,
            // An external agent has no prompt of ours to update.
            Err(_) => self.prompt_stale = false,
        }
    }

    /// Refuse what is switched off but still in the model's context.
    pub(crate) fn sync_blocked(&self) {
        *self.blocked.write().unwrap_or_else(PoisonError::into_inner) = self
            .toolset_off
            .difference(&self.context_off)
            .cloned()
            .collect();
    }

    /// Rebuild the prompt and the registry without what the session switched
    /// off, so it leaves the model's context. Only worth it where the prompt
    /// cache is lost anyway (before the first request, after a compaction,
    /// on an agent or model switch): elsewhere it would cost the cache.
    /// During a run it waits for the run to end.
    pub(crate) fn refresh_context(&mut self) {
        if self.external {
            return;
        }
        if self.is_busy() {
            self.context_stale = true;
            return;
        }
        let Some(profile) = self.catalog.resolve_without(&self.agent, &self.toolset_off) else {
            return;
        };
        let mut tools = profile.tools;
        // The MCP tools that already arrived stay, save those switched off;
        // the profile's own subscription is not taken, so they do not arrive
        // (and announce themselves) twice.
        for (_, tool) in &self.mcp_arrived {
            if !self.toolset_off.contains(tool.name()) {
                tools.insert(Arc::clone(tool));
            }
        }
        let prompt = if self.mode.get() == Mode::Plan {
            self.plan_prompt.apply(&profile.system_prompt)
        } else {
            profile.system_prompt.clone()
        };
        let worker_tools = tools.clone();
        match self.runtime.update(Box::new(move |agent| {
            agent.set_system_prompt(prompt);
            *agent.tools_mut() = worker_tools;
        })) {
            Ok(()) => {
                self.system_prompt = profile.system_prompt;
                self.tools = tools;
                self.waiting_tools.clear();
                self.leaving_tools.clear();
                self.context_off = self.toolset_off.clone();
                self.context_stale = false;
                self.sync_blocked();
            }
            Err(PromptError::Busy) => self.context_stale = true,
            // A stopped worker takes no change; the panel keeps showing what
            // the agent really has, and the journal says why.
            Err(error) => log::warn!("The agent's tools and prompt were not updated: {error}"),
        }
    }

    /// The checklist of what the session may use: the built-in tools, the
    /// skills, each MCP server's tools. Before the first request anything
    /// toggles freely; after it, what is in the context toggles between
    /// allowed and refused, and what is out of it stays out.
    pub(crate) fn toolset_items(&self) -> Vec<ChecklistItem> {
        let t = termide_i18n::t();
        let fresh = self.is_fresh();
        let item = |key: String, label: String, group: String| {
            let off = self.toolset_off.contains(&key);
            let in_context = !self.context_off.contains(&key);
            let enabled = fresh || in_context;
            let note = if !enabled {
                t.agent_toolset_note_new_session()
            } else if off && !fresh {
                t.agent_toolset_note_refused()
            } else {
                ""
            };
            ChecklistItem {
                key,
                label,
                group,
                checked: !off,
                enabled,
                note: note.to_string(),
            }
        };
        let mut items: Vec<ChecklistItem> = self
            .offered_tools
            .iter()
            // The skill loader goes with the skills, which have their own items.
            .filter(|name| name.as_str() != "skill")
            .map(|name| {
                item(
                    name.clone(),
                    name.clone(),
                    t.agent_toolset_builtin().to_string(),
                )
            })
            .collect();
        items.extend(self.offered_skills.iter().map(|name| {
            item(
                format!("skill:{name}"),
                name.clone(),
                t.agent_toolset_skills().to_string(),
            )
        }));
        items.extend(self.mcp_arrived.iter().map(|(server, tool)| {
            item(
                tool.name().to_string(),
                tool.name().to_string(),
                t.agent_toolset_mcp_fmt(server),
            )
        }));
        items
    }

    /// The hint line under the list's title: how it works, and — where there
    /// are MCP headings — what its buttons do.
    pub(crate) fn toolset_prompt(&self, groups: &[ChecklistGroup]) -> String {
        let t = termide_i18n::t();
        let mut prompt = t.agent_toolset_prompt().to_string();
        if !groups.is_empty() {
            prompt.push(' ');
            prompt.push_str(t.agent_toolset_buttons_hint());
        }
        prompt
    }

    /// Ask to bring the toolset checklist already open up to date. Cheap and
    /// quiet: the app drops it when no such list is open, so the panel may
    /// raise it on every tick that changed what the list shows.
    pub(crate) fn toolset_refresh(&self) -> PanelEvent {
        let groups = self.toolset_groups();
        let prompt = Some(self.toolset_prompt(&groups));
        PanelEvent::RefreshChecklist(ChecklistRefresh {
            action: TOOLSET_ACTION.to_string(),
            prompt,
            items: self.toolset_items(),
            groups,
        })
    }

    /// Apply the checklist: what is left unchecked is switched off. Before
    /// the first request that takes it out of the context at once; later it
    /// is refused until a compaction takes it out.
    pub(crate) fn apply_toolset(&mut self, checked: &[String]) {
        let fresh = self.is_fresh();
        let before = self.toolset_off.clone();
        let mut off = self.toolset_off.clone();
        for item in self.toolset_items() {
            if !item.enabled {
                continue;
            }
            if checked.contains(&item.key) {
                off.remove(&item.key);
            } else {
                off.insert(item.key);
            }
        }
        if off == self.toolset_off {
            return;
        }
        self.toolset_off = off;
        if let Some(session) = &mut self.session {
            let disabled: Vec<String> = self.toolset_off.iter().cloned().collect();
            if let Err(error) = session.append_toolset(&disabled) {
                log::warn!("agent session write failed: {error}");
            }
        }
        if fresh {
            self.refresh_context();
        } else {
            self.sync_blocked();
        }
        self.announce_toolset(&before);
    }

    /// Say what a change of the toolset did. A server whose tools changed
    /// gets its line — rewritten in place under the banner, a new one in a
    /// conversation, since from here on the model has a different set; the
    /// built-in tools and skills get one line between them in a
    /// conversation, and the banner's `tools` count before it.
    fn announce_toolset(&mut self, before: &BTreeSet<String>) {
        let changed: BTreeSet<&String> = before.symmetric_difference(&self.toolset_off).collect();
        let mut servers: Vec<String> = Vec::new();
        for (server, tool) in &self.mcp_arrived {
            if changed.contains(&tool.name().to_string()) && !servers.contains(server) {
                servers.push(server.clone());
            }
        }
        for server in servers {
            let line = self.mcp_tools_line(&server);
            self.mcp_notice(&server, line, NoticeKind::Info);
        }
        if self.banner_shown() {
            return;
        }
        let is_mcp = |key: &str| self.mcp_arrived.iter().any(|(_, tool)| tool.name() == key);
        let shown = |keys: Vec<&String>| -> String {
            let names: Vec<&str> = keys
                .into_iter()
                .map(|key| key.strip_prefix("skill:").unwrap_or(key))
                .collect();
            if names.is_empty() {
                "—".to_string()
            } else {
                names.join(", ")
            }
        };
        let off: Vec<&String> = self
            .toolset_off
            .difference(before)
            .filter(|key| !is_mcp(key))
            .collect();
        let on: Vec<&String> = before
            .difference(&self.toolset_off)
            .filter(|key| !is_mcp(key))
            .collect();
        if off.is_empty() && on.is_empty() {
            return;
        }
        let text = termide_i18n::t().agent_notice_toolset_changed_fmt(&shown(off), &shown(on));
        self.notice(text, NoticeKind::Info);
    }

    /// `(on, all)` of the tools `server` brought.
    pub(crate) fn mcp_tool_count(&self, server: &str) -> (usize, usize) {
        let tools: Vec<&str> = self
            .mcp_arrived
            .iter()
            .filter(|(s, _)| s == server)
            .map(|(_, tool)| tool.name())
            .collect();
        let off = tools
            .iter()
            .filter(|name| self.toolset_off.contains(**name))
            .count();
        (tools.len() - off, tools.len())
    }

    /// A connected server's line: its tools, and how many are on when not all.
    pub(crate) fn mcp_tools_line(&self, server: &str) -> String {
        let t = termide_i18n::t();
        match self.mcp_tool_count(server) {
            (on, all) if on == all => t.agent_notice_mcp_connected_fmt(server, all),
            (on, all) => t.agent_notice_mcp_tools_on_fmt(server, on, all),
        }
    }

    /// A line about the MCP server `server`. Under the banner a server has
    /// one, rewritten in place as it changes, since there it is the state of
    /// things and not a history; in a conversation every change is a line of
    /// its own.
    pub(crate) fn mcp_notice(&mut self, server: &str, text: String, kind: NoticeKind) {
        if self.banner_shown() {
            if let Some(&index) = self.mcp_lines.get(server) {
                if self.transcript.replace_notice(index, text.clone(), kind) {
                    return;
                }
            }
            self.mcp_lines
                .insert(server.to_string(), self.transcript.items().len());
        }
        self.notice(text, kind);
    }

    /// `on/all` of what the session offers, for the banner and the chip.
    pub(crate) fn toolset_counts(&self) -> (usize, usize) {
        let all = self
            .offered_tools
            .iter()
            .filter(|name| name.as_str() != "skill")
            .count()
            + self.offered_skills.len()
            + self.mcp_arrived.len();
        let off = self.toolset_off.len();
        (all.saturating_sub(off), all)
    }

    /// Take in tools that finished connecting and hand them to the worker as
    /// soon as it is between runs. `true` when something was shown.
    pub(crate) fn poll_late_tools(&mut self) -> bool {
        let mut arrivals = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.late_tools {
            loop {
                match rx.try_recv() {
                    Ok(event) => arrivals.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.late_tools = None;
        }
        let changed = !arrivals.is_empty();
        let t = termide_i18n::t();
        for event in arrivals {
            // Every event but a sign-in under way speaks for all the server's
            // tools: what it had before goes, and a `Ready` brings the new set.
            let replaced = !matches!(event, LateTools::LoginStarted { .. })
                && self.withdraw_mcp_source(event.source());
            match event {
                LateTools::Ready { source, tools } => {
                    let asked = self.mcp_reconnecting.remove(&source);
                    let text = if asked {
                        t.agent_notice_mcp_reconnected_fmt(&source, tools.len())
                    } else if replaced {
                        t.agent_notice_mcp_updated_fmt(&source, tools.len())
                    } else {
                        t.agent_notice_mcp_connected_fmt(&source, tools.len())
                    };
                    // Every one is listed in the checklist; one switched off
                    // stays out of the registry, and so out of the context.
                    for tool in tools {
                        let name = tool.name().to_string();
                        self.mcp_arrived.push((source.clone(), Arc::clone(&tool)));
                        if self.toolset_off.contains(&name) {
                            self.context_off.insert(name);
                        } else {
                            self.waiting_tools.push(tool);
                        }
                    }
                    self.sync_blocked();
                    // Under the banner the line says where the server stands
                    // now, switched-off tools counted; later it says what
                    // happened.
                    let text = if self.banner_shown() {
                        self.mcp_tools_line(&source)
                    } else {
                        text
                    };
                    self.mcp_notice(&source, text, NoticeKind::Info);
                }
                LateTools::Failed { source, error } => {
                    self.mcp_reconnecting.remove(&source);
                    let text = t.agent_notice_mcp_error_fmt(&source, &error);
                    self.mcp_notice(&source, text, NoticeKind::Warn);
                }
                LateTools::Gone { source } => {
                    self.mcp_reconnecting.remove(&source);
                    let text = t.agent_notice_mcp_gone_fmt(&source);
                    self.mcp_notice(&source, text, NoticeKind::Info);
                }
                LateTools::NeedsLogin { source } => {
                    self.mcp_reconnecting.remove(&source);
                    let text = t.agent_notice_mcp_needs_login_fmt(&source);
                    self.mcp_notice(&source, text, NoticeKind::Warn);
                }
                LateTools::LoginStarted { source, url } => {
                    let text = t.agent_notice_mcp_login_started_fmt(&source, &url);
                    self.mcp_notice(&source, text, NoticeKind::Info);
                }
            }
        }
        let pending = !self.waiting_tools.is_empty() || !self.leaving_tools.is_empty();
        if pending && !self.is_busy() {
            let batch = std::mem::take(&mut self.waiting_tools);
            let leaving = std::mem::take(&mut self.leaving_tools);
            let (for_worker, leaving_worker) = (batch.clone(), leaving.clone());
            match self.runtime.update(Box::new(move |agent| {
                for name in &leaving_worker {
                    agent.tools_mut().remove(name);
                }
                for tool in for_worker {
                    agent.tools_mut().insert(tool);
                }
            })) {
                Ok(()) => {
                    for name in &leaving {
                        self.tools.remove(name);
                    }
                    for tool in batch {
                        self.tools.insert(tool);
                    }
                }
                Err(_) => {
                    self.waiting_tools = batch;
                    self.leaving_tools = leaving;
                }
            }
        }
        changed
    }

    /// Take `source`'s MCP tools out of the checklist and the queue, and mark
    /// those the agent already has to leave its registry. `true` when the
    /// server had any.
    fn withdraw_mcp_source(&mut self, source: &str) -> bool {
        let mut names = Vec::new();
        self.mcp_arrived.retain(|(server, tool)| {
            if server == source {
                names.push(tool.name().to_string());
                return false;
            }
            true
        });
        self.waiting_tools
            .retain(|tool| !names.iter().any(|name| name == tool.name()));
        for name in &names {
            self.context_off.remove(name);
            if self.tools.get(name).is_some() && !self.leaving_tools.contains(name) {
                self.leaving_tools.push(name.clone());
            }
        }
        !names.is_empty()
    }
}
