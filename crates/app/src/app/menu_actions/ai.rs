//! AI menu actions — the Agents / Sessions / Skills / Prompts submenu with
//! browse + create/edit/delete/rename, modelled on the Commands menu.
//!
//! The section item lists are built by [`crate::state::AppState::ai_section_items`]
//! (shared with the renderer), so keys are addressed by index here. Item keys:
//! `new:project` / `new:global` (create rows), `item:<name>` (an agent, skill or
//! prompt) and `session:<path>` (a session log).

use std::path::PathBuf;

use anyhow::Result;
use termide_state::AiSection;

use super::super::App;
use super::{navigate_submenu, SubmenuNavAction};

/// What a section key points at on disk, plus its display name.
struct AiTarget {
    /// The path to remove or rename (a directory for agents/skills, a file for
    /// prompts and sessions).
    path: PathBuf,
    /// The item's name, for the confirm/rename dialogs and the default value.
    name: String,
    /// Whether renaming means setting a display name (sessions) rather than
    /// moving the path.
    is_session: bool,
}

/// Truncate `s` to `max` display characters, appending an ellipsis when cut.
fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The body for the "delete session?" confirmation: the session's display
/// name, else its first prompt, else `untitled`, followed by its id.
fn session_delete_message(
    name: Option<&str>,
    first_prompt: Option<&str>,
    id: &str,
    untitled: &str,
) -> String {
    let label = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| first_prompt.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or(untitled);
    let label = truncate_label(label, 60);
    if id.is_empty() {
        label
    } else {
        format!("{label} · {id}")
    }
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
        if let Some(path) = key.strip_prefix("session:") {
            self.ai_open_session(PathBuf::from(path))?;
        }
        Ok(())
    }

    /// Open a session log: focus the agent panel already showing it, else open
    /// a fresh one, so a session is never duplicated across panels.
    fn ai_open_session(&mut self, path: PathBuf) -> Result<()> {
        self.state.close_menu();
        // Reuse an open agent panel for this session. `AgentPanel::session_path`
        // is the typed accessor; a localized downcast reads it without adding a
        // method to the `Panel` trait (which lives in another committed crate).
        let already_open = self.layout_manager.focus_panel_where(|panel| {
            panel
                .as_any()
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
        if let Some(panel) =
            crate::app::agent_panel::restore_agent_panel(&settings, cwd, Some(path), None)
        {
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
        if let Some(path) = key.strip_prefix("session:") {
            let path = PathBuf::from(path);
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Some(AiTarget {
                path,
                name,
                is_session: true,
            });
        }
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
        Some(AiTarget {
            path,
            name,
            is_session: false,
        })
    }

    /// Delete the selected item, after confirmation.
    fn delete_ai_selected(&mut self, section: AiSection) -> Result<()> {
        let Some(target) = self.ai_target(section) else {
            return Ok(());
        };
        self.state.close_menu();
        let t = termide_i18n::t();
        let (title, message) = if section == AiSection::Sessions {
            let summary = self
                .state
                .ai_sessions_dir()
                .and_then(|dir| termide_agent_core::Session::list(&dir).ok())
                .and_then(|list| list.into_iter().find(|s| s.path == target.path));
            let message = match summary {
                Some(s) => session_delete_message(
                    s.name.as_deref(),
                    s.first_prompt.as_deref(),
                    &s.id,
                    t.ai_session_untitled(),
                ),
                None => t.ai_session_untitled().to_string(),
            };
            (t.ai_delete_session_title().to_string(), message)
        } else {
            (
                t.ai_delete_title().to_string(),
                format!("{} \"{}\"", t.ai_delete_title(), target.name),
            )
        };
        let modal = termide_modal::ConfirmModal::new(title, message);
        self.state.set_pending_action(
            termide_state::PendingAction::AiDelete {
                section,
                path: target.path.to_string_lossy().into_owned(),
            },
            crate::state::ActiveModal::Confirm(Box::new(modal)),
        );
        Ok(())
    }

    /// Rename the selected item (a display name for sessions, a path otherwise).
    fn rename_ai_selected(&mut self, section: AiSection) -> Result<()> {
        let Some(target) = self.ai_target(section) else {
            return Ok(());
        };
        self.state.close_menu();
        let t = termide_i18n::t();
        // A session takes a free-form display name; the rest are identifiers.
        let (default, hint) = if target.is_session {
            ("", "")
        } else {
            (target.name.as_str(), t.ai_name_hint())
        };
        let modal = termide_modal::InputModal::with_default(t.ai_rename_title(), hint, default);
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

    /// Delete a resource's file or directory.
    pub(in crate::app) fn ai_delete_item(&mut self, section: AiSection, path: &str) -> Result<()> {
        let path = PathBuf::from(path);
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

    /// Rename a resource: a session's display name, or a file/directory move.
    pub(in crate::app) fn ai_rename_item(
        &mut self,
        section: AiSection,
        path: &str,
        new_name: &str,
    ) -> Result<()> {
        let new_name = new_name.trim();
        let path = PathBuf::from(path);
        if section == AiSection::Sessions {
            if !new_name.is_empty() {
                if let Ok(mut session) = termide_agent_core::Session::open_exclusive(&path) {
                    let _ = session.set_name(new_name);
                }
            }
            self.state.needs_redraw = true;
            return Ok(());
        }
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
        self.state.ui.ai_submenu.selected = section.index();
        self.state.open_ai_nested_submenu(section);
    }
}

#[cfg(test)]
mod tests {
    use super::{session_delete_message, truncate_label, AiSection};

    /// The renderer names the sections by key; each row must name the
    /// section at its index, which is how the handlers address it.
    #[test]
    fn the_menu_rows_are_the_sections_in_order() {
        let items = termide_ui_render::get_ai_items(None);
        assert_eq!(items.len(), termide_ui_render::AI_SUBMENU_ITEM_COUNT);
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

    #[test]
    fn session_delete_message_names_the_session() {
        // Display name wins and carries the id.
        assert_eq!(
            session_delete_message(Some("Refactor"), Some("do x"), "abc123", "untitled"),
            "Refactor · abc123"
        );
        // Falls back to the first prompt, then to "untitled".
        assert_eq!(
            session_delete_message(None, Some("fix the bug"), "id1", "untitled"),
            "fix the bug · id1"
        );
        assert_eq!(
            session_delete_message(Some("  "), None, "id2", "untitled"),
            "untitled · id2"
        );
        // A missing id drops the separator.
        assert_eq!(
            session_delete_message(Some("Solo"), None, "", "untitled"),
            "Solo"
        );
    }

    #[test]
    fn truncate_label_cuts_with_an_ellipsis() {
        assert_eq!(truncate_label("short", 60), "short");
        let long = "x".repeat(80);
        let cut = truncate_label(&long, 60);
        assert_eq!(cut.chars().count(), 60);
        assert!(cut.ends_with('…'));
    }
}
