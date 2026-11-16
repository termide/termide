//! AI menu actions — the Agents / Sessions / Skills / Prompts submenu with
//! browse + create/edit/delete/rename, modelled on the Commands menu.
//!
//! The section item lists are built by [`crate::state::AppState::ai_section_items`]
//! (shared with the renderer), so keys are addressed by index here. Item keys:
//! `new:project` / `new:global` (create rows), `item:<name>` (an agent, skill or
//! prompt) and `location:<dir>` (a directory sessions worked in, by where
//! they are filed).

use std::path::{Path, PathBuf};

use anyhow::Result;
use termide_agent_core::relocate::SessionLocation;
use termide_state::AiSection;

use super::super::{parked_projects, App};
use super::{navigate_submenu, SubmenuNavAction};
use crate::state::SESSION_LOCATION_KEY;

/// What a section key points at on disk, plus its display name.
struct AiTarget {
    /// The path to remove or rename (a directory for agents/skills, a file for
    /// prompts and sessions).
    path: PathBuf,
    /// The item's name, for the confirm/rename dialogs and the default value.
    name: String,
}

impl App {
    // =========================================================================
    // Keyboard navigation
    // =========================================================================

    /// Handle a key in the AI submenu (the four sections).
    pub(in crate::app) fn handle_ai_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        if self.state.ui.ai_nested.open {
            return self.handle_ai_nested_submenu_key(key);
        }
        let items = termide_ui_render::get_ai_items(super::super::agent_panel::web_browser_shown());
        let separators: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_separator)
            .map(|(index, _)| index)
            .collect();
        match navigate_submenu(
            &key,
            &mut self.state.ui.ai_submenu,
            items.len(),
            &separators,
        ) {
            SubmenuNavAction::Close => self.state.close_menu(),
            SubmenuNavAction::Execute | SubmenuNavAction::Right => self.open_ai_selected_section(),
            SubmenuNavAction::Left => self.switch_to_prev_menu()?,
            _ => {}
        }
        Ok(())
    }

    /// Open (or toggle) the section under the cursor, or switch the browser
    /// window when that row is under it.
    pub(in crate::app) fn open_ai_selected_section(&mut self) {
        let items = termide_ui_render::get_ai_items(super::super::agent_panel::web_browser_shown());
        let sel = self.state.ui.ai_submenu.selected;
        if let Some(item) = items.get(sel) {
            if item.is_separator {
                return;
            }
            if item.key == termide_ui_render::AI_BROWSER_KEY {
                self.state.close_menu();
                super::super::agent_panel::toggle_web_browser();
                return;
            }
            if item.key == termide_ui_render::AI_USAGE_KEY {
                self.state.close_menu();
                self.open_usage(None);
                return;
            }
            let Some(section) = AiSection::from_key(&item.key) else {
                return;
            };
            if self.state.ui.ai_nested.open && self.state.ui.current_ai_section == Some(section) {
                self.state.close_ai_nested_submenu();
            } else {
                self.state.ui.ai_nested.selected = 0;
                self.state.open_ai_nested_submenu(section);
            }
        }
    }

    /// Handle a key in a section's item list.
    fn handle_ai_nested_submenu_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let Some(section) = self.state.ui.current_ai_section else {
            self.state.close_ai_nested_submenu();
            return Ok(());
        };
        let items = self.state.ai_section_items(section);
        let separators: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.is_separator)
            .map(|(idx, _)| idx)
            .collect();
        match navigate_submenu(&key, &mut self.state.ui.ai_nested, items.len(), &separators) {
            SubmenuNavAction::Close | SubmenuNavAction::Left => {
                self.state.close_ai_nested_submenu();
            }
            SubmenuNavAction::Right => self.switch_to_next_menu()?,
            SubmenuNavAction::Execute | SubmenuNavAction::Edit => {
                self.execute_ai_nested_action(section)?;
            }
            SubmenuNavAction::Delete => self.delete_ai_selected(section)?,
            SubmenuNavAction::Rename => self.rename_ai_selected(section)?,
            SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// The key of the selected row, skipping separators and the empty
    /// placeholder.
    fn ai_selected_key(&self, section: AiSection) -> Option<String> {
        let items = self.state.ai_section_items(section);
        items
            .get(self.state.ui.ai_nested.selected)
            .filter(|i| !i.is_separator && !i.key.is_empty())
            .map(|i| i.key.clone())
    }

    // =========================================================================
    // Actions
    // =========================================================================

    /// Enter/F4 on a row: create, edit a file, or open a session.
    pub(in crate::app) fn execute_ai_nested_action(&mut self, section: AiSection) -> Result<()> {
        let Some(key) = self.ai_selected_key(section) else {
            return Ok(());
        };
        if key == "new:project" || key == "new:global" {
            self.ai_create(section, key == "new:global");
            return Ok(());
        }
        if let Some(name) = key.strip_prefix("item:") {
            let name = name.to_string();
            match section {
                AiSection::Agents => {
                    if let Some(path) = self.state.ai_dirs().agent_path(&name) {
                        self.state.close_menu();
                        self.open_path_in_editor(path)?;
                    }
                }
                AiSection::Skills => {
                    if let Some(path) = self
                        .state
                        .ai_dirs()
                        .skills()
                        .into_iter()
                        .find(|s| s.name == name)
                        .map(|s| s.path)
                    {
                        self.state.close_menu();
                        self.open_path_in_editor(path)?;
                    }
                }
                AiSection::Prompts => {
                    if let Some(path) = self.state.ai_dirs().prompt_path(&name) {
                        self.state.close_menu();
                        self.open_path_in_editor(path)?;
                    }
                }
                // Session rows carry `session:` keys, handled below.
                AiSection::Sessions => {}
            }
            return Ok(());
        }
        if let Some(dir) = key.strip_prefix(SESSION_LOCATION_KEY) {
            self.ai_open_location(Path::new(dir))?;
        }
        Ok(())
    }

    /// The listed session location filed in `dir`.
    fn ai_location(&self, dir: &Path) -> Option<SessionLocation> {
        self.state
            .cache
            .session_locations
            .iter()
            .find(|location| location.dir == dir)
            .cloned()
    }

    /// Enter on a Sessions row: an agent panel in that directory, whose
    /// banner lists its sessions to open, rename or delete; for a directory
    /// gone, ask where it is now.
    fn ai_open_location(&mut self, dir: &Path) -> Result<()> {
        let Some(location) = self.ai_location(dir) else {
            return Ok(());
        };
        self.state.close_menu();
        if location.cwd.is_dir() {
            return self.open_agent_in(location.cwd);
        }
        let name = parked_projects::display_root(&location.cwd);
        let start = location
            .cwd
            .ancestors()
            .find(|dir| dir.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.project_root.clone());
        let t = termide_i18n::t();
        let modal = termide_modal::DirectoryPickerModal::new(
            start,
            t.projects_relink_title_fmt(&name),
            t.directory_picker_select().to_string(),
        )
        .with_keybindings(&self.state.config.file_manager.keybindings);
        self.state.set_pending_action(
            termide_state::PendingAction::RelinkSessions {
                dir: location.dir,
                cwd: location.cwd,
            },
            crate::state::ActiveModal::DirectoryPicker(Box::new(modal)),
        );
        Ok(())
    }

    /// The sessions filed in `dir`, worked in `cwd` now gone, worked in
    /// `target`: file them there, their paths rewritten, as a project's
    /// follow it. Refused while one of them is open.
    pub(in crate::app) fn relink_sessions(
        &mut self,
        dir: PathBuf,
        cwd: PathBuf,
        target: PathBuf,
    ) -> Result<()> {
        let t = termide_i18n::t();
        let to_dir = super::super::agent_panel::session_dir_of(&target);
        if let Some(to_dir) = to_dir.filter(|_| target != cwd) {
            // The whole tree moves, the sessions of subdirectories too.
            if any_session_open(&dir) {
                self.show_error_modal(t.ai_sessions_open().to_string());
                return Ok(());
            }
            let relocation = termide_agent_core::relocate::SessionsRelocation {
                from_dir: &dir,
                to_dir: &to_dir,
                from_root: &cwd,
                to_root: &target,
                copy: false,
            };
            let moved = match termide_agent_core::relocate::relocate_sessions(relocation) {
                Ok(moved) => moved,
                Err(e) => {
                    log::error!("cannot move the sessions of {}: {e}", cwd.display());
                    self.show_error_modal(e.to_string());
                    return Ok(());
                }
            };
            // Agent panels of saved layouts follow their logs, as after a
            // project move.
            if let Err(e) = termide_project::relocate_session_paths(&cwd, &target, &moved) {
                log::warn!("cannot point the layouts at the moved sessions: {e:#}");
            }
            self.state
                .set_info(t.ai_sessions_relinked_fmt(&parked_projects::display_root(&target)));
        }
        self.reopen_ai_menu(AiSection::Sessions);
        Ok(())
    }

    /// Open a session log: focus the agent panel already showing it, else open
    /// a fresh one, so a session is never duplicated across panels.
    pub(in crate::app) fn ai_open_session(&mut self, path: PathBuf) -> Result<()> {
        self.state.close_menu();
        // Reuse an open agent panel for this session. `AgentPanel::session_path`
        // is the typed accessor; a localized downcast reads it without adding a
        // method to the `Panel` trait (which lives in another committed crate).
        let already_open = self.layout_manager.focus_panel_where(|panel| {
            panel
                .downcast_ref::<termide_panel_agent::AgentPanel>()
                .and_then(|agent| agent.session_path())
                .is_some_and(|open| open == path.as_path())
        });
        if already_open {
            self.state.needs_redraw = true;
            return Ok(());
        }
        let settings = self.state.config.ai.clone();
        let cwd = self.project_root.clone();
        if let Some(panel) = crate::app::agent_panel::restore_agent_panel(
            &settings,
            &self.project_root,
            cwd,
            Some(path),
            None,
            termide_core::AgentSetupState::default(),
        ) {
            self.add_panel(Box::new(panel));
            self.auto_save_layout();
        }
        Ok(())
    }

    /// Ask for a name, then scaffold the new item at the chosen scope.
    fn ai_create(&mut self, section: AiSection, scope_global: bool) {
        let t = termide_i18n::t();
        let title = match section {
            AiSection::Agents => t.ai_create_agent_title(),
            AiSection::Skills => t.ai_create_skill_title(),
            AiSection::Prompts => t.ai_create_prompt_title(),
            // Sessions come from the agent panel, not from this menu.
            AiSection::Sessions => return,
        };
        self.state.close_menu();
        let modal = termide_modal::InputModal::new(title, t.ai_name_hint());
        self.state.set_pending_action(
            termide_state::PendingAction::AiCreate {
                section,
                scope_global,
            },
            crate::state::ActiveModal::Input(Box::new(modal)),
        );
    }

    /// Resolve the on-disk target of the selected row.
    fn ai_target(&self, section: AiSection) -> Option<AiTarget> {
        let key = self.ai_selected_key(section)?;
        let name = key.strip_prefix("item:")?.to_string();
        let dirs = self.state.ai_dirs();
        let path = match section {
            // The default agent is edited, never renamed or removed: every
            // panel starts as it, and its files are seeded back anyway.
            AiSection::Agents if name == termide_agent_core::DEFAULT_AGENT => return None,
            AiSection::Agents => dirs.agent_dir(&name)?,
            AiSection::Skills => dirs
                .skills()
                .into_iter()
                .find(|s| s.name == name)
                .and_then(|s| s.path.parent().map(|p| p.to_path_buf()))?,
            AiSection::Prompts => dirs.prompt_path(&name)?,
            AiSection::Sessions => return None,
        };
        Some(AiTarget { path, name })
    }

    /// Delete the selected item, after confirmation; on a Sessions row, all
    /// the sessions of that directory.
    fn delete_ai_selected(&mut self, section: AiSection) -> Result<()> {
        let t = termide_i18n::t();
        let (path, title, message) = if section == AiSection::Sessions {
            let Some(location) = self
                .ai_selected_key(section)
                .as_deref()
                .and_then(|key| key.strip_prefix(SESSION_LOCATION_KEY))
                .and_then(|dir| self.ai_location(Path::new(dir)))
            else {
                return Ok(());
            };
            let message = t.ai_delete_location_fmt(
                location.count,
                &parked_projects::display_root(&location.cwd),
            );
            (
                location.dir,
                t.ai_delete_location_title().to_string(),
                message,
            )
        } else {
            let Some(target) = self.ai_target(section) else {
                return Ok(());
            };
            let message = format!("{} \"{}\"", t.ai_delete_title(), target.name);
            (target.path, t.ai_delete_title().to_string(), message)
        };
        self.state.close_menu();
        let modal = termide_modal::ConfirmModal::new(title, message);
        self.state.set_pending_action(
            termide_state::PendingAction::AiDelete {
                section,
                path: path.to_string_lossy().into_owned(),
            },
            crate::state::ActiveModal::Confirm(Box::new(modal)),
        );
        Ok(())
    }

    /// Rename the selected agent, skill or prompt; a Sessions row has no name
    /// to change.
    fn rename_ai_selected(&mut self, section: AiSection) -> Result<()> {
        let Some(target) = self.ai_target(section) else {
            return Ok(());
        };
        self.state.close_menu();
        let t = termide_i18n::t();
        let modal = termide_modal::InputModal::with_default(
            t.ai_rename_title(),
            t.ai_name_hint(),
            &target.name,
        );
        self.state.set_pending_action(
            termide_state::PendingAction::AiRename {
                section,
                path: target.path.to_string_lossy().into_owned(),
            },
            crate::state::ActiveModal::Input(Box::new(modal)),
        );
        Ok(())
    }

    // =========================================================================
    // Modal results (called from the modal handler)
    // =========================================================================

    /// A valid resource name: non-empty, no path separators.
    fn valid_ai_name(name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }

    /// The create root for the chosen scope: the project's `.termide/ai` or the
    /// global `<config>/ai`.
    fn ai_create_root(&self, scope_global: bool) -> PathBuf {
        if scope_global {
            termide_config::get_config_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("ai")
        } else {
            self.project_root.join(".termide").join("ai")
        }
    }

    /// Scaffold a new agent/skill/prompt and open its primary file.
    pub(in crate::app) fn ai_create_item(
        &mut self,
        section: AiSection,
        scope_global: bool,
        name: &str,
    ) -> Result<()> {
        let name = name.trim();
        if !Self::valid_ai_name(name) {
            self.show_error_modal(termide_i18n::t().ai_name_invalid().to_string());
            return Ok(());
        }
        let root = self.ai_create_root(scope_global);
        let open_path: Option<PathBuf> = match section {
            AiSection::Agents => {
                let dir = root.join("agents").join(name);
                // `default` exists already, and a project's would be ignored.
                if dir.exists() || name == termide_agent_core::DEFAULT_AGENT {
                    None
                } else {
                    std::fs::create_dir_all(&dir)?;
                    let file = dir.join(termide_agent_core::AGENT_FILE);
                    std::fs::write(
                        &file,
                        format!(
                            "---\ndescription: \n# model: \n# mode: ask\n---\n\
                             You are {name}.\n\n{{{{tools}}}}\n"
                        ),
                    )?;
                    Some(file)
                }
            }
            AiSection::Skills => {
                let dir = root.join("skills").join(name);
                if dir.exists() {
                    None
                } else {
                    std::fs::create_dir_all(&dir)?;
                    let file = dir.join("SKILL.md");
                    std::fs::write(&file, format!("---\nname: {name}\ndescription: \n---\n\n"))?;
                    Some(file)
                }
            }
            AiSection::Prompts => {
                let dir = root.join("prompts");
                std::fs::create_dir_all(&dir)?;
                let file = dir.join(format!("{name}.md"));
                if file.exists() {
                    None
                } else {
                    std::fs::write(&file, "---\ndescription: \nargument-hint: \n---\n\n")?;
                    Some(file)
                }
            }
            // Sessions come from the agent panel, not from this menu.
            AiSection::Sessions => return Ok(()),
        };
        match open_path {
            Some(path) => self.open_path_in_editor(path)?,
            None => self.show_error_modal(termide_i18n::t().ai_name_exists().to_string()),
        }
        Ok(())
    }

    /// Delete a resource's file or directory; for Sessions, the sessions
    /// filed in the directory `path` but those open in a panel, which stay
    /// and are reported.
    pub(in crate::app) fn ai_delete_item(&mut self, section: AiSection, path: &str) -> Result<()> {
        let path = PathBuf::from(path);
        if section == AiSection::Sessions {
            match termide_agent_core::relocate::delete_closed_sessions(&path) {
                Ok(0) => {}
                Ok(kept) => self
                    .state
                    .set_info(termide_i18n::t().ai_sessions_kept_open_fmt(kept)),
                Err(error) => {
                    log::error!("cannot delete the sessions in {}: {error}", path.display());
                    self.state.set_error(error.to_string());
                }
            }
            self.state.needs_redraw = true;
            return Ok(());
        }
        let result = if section.item_is_file() {
            std::fs::remove_file(&path)
        } else {
            std::fs::remove_dir_all(&path)
        };
        if let Err(error) = result {
            log::error!("cannot delete {}: {error}", path.display());
        }
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Rename a resource: its file or directory moves.
    pub(in crate::app) fn ai_rename_item(
        &mut self,
        section: AiSection,
        path: &str,
        new_name: &str,
    ) -> Result<()> {
        let new_name = new_name.trim();
        let path = PathBuf::from(path);
        if !Self::valid_ai_name(new_name) {
            self.show_error_modal(termide_i18n::t().ai_name_invalid().to_string());
            return Ok(());
        }
        let target = if section == AiSection::Prompts {
            path.with_file_name(format!("{new_name}.md"))
        } else {
            path.with_file_name(new_name)
        };
        if target.exists() {
            self.show_error_modal(termide_i18n::t().ai_name_exists().to_string());
            return Ok(());
        }
        if let Err(error) = std::fs::rename(&path, &target) {
            log::error!("cannot rename {}: {error}", path.display());
        }
        self.state.needs_redraw = true;
        Ok(())
    }

    // =========================================================================
    // Reopen after a modal
    // =========================================================================

    /// Reopen the AI menu on `section` after a create/delete/rename modal.
    pub(in crate::app) fn reopen_ai_menu(&mut self, section: AiSection) {
        use termide_ui_render::menu::AI_MENU_INDEX;
        self.state.ui.menu_open = true;
        self.state.ui.selected_menu_item = Some(AI_MENU_INDEX);
        self.state.open_ai_submenu();
        self.state.ui.ai_submenu.select(section.index());
        self.state.open_ai_nested_submenu(section);
    }
}

/// Whether a session log anywhere under `dir` is open: in a panel here or
/// in another instance.
fn any_session_open(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => any_session_open(&path),
            _ => termide_agent_core::Session::is_open(&path),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::AiSection;

    /// The renderer names the sections by key; each row must name the
    /// section at its index, which is how the handlers address it.
    #[test]
    fn the_menu_rows_are_the_sections_in_order() {
        let items = termide_ui_render::get_ai_items(None);
        assert_eq!(items.len(), termide_ui_render::AI_SUBMENU_ITEM_COUNT + 2);
        for section in AiSection::ALL {
            assert_eq!(items[section.index()].key, section.key());
        }
        assert_eq!(
            AiSection::Agents.index(),
            termide_ui_render::AI_SUBMENU_AGENTS
        );
        assert_eq!(
            AiSection::Sessions.index(),
            termide_ui_render::AI_SUBMENU_SESSIONS
        );
        assert_eq!(
            AiSection::Skills.index(),
            termide_ui_render::AI_SUBMENU_SKILLS
        );
        assert_eq!(
            AiSection::Prompts.index(),
            termide_ui_render::AI_SUBMENU_PROMPTS
        );
    }
}
