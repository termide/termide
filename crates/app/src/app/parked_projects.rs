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
use crate::projects_menu::path_order;
use crate::state::{ActiveModal, PendingAction};

/// What a project open in the background keeps.
pub(super) struct ParkedProject {
    layout: LayoutManager,
    /// Events its panels raised while parked, handled once it is current.
    events: Vec<PanelEvent>,
}

impl ParkedProject {
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

/// `root` as the menus and messages name it.
fn display_root(root: &Path) -> String {
    termide_core::util::shorten_home_path(&root.display().to_string())
}

impl App {
    /// Make `root` the current project: park the current one and hand back
    /// what `root` kept when it was open already (`None` when it opens now
    /// and its layout has yet to be set up).
    pub(super) fn enter_project(&mut self, root: PathBuf) -> Result<Option<ParkedProject>> {
        std::env::set_current_dir(&root)?;
        log::info!("Changed working directory to: {:?}", root);

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
        Ok(restored)
    }

    /// Point everything that depends on the project root at `root`.
    pub(super) fn set_project_root(&mut self, root: PathBuf) {
        self.project_root = root;
        self.state.project_root = self.project_root.clone();
        self.state.project_bookmarks =
            termide_config::BookmarksConfig::load_from_project(&self.project_root);

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
        self.state.needs_watcher_registration = true;
        self.state.needs_redraw = true;
        if !parked.events.is_empty() {
            if let Err(e) = self.process_panel_events(parked.events) {
                log::error!("Error processing events of a parked project: {}", e);
            }
        }
    }

    /// Bring `state.open_projects`, which the menus draw, in step with the
    /// open projects.
    pub(super) fn sync_open_projects(&mut self) {
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

    /// Tick the panels of the parked projects. What they raise waits for
    /// their project; a request for attention rings the bell at once.
    pub(super) fn tick_parked_projects(&mut self) {
        if self.open_projects.count() < 2 {
            return;
        }
        let mut ring = false;
        for (_, parked) in self.open_projects.parked_mut() {
            for panel in parked.layout.iter_all_panels_mut() {
                for event in panel.tick() {
                    match event {
                        PanelEvent::RequestAttention => ring = true,
                        event => parked.events.push(event),
                    }
                }
            }
        }
        if ring {
            self.state.attention_bell();
        }
        self.sync_open_projects();
    }

    /// The open projects' roots in the order the menus list them.
    fn listed_open_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.open_projects.roots().map(Path::to_path_buf).collect();
        roots.sort_by(|a, b| path_order(a, b));
        roots
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

    /// Ask before closing the parked project at `root`. `menu` is where to
    /// return afterwards, see `PendingAction::CloseProject`.
    pub(super) fn confirm_close_project(&mut self, root: PathBuf, menu: Option<usize>) {
        let Some(live) = self
            .open_projects
            .parked()
            .find(|(parked_root, _)| *parked_root == root)
            .map(|(_, parked)| parked.requires_confirmation())
        else {
            return;
        };
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
            PendingAction::CloseProject { root, menu },
            ActiveModal::Confirm(Box::new(modal)),
        );
    }

    /// Close the parked project at `root`: save its layout and drop its
    /// panels, which stops the processes in its terminals.
    pub(super) fn close_project(&mut self, root: &Path, menu: Option<usize>) -> Result<()> {
        if let Some(mut parked) = self.open_projects.close(root) {
            if self.persist_layout {
                if let Err(e) = save_layout_of(root, &mut parked.layout) {
                    log::error!("Failed to save the layout of {:?}: {}", root, e);
                }
            }
            log::info!("Closed project {:?}", root);
        }
        self.sync_open_projects();
        match menu {
            Some(selection) => self.reopen_projects_menu(selection),
            None => self.handle_open_projects_modal()?,
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
