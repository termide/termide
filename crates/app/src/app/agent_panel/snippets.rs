//! Editor and terminal text handed to an agent panel: an editor's or a
//! terminal's "Send to agent", and `@selection` / `@term` in the agent's
//! `@` list. The app remembers the editor, terminal and agent last used, so
//! each finds the panel the user means without naming it.

use termide_core::{AgentSnippet, Panel, SnippetWanted};
use termide_panel_agent::AgentPanel;
use termide_panel_editor::Editor;
use termide_panel_terminal::Terminal;

/// The panels last used of each kind. An editor or a terminal is known by
/// its address, which its box keeps for as long as the panel lives; the
/// kind is checked again on use, so an address taken over by another panel
/// after a close only picks that one, never a wrong type.
#[derive(Default)]
pub(crate) struct SnippetSources {
    editor: Option<usize>,
    terminal: Option<usize>,
    agent: Option<u64>,
}

fn address(panel: &dyn Panel) -> usize {
    panel as *const dyn Panel as *const () as usize
}

impl crate::app::App {
    /// Remember the focused panel as the editor, terminal or agent last used.
    pub(in crate::app) fn note_snippet_source(&mut self) {
        let Some(panel) = self.layout_manager.active_panel() else {
            return;
        };
        let panel: &dyn Panel = &**panel;
        let sources = &mut self.snippet_sources;
        if panel.downcast_ref::<Editor>().is_some() {
            sources.editor = Some(address(panel));
        } else if panel.downcast_ref::<Terminal>().is_some() {
            sources.terminal = Some(address(panel));
        } else if let Some(agent) = panel.downcast_ref::<AgentPanel>() {
            if !agent.is_delegate() {
                sources.agent = Some(agent.panel_id());
            }
        }
    }

    /// Put `snippet` into the prompt of the agent last used, or of a new one
    /// when none is open, and focus it.
    pub(in crate::app) fn send_to_agent(&mut self, snippet: AgentSnippet) -> anyhow::Result<()> {
        let open = |id: Option<u64>| {
            move |panel: &dyn Panel| {
                panel.downcast_ref::<AgentPanel>().is_some_and(|agent| {
                    !agent.is_delegate() && id.is_none_or(|id| agent.panel_id() == id)
                })
            }
        };
        let found = self
            .layout_manager
            .focus_panel_where(open(self.snippet_sources.agent))
            || self.layout_manager.focus_panel_where(open(None));
        if !found {
            self.handle_open_agent()?;
        }
        let Some(agent) = self
            .layout_manager
            .active_panel_mut()
            .and_then(|panel| panel.downcast_mut::<AgentPanel>())
        else {
            // No connection to open one on: the error is already shown.
            return Ok(());
        };
        agent.attach_snippet(snippet);
        self.snippet_sources.agent = Some(agent.panel_id());
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Fetch what `wanted` names for the agent panel `id`: the selection of
    /// the editor last used, or the text of the terminal last used. Without
    /// one, the first open counts; a status message says when there is none
    /// or it holds nothing.
    pub(in crate::app) fn want_snippet(&mut self, id: u64, wanted: SnippetWanted) {
        let snippet = match wanted {
            SnippetWanted::Selection => self.snippet_from(self.snippet_sources.editor, |panel| {
                panel
                    .downcast_ref::<Editor>()
                    .map(|editor| editor.agent_snippet())
            }),
            SnippetWanted::Terminal => self.snippet_from(self.snippet_sources.terminal, |panel| {
                panel
                    .downcast_ref::<Terminal>()
                    .map(|terminal| terminal.agent_snippet())
            }),
        };
        let t = termide_i18n::t();
        let snippet = match (wanted, snippet) {
            (_, Some(Some(snippet))) => snippet,
            (SnippetWanted::Selection, None) => {
                return self.state.set_error(t.agent_snippet_no_editor().into())
            }
            (SnippetWanted::Selection, Some(None)) => {
                return self.state.set_error(t.agent_snippet_no_selection().into())
            }
            (SnippetWanted::Terminal, None) => {
                return self.state.set_error(t.agent_snippet_no_terminal().into())
            }
            (SnippetWanted::Terminal, Some(None)) => {
                return self
                    .state
                    .set_error(t.agent_snippet_empty_terminal().into())
            }
        };
        let agent = self
            .layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.downcast_mut::<AgentPanel>())
            .find(|agent| agent.panel_id() == id);
        if let Some(agent) = agent {
            agent.attach_snippet(snippet);
            self.state.needs_redraw = true;
        }
    }

    /// What `take` gives for the panel at `last` when it is still open and
    /// gives something, else for the first open panel of its kind that has
    /// something, else for the first of its kind: `None` when there is no
    /// panel of the kind at all.
    fn snippet_from(
        &mut self,
        last: Option<usize>,
        take: impl Fn(&dyn Panel) -> Option<Option<AgentSnippet>>,
    ) -> Option<Option<AgentSnippet>> {
        let mut taken: Vec<(usize, Option<AgentSnippet>)> = Vec::new();
        for panel in self.layout_manager.iter_all_panels_mut() {
            let panel: &dyn Panel = &**panel;
            if let Some(snippet) = take(panel) {
                taken.push((address(panel), snippet));
            }
        }
        if let Some(at) = taken
            .iter()
            .position(|(address, snippet)| Some(*address) == last && snippet.is_some())
        {
            return Some(taken.swap_remove(at).1);
        }
        if let Some(at) = taken.iter().position(|(_, snippet)| snippet.is_some()) {
            return Some(taken.swap_remove(at).1);
        }
        taken.into_iter().next().map(|(_, snippet)| snippet)
    }
}
