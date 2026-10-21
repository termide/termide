//! A view of a delegated task that runs out of sight, shown over the layout.
//!
//! The tasks register in `termide_panel_agent::live` while they run; a
//! click on a task's row, or `o` on its block, asks the app for a view of it
//! (`PanelEvent::FocusDelegate`). The view is an agent panel following the
//! task's session log, shown over the layout (`LayoutManager::overlay`) so
//! that it moves no column. Like any modal, it holds the rest of the window
//! while it is up: every key but quitting and detaching goes to it, the
//! mouse reaches nothing outside it, and only `Esc`, the panel-close key or
//! its Close button closes it — or `F9` and its In a panel button, which
//! move it into a panel of its own beside the others. The delegate's cards follow the view:
//! the panel that delegated the task sets them aside while one is open, and
//! gets back those still unanswered when it closes.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};
use termide_core::{CommandResult, PanelCommand, PanelEvent};
use termide_panel_agent::{AgentPanel, Card};

/// A delegated task carried on out of its own log: the project and the
/// panel that delegated it, and the channel its permission requests come
/// back on.
struct Carried {
    project_root: PathBuf,
    parent: u64,
    rx: std::sync::mpsc::Receiver<termide_agent_core::PermissionEnvelope>,
}

/// The delegated tasks carried on out of their own logs. A carried task has
/// no panel of its own, so its cards go to the panel that delegated it —
/// in its project, parked or not; they wait there until its `tick` takes
/// them.
static CARRIED: std::sync::Mutex<Vec<Carried>> = std::sync::Mutex::new(Vec::new());

/// Where a carried delegate's cards wait: the panel `parent` of the project
/// at `project_root`, on `rx`.
pub(in crate::app) fn carry_cards(
    project_root: PathBuf,
    parent: u64,
    rx: std::sync::mpsc::Receiver<termide_agent_core::PermissionEnvelope>,
) {
    CARRIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Carried {
            project_root,
            parent,
            rx,
        });
}

/// Hand the cards the carried delegates have asked for to the panels that
/// delegated them; a run that has ended leaves its channel behind.
pub(in crate::app) fn route_carried(app: &mut App) {
    let mut held = CARRIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut gone = Vec::new();
    let mut out: Vec<(PathBuf, u64, Vec<Card>)> = Vec::new();
    for (index, carried) in held.iter_mut().enumerate() {
        let rx = &carried.rx;
        let mut cards = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(envelope) => cards.push(Card::Permission(Box::new(envelope))),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    gone.push(index);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        if !cards.is_empty() {
            out.push((carried.project_root.clone(), carried.parent, cards));
        }
    }
    for index in gone.into_iter().rev() {
        held.remove(index);
    }
    drop(held);
    for (root, parent, cards) in out {
        if let Err(error) = app.in_project(&root, |app| app.give_cards(parent, cards)) {
            log::warn!("a carried-on delegate's cards found no panel: {error}");
        }
    }
}

use super::{agent_setup, DEFAULT_AGENT};
use crate::app::App;

impl App {
    /// Show a view of the task the `task` call `call` of panel `panel`
    /// handed over, while it runs out of sight, over the layout. `false`
    /// when no such task runs.
    pub(in crate::app) fn watch_delegate(&mut self, panel: u64, call: &str) -> bool {
        if self
            .overlay_agent()
            .is_some_and(|view| view.runs_task(panel, call))
        {
            return true;
        }
        let Some(watch) = termide_panel_agent::live::watch(panel, call) else {
            return false;
        };
        // One view at a time: another one gives its cards back first.
        self.close_overlay();
        // On the default agent until the task is over: the view only reads
        // the log, and the delegate's own agent — an external one would
        // start a process of its own — comes with its session once it opens.
        let settings = self.project_ai_settings();
        let setup = agent_setup(
            &settings,
            self.project_root.clone(),
            &self.project_root,
            DEFAULT_AGENT,
            None,
        );
        let mut view = AgentPanel::new(setup);
        view.set_in_overlay(true);
        view.start_watch(watch);
        self.layout_manager.overlay = Some(Box::new(view));
        self.state.needs_redraw = true;
        true
    }

    /// The agent panel shown over the layout, if one is.
    fn overlay_agent(&mut self) -> Option<&mut AgentPanel> {
        self.layout_manager
            .overlay
            .as_mut()
            .and_then(|panel| panel.as_any_mut().downcast_mut::<AgentPanel>())
    }

    /// Close the view shown over the layout: the delegate's cards it holds go
    /// back to the panel that delegated the task; the task goes on.
    pub(in crate::app) fn close_overlay(&mut self) {
        let Some(mut panel) = self.layout_manager.overlay.take() else {
            return;
        };
        if let Some(view) = panel.as_any_mut().downcast_mut::<AgentPanel>() {
            let parent = view.delegated_by();
            let cards = view.release_cards();
            // The view goes first, so the task's cards are no longer set
            // aside for it.
            drop(panel);
            if let Some(parent) = parent {
                self.give_cards(parent, cards);
            }
        }
        self.state.needs_redraw = true;
    }

    /// Move the view shown over the layout into a panel of its own beside
    /// the others, in focus; its cards stay with it.
    pub(in crate::app) fn dock_overlay(&mut self) {
        let Some(mut panel) = self.layout_manager.overlay.take() else {
            return;
        };
        if let Some(view) = panel.as_any_mut().downcast_mut::<AgentPanel>() {
            view.set_in_overlay(false);
        }
        self.add_panel(panel);
        self.auto_save_layout();
        self.state.needs_redraw = true;
    }

    /// Hand `cards` to the panel `parent`; one gone keeps none.
    pub(in crate::app) fn give_cards(&mut self, parent: u64, cards: Vec<Card>) {
        let Some(target) = self.layout_manager.iter_all_panels_mut().find_map(|panel| {
            panel
                .as_any_mut()
                .downcast_mut::<AgentPanel>()
                .filter(|agent| agent.panel_id() == parent)
        }) else {
            return;
        };
        for card in cards {
            target.give_card(card);
        }
    }

    /// Move the delegates' cards the panels set aside to the views following
    /// their tasks — over the layout or in a panel — or back to the panel
    /// that set them aside when the view has gone meanwhile.
    pub(in crate::app) fn route_delegate_cards(&mut self) {
        let mut moving = Vec::new();
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(agent) = panel.as_any_mut().downcast_mut::<AgentPanel>() {
                let parent = agent.panel_id();
                moving.extend(
                    agent
                        .take_cards_out()
                        .into_iter()
                        .map(|(call, card)| (parent, call, card)),
                );
            }
        }
        for (parent, call, card) in moving {
            let view = match self.overlay_agent() {
                Some(view) if view.runs_task(parent, &call) => Some(view),
                _ => self.layout_manager.iter_all_panels_mut().find_map(|panel| {
                    panel
                        .as_any_mut()
                        .downcast_mut::<AgentPanel>()
                        .filter(|agent| agent.runs_task(parent, &call))
                }),
            };
            match view {
                Some(view) => view.give_card(card),
                None => self.give_cards(parent, vec![card]),
            }
            self.state.needs_redraw = true;
        }
    }

    /// A key while a view is shown over the layout: quitting and detaching
    /// work as ever, closing a panel closes the view, the clipboard keys and
    /// every other key go to it. `false` when no view is up.
    pub(in crate::app) fn overlay_key(&mut self, key: KeyEvent) -> Result<bool> {
        if self.layout_manager.overlay.is_none() {
            return Ok(false);
        }
        self.ensure_hotkey_table();
        let (quit, close, clipboard) = {
            let table = self
                .state
                .cache
                .hotkey_table
                .as_ref()
                .expect("hotkey table built");
            let clipboard = if table.matches("copy", &key) {
                Some(PanelCommand::Copy)
            } else if table.matches("cut", &key) {
                Some(PanelCommand::Cut)
            } else if table.matches("paste", &key) {
                Some(PanelCommand::Paste)
            } else {
                None
            };
            (
                table.matches("quit", &key) || table.matches("detach_instance", &key),
                table.matches("close_panel", &key),
                clipboard,
            )
        };
        if quit {
            return Ok(false);
        }
        if close {
            self.close_overlay();
            return Ok(true);
        }
        let chord = termide_core::KeyChord::new(key, &self.normalizer);
        let Some(panel) = self.layout_manager.overlay.as_mut() else {
            return Ok(false);
        };
        let events = match clipboard {
            Some(command) => {
                if let CommandResult::Handled(redraw) = panel.handle_command(command) {
                    self.state.needs_redraw |= redraw;
                }
                Vec::new()
            }
            None => panel.handle_key(chord),
        };
        self.overlay_events(events)?;
        self.state.needs_redraw = true;
        Ok(true)
    }

    /// A mouse event while a view is shown over the layout: inside it, the
    /// view's; outside it, nothing's — the view holds the window as a modal
    /// does. `false` when no view is up.
    pub(in crate::app) fn overlay_mouse(&mut self, event: MouseEvent) -> Result<bool> {
        let main = self.main_area();
        let Some(panel) = self.layout_manager.overlay.as_mut() else {
            return Ok(false);
        };
        let area = termide_layout::overlay_area(main);
        let inside = event.column >= area.x
            && event.column < area.x + area.width
            && event.row >= area.y
            && event.row < area.y + area.height;
        if !inside {
            return Ok(true);
        }
        let events = panel.handle_mouse(event, area);
        self.overlay_events(events)?;
        self.state.needs_redraw = true;
        Ok(true)
    }

    /// The wheel while a view is shown over the layout: inside it, the
    /// view's; outside, nothing moves. `false` when no view is up.
    pub(in crate::app) fn overlay_scroll(&mut self, mouse: MouseEvent, delta: i32) -> Result<bool> {
        let main = self.main_area();
        let Some(panel) = self.layout_manager.overlay.as_mut() else {
            return Ok(false);
        };
        let area = termide_layout::overlay_area(main);
        let inside = mouse.column >= area.x
            && mouse.column < area.x + area.width
            && mouse.row >= area.y
            && mouse.row < area.y + area.height;
        if inside {
            let events = panel.handle_scroll(delta, area);
            self.overlay_events(events)?;
            self.state.needs_redraw = true;
        }
        Ok(true)
    }

    /// The area the layout fills, under the menu bar and over the status
    /// bar, where a view over the layout goes.
    fn main_area(&self) -> ratatui::layout::Rect {
        ratatui::layout::Rect {
            x: 0,
            y: 1,
            width: self.state.terminal.width,
            height: self.panel_area_height(),
        }
    }

    /// Tick the view shown over the layout.
    pub(in crate::app) fn tick_overlay(&mut self) -> Result<()> {
        let Some(panel) = self.layout_manager.overlay.as_mut() else {
            return Ok(());
        };
        let events = panel.tick();
        if !events.is_empty() {
            self.state.needs_redraw = true;
            self.overlay_events(events)?;
        }
        Ok(())
    }

    /// What the view shown over the layout asked for: going back to the
    /// panel that delegated the task closes it, moving it into a panel docks
    /// it; the rest as any panel's.
    fn overlay_events(&mut self, events: Vec<PanelEvent>) -> Result<()> {
        let mut rest = Vec::new();
        for event in events {
            match event {
                PanelEvent::DockOverlay => self.dock_overlay(),
                PanelEvent::FocusAgentPanel(_) => {
                    self.close_overlay();
                    rest.push(event);
                }
                event => rest.push(event),
            }
        }
        if rest.is_empty() {
            return Ok(());
        }
        self.process_panel_events(rest)
    }
}
