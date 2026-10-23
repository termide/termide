//! Projects open in the background.
//!
//! Switching projects parks the panels of the one left instead of dropping
//! them. Terminals keep running (their output is read on threads of their
//! own), editors keep unsaved edits, and agents keep working. Parked panels
//! are still ticked, but the events they raise wait for their project to be
//! current again: acted on now they would land in another project's layout.
//! Only a request for attention acts at once, as a bell.

use std::path::{Path, PathBuf};

use anyhow::Result;
use termide_core::{Panel, PanelCommand, PanelEvent};
use termide_i18n as i18n;
use termide_layout::LayoutManager;

use super::project_layout::save_layout_of;
use super::App;
use crate::open_projects::OpenProjectView;
use crate::state::{ActiveModal, PendingAction, ProjectsOrigin};
use crate::PanelExt;

/// Where to move an open project in the list, with the keys that move a
/// panel between groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectStep {
    /// One place up, or left on the menu bar (`swap_left`).
    Back,
    /// One place down, or right (`swap_right`).
    Forward,
    /// To the start (`move_first`).
    First,
    /// To the end (`move_last`).
    Last,
}

/// What a project open in the background keeps.
pub(super) struct ParkedProject {
    layout: LayoutManager,
    /// Events its panels raised while parked, handled once it is current.
    events: Vec<PanelEvent>,
}

impl ParkedProject {
    /// Every panel of the parked project.
    pub(super) fn panels_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Panel>> {
        self.layout.iter_all_panels_mut()
    }

    fn panels(&self) -> impl Iterator<Item = &Box<dyn Panel>> {
        self.layout
            .panel_groups
            .iter()
            .flat_map(|group| group.panels().iter())
    }

    /// Whether a panel waits for the user.
    fn needs_attention(&self) -> bool {
        self.panels().any(|panel| panel.needs_attention())
    }

    /// Whether closing it would stop running processes or drop unsaved
    /// changes.
    fn requires_confirmation(&self) -> bool {
        self.panels()
            .any(|panel| panel.needs_close_confirmation().is_some())
    }
}

/// Queue `event`, raised by a panel of a parked project, for when the project
/// is current again. A redraw request means nothing off screen — a busy
/// agent raises one ten times a second — and one directory change per queue
/// says all that the next does.
fn queue_parked_event(events: &mut Vec<PanelEvent>, event: PanelEvent) {
    match event {
        PanelEvent::NeedsRedraw => {}
        PanelEvent::WorkingDirectoryChanged
            if events
                .iter()
                .any(|queued| matches!(queued, PanelEvent::WorkingDirectoryChanged)) => {}
        event => events.push(event),
    }
}

/// The root a project is known by: the directory with symlinks resolved, so
/// one project reached by two paths is opened once.
pub(super) fn project_root_of(path: PathBuf) -> PathBuf {
    dunce::canonicalize(&path).unwrap_or(path)
}

/// Whether an agent panel in the saved layout of the project at `root` left
/// work waiting on the user or on a clock: a run cut off or paused, a goal or
/// loop going, a failed request waiting to be tried again — what the session
/// lists mark `⏸` (see `SessionSummary::unfinished`).
fn holds_agent_work(root: &Path) -> bool {
    let Ok(layout) = termide_project::ProjectLayout::load(root) else {
        return false;
    };
    layout
        .panel_groups
        .iter()
        .flat_map(|group| &group.panels)
        .filter_map(|panel| match panel {
            termide_project::PanelState::Agent {
                session: Some(path),
                ..
            } => Some(path),
            _ => None,
        })
        .any(|path| {
            termide_agent_core::session::Session::open(path).is_ok_and(|session| {
                termide_agent_core::session::SessionSummary::from(&session).unfinished
            })
        })
}

/// `root` as the menus and messages name it.
fn display_root(root: &Path) -> String {
    termide_core::util::shorten_home_path(&root.display().to_string())
}

/// What entering a project hands back to finish the switch with.
pub(super) struct EnteredProject {
    /// What the project kept when it was open already (`None` when it opens
    /// now and its layout has yet to be set up).
    pub restored: Option<ParkedProject>,
    /// The operations panel of the project left, for
    /// [`App::carry_operations_panel`].
    pub operations_panel: Option<Box<dyn Panel>>,
}

impl App {
    /// Make `root` the current project: park the current one and hand back
    /// what `root` kept and what follows the user there.
    pub(super) fn enter_project(&mut self, root: PathBuf) -> Result<EnteredProject> {
        std::env::set_current_dir(&root)?;
        if !self.open_projects.is_open(&root) {
            // A project opened by hand starts a new set: the last run's is
            // no longer offered.
            self.state.reopenable_projects.clear();
        }
        log::info!("Changed working directory to: {:?}", root);

        // Operations run for the instance, not for a project, so their panel
        // leaves with the user: one left in a parked layout would keep cards
        // of operations that end while it is off screen.
        let operations_panel = self.close_operations_panel();
        self.auto_save_layout();
        // Parked panels skip background work and catch up once shown again.
        for panel in self.layout_manager.iter_all_panels_mut() {
            panel.handle_command(PanelCommand::MarkStale);
        }
        let leaving = ParkedProject {
            layout: std::mem::replace(&mut self.layout_manager, LayoutManager::new()),
            events: Vec::new(),
        };
        let restored = self.open_projects.switch(root.clone(), leaving);
        self.set_project_root(root);
        Ok(EnteredProject {
            restored,
            operations_panel,
        })
    }

    /// Put the operations panel taken from the project left into the layout
    /// of the one entered, without taking the focus.
    pub(super) fn carry_operations_panel(&mut self, panel: Option<Box<dyn Panel>>) {
        if let Some(panel) = panel {
            self.add_panel_without_focus(panel);
            self.state.operations_panel_dirty = true;
        }
    }

    /// Point everything that depends on the project root at `root`.
    pub(super) fn set_project_root(&mut self, root: PathBuf) {
        self.project_root = root;
        self.state.project_root = self.project_root.clone();
        self.state.project_bookmarks =
            termide_config::BookmarksConfig::load_from_project(&self.project_root);

        // The settings of the project left must not carry over: the one
        // entered gets the global layer with its own file, if it has one,
        // on top. Otherwise the next settings save would write the old
        // project's overrides into the new project or the global file.
        match termide_config::Config::with_project_overlay(
            &self.state.global_baseline,
            &self.project_root,
        ) {
            Ok((config, warnings)) => {
                for warning in warnings {
                    log::warn!("{warning}");
                }
                self.adopt_stored_config(config);
            }
            Err(e) => log::warn!("Could not load the project's settings: {e}"),
        }

        // Invalidate caches that depend on the project root: project-local
        // commands.toml lives under `<project_root>/.termide/`, so both the
        // commands registry and the global hotkey table (which folds
        // command hotkeys in) must rebuild for the new project.
        self.state.cache.commands_registry = None;
        self.state.cache.hotkey_table = None;
    }

    /// Put a parked project's panels back on screen and handle what they
    /// raised meanwhile.
    pub(super) fn restore_parked_project(&mut self, parked: ParkedProject) {
        self.layout_manager = parked.layout;
        self.layout_manager
            .redistribute_widths_proportionally(self.state.terminal.width);
        // The project was likely entered for what waits in it; a mark on an
        // unfocused header is easy to miss.
        self.layout_manager.focus_waiting_panel();
        // Diagnostics published while the project was parked reached only
        // `all_diagnostics`.
        for panel in self.layout_manager.iter_all_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                let fresh = editor
                    .file_path()
                    .and_then(|path| self.state.all_diagnostics.get(path))
                    .cloned();
                if let Some(diagnostics) = fresh {
                    editor.update_diagnostics(diagnostics);
                }
            }
        }
        // The outline may have missed edits and focus moves while parked.
        self.resync_outline();
        self.state.needs_watcher_registration = true;
        self.state.needs_redraw = true;
        if !parked.events.is_empty() {
            if let Err(e) = self.process_panel_events(parked.events) {
                log::error!("Error processing events of a parked project: {}", e);
            }
        }
    }

    /// Bring `state.open_projects`, which the menus draw, in step with the
    /// open projects, and save the set for reopening when it changed.
    pub(super) fn sync_open_projects(&mut self) {
        self.save_open_projects();
        self.plugins.mark_dirty();
        let views: Vec<OpenProjectView> =
            self.open_projects
                .roots()
                .map(|root| OpenProjectView {
                    root: root.to_path_buf(),
                    attention: self.open_projects.parked().any(|(parked_root, parked)| {
                        parked_root == root && parked.needs_attention()
                    }),
                })
                .collect();
        if views != self.state.open_projects {
            self.state.open_projects = views;
            self.state.needs_redraw = true;
        }
    }

    /// Save the open projects and the current one for the next run to
    /// reopen, once more than one has been open and whenever either changes
    /// since. The current project goes along, so that `termide --restore`
    /// reopens the run in the project it was in.
    fn save_open_projects(&mut self) {
        if !self.persist_layout {
            return;
        }
        let projects = termide_project::SavedOpenProjects {
            roots: self.open_projects.roots().map(Path::to_path_buf).collect(),
            current: Some(self.project_root.clone()),
        };
        let unchanged = match &self.saved_open_projects {
            Some(saved) => *saved == projects,
            None => projects.roots.len() < 2,
        };
        if unchanged {
            return;
        }
        if let Err(e) = termide_project::save_open_projects(&projects) {
            log::error!("Failed to save the open projects: {}", e);
        }
        self.saved_open_projects = Some(projects);
    }

    /// Read the projects open together in the last run, those that still
    /// exist and are not open now, for the Projects menu to offer.
    pub(super) fn load_reopenable_projects(&mut self) {
        if !self.persist_layout {
            return;
        }
        let mut roots: Vec<PathBuf> = Vec::new();
        for root in termide_project::load_open_projects().roots {
            let root = project_root_of(root);
            if root.is_dir() && !self.open_projects.is_open(&root) && !roots.contains(&root) {
                roots.push(root);
            }
        }
        self.state.reopenable_projects = roots;
    }

    /// Open the projects of the last run in the background. Their layouts
    /// load the first time each is entered; the current project stays.
    pub(super) fn reopen_previous_projects(&mut self) {
        let roots = std::mem::take(&mut self.state.reopenable_projects);
        let mut opened = 0;
        for root in roots {
            if self.open_projects.open_pending(root) {
                opened += 1;
            }
        }
        // In the order they were kept, the current project among them.
        let order: Vec<PathBuf> = termide_project::load_open_projects()
            .roots
            .into_iter()
            .map(project_root_of)
            .collect();
        self.open_projects.arrange(&order);
        log::info!("Reopened {} projects of the last run", opened);
        self.load_projects_holding_agent_work();
        self.sync_open_projects();
    }

    /// Load in the background the reopened projects whose agent left work
    /// unfinished: a failed request waiting to be tried again — on a wait, or
    /// on a card that tries again by itself — or a run, goal or loop that asks
    /// whether to carry on. A project not loaded has no panels to tick, so
    /// the try would wait for the user to enter it, and the card would ring
    /// no bell and mark no project; loaded and parked, it goes like in any
    /// project left this run.
    fn load_projects_holding_agent_work(&mut self) {
        let home = self.project_root.clone();
        let waiting: Vec<PathBuf> = self
            .open_projects
            .roots()
            .filter(|root| self.open_projects.is_pending(root) && holds_agent_work(root))
            .map(Path::to_path_buf)
            .collect();
        for root in waiting {
            // Entered and left at once, through the switch the user makes,
            // so the project loads as it would on entering it. The visit
            // does not count as one the user made.
            let recency = [&root, &home].map(|root| self.open_projects.last_used(root));
            if let Err(e) = self.switch_to_project(root.clone()) {
                log::warn!("Could not load {} in the background: {e}", root.display());
                continue;
            }
            if let Err(e) = self.switch_to_project(home.clone()) {
                log::error!("Could not return to {}: {e}", home.display());
                return;
            }
            for (root, last_used) in [&root, &home].into_iter().zip(recency) {
                if let Some(last_used) = last_used {
                    self.open_projects.set_last_used(root, last_used);
                }
            }
            log::info!(
                "Loaded {} in the background: an agent in it left work unfinished",
                root.display()
            );
        }
    }

    /// Tick the panels of the parked projects. What they raise waits for
    /// their project; a request for attention rings the bell at once.
    pub(super) fn tick_parked_projects(&mut self) {
        if self.open_projects.count() < 2 {
            return;
        }
        let mut ring = false;
        let mut attention_changed = false;
        let mut agent_runs = Vec::new();
        for (root, parked) in self.open_projects.parked_mut() {
            for panel in parked.layout.iter_all_panels_mut() {
                for event in panel.tick() {
                    match event {
                        PanelEvent::RequestAttention => ring = true,
                        // The project's plugins hear it now, as they would
                        // with the project in front, not when it comes back.
                        PanelEvent::AgentRun { panel, finished } => {
                            agent_runs.push((root.to_path_buf(), panel, finished));
                        }
                        event => queue_parked_event(&mut parked.events, event),
                    }
                }
            }
            // Rebuilt below only when a mark changes: this runs every tick.
            let shown = self
                .state
                .open_projects
                .iter()
                .find(|view| view.root == root)
                .is_some_and(|view| view.attention);
            attention_changed |= shown != parked.needs_attention();
        }
        for (root, panel, finished) in agent_runs {
            let (method, params) = super::plugins::agent_run_event(panel, finished);
            self.plugins.host.emit(&root, method, &params);
        }
        if ring {
            self.state.attention_bell();
        }
        if attention_changed {
            self.sync_open_projects();
        }
    }

    /// Run `act` on the project at `root` as if it were current: on screen
    /// as it is, parked with its layout and root standing in for the
    /// current ones until `act` returns. So an agent of a parked project
    /// reaches its own panels — takes one for a task, messages one — and
    /// what `act` adds shows when the user enters the project. Why not, when
    /// `root` is not loaded: a project not entered yet runs no agents.
    pub(super) fn in_project<R>(
        &mut self,
        root: &Path,
        act: impl FnOnce(&mut App) -> R,
    ) -> Result<R, String> {
        if super::agent_panel::same_dir(root, &self.project_root) {
            return Ok(act(self));
        }
        let Some((parked_root, parked)) = self
            .open_projects
            .parked_mut()
            .find(|(parked_root, _)| super::agent_panel::same_dir(parked_root, root))
        else {
            return Err(format!(
                "{} is not open in this window; its panels are out of reach",
                display_root(root)
            ));
        };
        let parked_root = parked_root.to_path_buf();
        let layout = std::mem::replace(&mut parked.layout, LayoutManager::new());
        let home_layout = std::mem::replace(&mut self.layout_manager, layout);
        let home_root = std::mem::replace(&mut self.project_root, parked_root.clone());
        let home_state_root = std::mem::replace(&mut self.state.project_root, parked_root.clone());
        let result = act(self);
        let layout = std::mem::replace(&mut self.layout_manager, home_layout);
        self.project_root = home_root;
        self.state.project_root = home_state_root;
        if let Some((_, parked)) = self
            .open_projects
            .parked_mut()
            .find(|(root, _)| *root == parked_root)
        {
            parked.layout = layout;
        }
        Ok(result)
    }

    /// The agent settings of the project [`Self::in_project`] works on: the
    /// current ones on screen; for a parked project, its own as entering it
    /// would load them — the global layer with the project's file on top —
    /// so a panel set up there runs as one the user opens there. Read from
    /// disk, so only when a panel is set up.
    pub(super) fn project_ai_settings(&self) -> termide_config::AiSettings {
        if self.project_root == self.open_projects.current() {
            return self.state.config.ai.clone();
        }
        match termide_config::Config::with_project_overlay(
            &self.state.global_baseline,
            &self.project_root,
        ) {
            Ok((config, _)) => config.ai,
            Err(e) => {
                log::warn!(
                    "Could not load the settings of {}: {e}; using the global ones",
                    display_root(&self.project_root)
                );
                self.state.global_baseline.ai.clone()
            }
        }
    }

    /// The files the parked projects' editors wrote since the last call,
    /// with the root of the project each belongs to.
    pub(super) fn take_parked_saved_paths(&mut self) -> Vec<(PathBuf, PathBuf)> {
        let mut saved = Vec::new();
        for (root, parked) in self.open_projects.parked_mut() {
            for panel in parked.layout.iter_all_panels_mut() {
                if let Some(editor) = panel.as_editor_mut() {
                    saved.extend(
                        editor
                            .take_saved_paths()
                            .into_iter()
                            .map(|path| (root.to_path_buf(), path)),
                    );
                }
            }
        }
        saved
    }

    /// The editor of `path` in the project at `project` — the current one or
    /// a parked one — if it is open there.
    pub(super) fn project_editor_mut(
        &mut self,
        project: &Path,
        path: &Path,
    ) -> Option<&mut termide_panel_editor::Editor> {
        let layout = if project == self.open_projects.current() {
            &mut self.layout_manager
        } else {
            &mut self
                .open_projects
                .parked_mut()
                .find(|(root, _)| *root == project)?
                .1
                .layout
        };
        layout
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_editor_mut())
            .find(|editor| editor.file_path() == Some(path))
    }

    /// The plugin panel `id`, in the current project or a parked one.
    pub(super) fn plugin_panel_mut(
        &mut self,
        id: u64,
    ) -> Option<&mut termide_panel_markdown::MarkdownPanel> {
        let parked = self
            .open_projects
            .parked_mut()
            .flat_map(|(_, parked)| parked.layout.iter_all_panels_mut());
        self.layout_manager
            .iter_all_panels_mut()
            .chain(parked)
            .filter_map(|panel| {
                panel
                    .as_any_mut()
                    .downcast_mut::<termide_panel_markdown::MarkdownPanel>()
            })
            .find(|panel| panel.plugin_panel_id() == Some(id))
    }

    /// The ids of every plugin panel open, in any project.
    pub(super) fn plugin_panel_ids(&mut self) -> Vec<u64> {
        let parked = self
            .open_projects
            .parked_mut()
            .flat_map(|(_, parked)| parked.layout.iter_all_panels_mut());
        self.layout_manager
            .iter_all_panels_mut()
            .chain(parked)
            .filter_map(|panel| {
                panel
                    .as_any()
                    .downcast_ref::<termide_panel_markdown::MarkdownPanel>()
                    .and_then(termide_panel_markdown::MarkdownPanel::plugin_panel_id)
            })
            .collect()
    }

    /// The roots of the parked projects.
    pub(super) fn parked_roots(&self) -> Vec<PathBuf> {
        self.open_projects
            .parked()
            .map(|(root, _)| root.to_path_buf())
            .collect()
    }

    /// The open projects' roots in the order the menus list them.
    pub(super) fn listed_open_roots(&self) -> Vec<PathBuf> {
        self.open_projects.roots().map(Path::to_path_buf).collect()
    }

    /// Move the open project at `root` as `step` says. Returns its new
    /// place, or `None` when it did not move.
    pub(super) fn move_open_project(&mut self, root: &Path, step: ProjectStep) -> Option<usize> {
        let from = self.open_projects.position(root)?;
        let to = match step {
            ProjectStep::Back => from.checked_sub(1)?,
            ProjectStep::Forward => from + 1,
            ProjectStep::First => 0,
            ProjectStep::Last => usize::MAX,
        };
        if !self.open_projects.move_to(root, to) {
            return None;
        }
        self.sync_open_projects();
        self.open_projects.position(root)
    }

    /// Switch to the open project at `index` in the order the menus list
    /// them.
    pub(super) fn switch_to_open_project(&mut self, index: usize) -> Result<()> {
        let Some(root) = self.listed_open_roots().into_iter().nth(index) else {
            return Ok(());
        };
        self.switch_to_project(root)
    }

    /// Switch to the open project after (or before) the current one, in the
    /// order the menus list them, wrapping around.
    pub(super) fn cycle_open_projects(&mut self, forward: bool) -> Result<()> {
        let count = self.open_projects.count();
        if count < 2 {
            return Ok(());
        }
        let current = self
            .listed_open_roots()
            .iter()
            .position(|root| *root == self.project_root)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.switch_to_open_project(next)
    }

    /// The open project to switch to when the current one closes: the one
    /// left last.
    fn successor_project(&self) -> Option<PathBuf> {
        self.open_projects
            .by_recent_use()
            .get(1)
            .map(|root| root.to_path_buf())
    }

    /// Ask before closing the open project at `root`, then return `from`
    /// where it was asked.
    ///
    /// The current project closes by switching to the one left last, and
    /// only while another is open; it is asked about only when its panels
    /// hold running processes or unsaved changes.
    pub(super) fn confirm_close_project(&mut self, root: PathBuf, from: ProjectsOrigin) {
        if !self.open_projects.is_open(&root) {
            return;
        }
        if root == self.project_root {
            if self.successor_project().is_none() {
                return;
            }
            if !self.has_panels_requiring_confirmation() {
                if let Err(e) = self.close_project(&root, from) {
                    log::error!("Failed to close the current project: {}", e);
                }
                return;
            }
            let t = i18n::t();
            let message = format!("{}\n{}", display_root(&root), t.projects_close_warning());
            let modal = termide_modal::ConfirmModal::new(t.projects_close_title(), message)
                .defaulting_to_no();
            self.state.set_pending_action(
                PendingAction::CloseProject { root, from },
                ActiveModal::Confirm(Box::new(modal)),
            );
            return;
        }
        // A project reopened but not entered yet has nothing running.
        let live = self
            .open_projects
            .parked()
            .any(|(parked_root, parked)| parked_root == root && parked.requires_confirmation());
        let t = i18n::t();
        let mut message = display_root(&root);
        if live {
            message.push('\n');
            message.push_str(t.projects_close_warning());
        }
        let mut modal = termide_modal::ConfirmModal::new(t.projects_close_title(), message);
        if live {
            modal = modal.defaulting_to_no();
        }
        self.state.set_pending_action(
            PendingAction::CloseProject { root, from },
            ActiveModal::Confirm(Box::new(modal)),
        );
    }

    /// Close the open project at `root`: save its layout and drop its
    /// panels, which stops the processes in its terminals. The current
    /// project is first left for the one left last, and stays when no other
    /// is open.
    pub(super) fn close_project(&mut self, root: &Path, from: ProjectsOrigin) -> Result<()> {
        if root == self.project_root {
            let Some(successor) = self.successor_project() else {
                return self.return_to_projects(from);
            };
            self.switch_to_project(successor)?;
        }
        if let Some(mut parked) = self.open_projects.close(root) {
            // Its agent panels go with it, and the tasks they handed over
            // stop with them, as when a panel is closed.
            for panel in parked.layout.iter_all_panels_mut() {
                if let Some(agent) = panel
                    .as_any()
                    .downcast_ref::<termide_panel_agent::AgentPanel>()
                {
                    termide_panel_agent::live::stop_all(agent.panel_id());
                    termide_panel_agent::live::clear_parked_of(agent.panel_id());
                }
            }
            // As closing an editor panel does: the server forgets the file.
            if let Some(lsp_manager) = self.state.lsp_manager.as_ref() {
                for panel in parked.layout.iter_all_panels_mut() {
                    if let Some(editor) = panel.as_editor_mut() {
                        editor.cleanup_lsp(lsp_manager);
                    }
                }
            }
            if self.persist_layout {
                if let Err(e) = save_layout_of(root, &mut parked.layout) {
                    log::error!("Failed to save the layout of {:?}: {}", root, e);
                }
            }
            log::info!("Closed project {:?}", root);
        }
        self.sync_open_projects();
        self.return_to_projects(from)
    }

    /// Go back to where closing or deleting a project was started.
    pub(super) fn return_to_projects(&mut self, from: ProjectsOrigin) -> Result<()> {
        match from {
            ProjectsOrigin::Switcher => self.handle_open_projects_modal()?,
            ProjectsOrigin::Menu(selection) => self.reopen_projects_menu(selection),
            ProjectsOrigin::Button(button) => self.reopen_menu_bar_at_button(button),
        }
        Ok(())
    }

    /// What to ask before quitting, if anything: the current project's
    /// panels hold running processes or unsaved changes, or a parked
    /// project's do.
    pub(super) fn quit_confirmation(&self) -> Option<String> {
        let background: Vec<String> = self
            .open_projects
            .parked()
            .filter(|(_, parked)| parked.requires_confirmation())
            .map(|(root, _)| display_root(root))
            .collect();
        if background.is_empty() && !self.has_panels_requiring_confirmation() {
            return None;
        }
        let t = i18n::t();
        let mut message = t.app_quit_confirm().to_string();
        if !background.is_empty() {
            message.push('\n');
            message.push_str(&t.app_quit_background_fmt(&background.join(", ")));
        }
        Some(message)
    }

    /// Save the layouts of the parked projects, as quitting leaves them.
    pub(super) fn save_parked_layouts(&mut self) {
        if !self.persist_layout {
            return;
        }
        for (root, parked) in self.open_projects.parked_mut() {
            if let Err(e) = save_layout_of(root, &mut parked.layout) {
                log::error!("Failed to save the layout of {:?}: {}", root, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parked_queue_drops_redraws_and_keeps_one_directory_change() {
        let mut events = Vec::new();
        for _ in 0..100 {
            queue_parked_event(&mut events, PanelEvent::NeedsRedraw);
            queue_parked_event(&mut events, PanelEvent::WorkingDirectoryChanged);
        }
        queue_parked_event(&mut events, PanelEvent::ShowError("failed".into()));
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], PanelEvent::WorkingDirectoryChanged));
        assert!(matches!(events[1], PanelEvent::ShowError(_)));
    }

    #[cfg(unix)]
    #[test]
    fn a_project_reached_through_a_symlink_has_the_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(project_root_of(link), project_root_of(real));
    }
}
