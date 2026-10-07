//! Project layout persistence for the layout manager.
//!
//! Converts the live layout to a `ProjectLayout` and back.

use std::path::{Path, PathBuf};

use anyhow::Result;

use termide_core::Panel;
use termide_layout::{LayoutManager, PanelGroup, MIN_PANEL_HEIGHT};

/// First existing directory at or above `dir`, or `None` when even the root is
/// unreachable.
///
/// The layout stores the directory a terminal was last working in, which can
/// be gone by the next launch (a temp directory, an unmounted share). Spawning
/// a shell there fails and would silently drop the panel, so fall back to the
/// nearest surviving ancestor; `None` lets the terminal pick its own default.
fn nearest_existing_dir(dir: &Path) -> Option<PathBuf> {
    let mut candidate = Some(dir);
    while let Some(path) = candidate {
        if path.is_dir() {
            return Some(path.to_path_buf());
        }
        candidate = path.parent();
    }
    None
}

fn fullscreen_preset(n: usize, focused: usize, area_height: u16) -> Vec<u16> {
    let collapsed_total = MIN_PANEL_HEIGHT as u32 * (n as u32 - 1);
    let focused_height = (area_height as u32)
        .saturating_sub(collapsed_total)
        .max(MIN_PANEL_HEIGHT as u32) as u16;
    let mut heights = vec![MIN_PANEL_HEIGHT; n];
    if focused < n {
        heights[focused] = focused_height;
    }
    heights
}
use termide_config::AiSettings;
use termide_panel_editor::{Editor, EditorConfig};
use termide_panel_file_manager::FileManager;
use termide_panel_image::ImagePanel;
use termide_panel_misc::JournalPanel;
use termide_panel_terminal::Terminal;
use termide_project::{
    cleanup_unsaved_buffer, load_unsaved_buffer, GroupLayoutMode, PanelGroupState, PanelState,
    ProjectLayout,
};
use termide_theme::Theme;

/// Extension trait converting the layout manager to and from a project layout.
pub trait LayoutPersistence {
    /// Serialize current layout to ProjectLayout.
    fn to_state(&mut self, project_dir: &Path) -> ProjectLayout;

    /// Restore layout from ProjectLayout.
    fn from_layout(
        layout: ProjectLayout,
        project_dir: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
        agent_settings: AiSettings,
    ) -> Result<LayoutManager>;
}

impl LayoutPersistence for LayoutManager {
    fn to_state(&mut self, project_dir: &Path) -> ProjectLayout {
        let panel_groups: Vec<PanelGroupState> = self
            .panel_groups
            .iter_mut()
            .map(|group| {
                let panels: Vec<_> = group
                    .panels_mut()
                    .iter_mut()
                    .filter_map(|panel| panel.to_state(project_dir))
                    .collect();

                PanelGroupState {
                    panels,
                    expanded_index: group.expanded_index(),
                    width: group.width,
                    // `mode` is legacy — never written by current code.
                    mode: GroupLayoutMode::default(),
                    split_heights: group.split_heights().map(|s| s.to_vec()),
                    fullscreen_cache: group.fullscreen_cache().map(|c| c.to_vec()),
                }
            })
            .collect();

        ProjectLayout {
            panel_groups,
            focused_group: self.focus,
        }
    }

    fn from_layout(
        layout: ProjectLayout,
        project_dir: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
        agent_settings: AiSettings,
    ) -> Result<LayoutManager> {
        let mut manager = LayoutManager::new();

        for saved_group in layout.panel_groups {
            if saved_group.panels.is_empty() {
                continue;
            }

            // Construct every panel in this group on its own worker
            // thread so heavy initializers (file reads, PTY spawn, VFS
            // probes, local directory walks) run concurrently instead
            // of stacking up on the main thread. The project dir,
            // editor config and terminal dimensions are cheap to clone
            // per worker; everything else is move-by-value out of the
            // saved layout.
            let project_dir_owned: PathBuf = project_dir.to_path_buf();
            let handles: Vec<_> = saved_group
                .panels
                .into_iter()
                .map(|saved_panel| {
                    let project_dir = project_dir_owned.clone();
                    let editor_config = editor_config.clone();
                    let agent_settings = agent_settings.clone();
                    std::thread::spawn(move || {
                        construct_panel(
                            saved_panel,
                            &project_dir,
                            term_height,
                            term_width,
                            editor_config,
                            &agent_settings,
                        )
                    })
                })
                .collect();

            let mut panels: Vec<Box<dyn Panel>> = Vec::with_capacity(handles.len());
            for handle in handles {
                match handle.join() {
                    Ok(Some(panel)) => panels.push(panel),
                    Ok(None) => {} // construct_panel logged the failure
                    Err(_) => {
                        log::warn!("panel construction worker panicked during layout restore")
                    }
                }
            }

            if panels.is_empty() {
                continue;
            }

            let n_panels = panels.len();
            let expanded_idx = saved_group.expanded_index.min(n_panels - 1);

            // Decide what fullscreen-cache to seed the group with so
            // toggle-off in this run restores the user's pre-fullscreen
            // layout from the previous run, not a generated preset.
            //
            // 1. New layouts explicitly carry `fullscreen_cache` when
            //    the preset was active at save time.
            // 2. Legacy layouts with `mode = Accordion` and no
            //    `split_heights` (= old binary accordion view) get a
            //    fresh equal-distribution cache so toggling off lands
            //    the user in a sane free-resize layout.
            let area_height = term_height.saturating_sub(2);
            let fullscreen_cache = if let Some(cache) = saved_group.fullscreen_cache {
                Some(cache)
            } else if matches!(saved_group.mode, GroupLayoutMode::Accordion)
                && saved_group.split_heights.is_none()
                && n_panels >= 2
            {
                let per = area_height / n_panels as u16;
                let rem = area_height % n_panels as u16;
                let cache: Vec<u16> = (0..n_panels as u16)
                    .map(|i| if i < rem { per + 1 } else { per }.max(1))
                    .collect();
                Some(cache)
            } else {
                None
            };

            // If we have a fullscreen cache, the on-disk `split_heights`
            // is the preset shape (or absent for legacy layouts); apply
            // the preset for the focused panel.
            let in_fullscreen = fullscreen_cache.is_some();
            let split_heights = if in_fullscreen {
                Some(fullscreen_preset(n_panels, expanded_idx, area_height))
            } else {
                saved_group.split_heights
            };

            let mut group = PanelGroup::from_parts(
                panels,
                expanded_idx,
                saved_group.width,
                split_heights,
                fullscreen_cache,
            );
            // RefreshIfStale on the focused panel — `from_parts` is a
            // raw constructor and skips that signal.
            if let Some(panel) = group.expanded_panel_mut() {
                panel.handle_command(termide_core::PanelCommand::RefreshIfStale);
            }

            manager.panel_groups.push(group);
        }

        manager.focus = layout
            .focused_group
            .min(manager.panel_groups.len().saturating_sub(1));

        Ok(manager)
    }
}

/// Build one panel from its saved descriptor.
///
/// Runs from a worker thread (see `from_layout`), so any blocking I/O
/// — file reads, VFS probes, PTY spawn — overlaps with other panels in
/// the group instead of stacking on the main thread. All inputs are
/// owned so the closure is `Send` without extra dances; logging of
/// failures happens here so the caller can just `match` the result.
fn construct_panel(
    saved_panel: PanelState,
    project_dir: &Path,
    term_height: u16,
    term_width: u16,
    editor_config: EditorConfig,
    agent_settings: &AiSettings,
) -> Option<Box<dyn Panel + Send>> {
    match saved_panel {
        PanelState::FileManager { path_or_url } => {
            if termide_vfs::is_vfs_url(&path_or_url) {
                let vfs_manager = std::sync::Arc::new(termide_vfs::VfsManager::new());
                match FileManager::new_with_vfs_url(&path_or_url, vfs_manager) {
                    Ok(fm) => Some(Box::new(fm)),
                    Err(e) => {
                        log::warn!(
                            "Failed to restore remote FileManager at '{}': {}",
                            path_or_url,
                            e
                        );
                        None
                    }
                }
            } else {
                Some(Box::new(FileManager::new_with_path(PathBuf::from(
                    path_or_url,
                ))))
            }
        }
        PanelState::Editor {
            path,
            unsaved_buffer_file,
        } => {
            if let Some(file_path) = path {
                Editor::open_file_with_config(file_path, editor_config)
                    .ok()
                    .map(|e| Box::new(e) as Box<dyn Panel + Send>)
            } else if let Some(ref buffer_file) = unsaved_buffer_file {
                match load_unsaved_buffer(project_dir, buffer_file) {
                    Ok(content) => {
                        if content.trim().is_empty() {
                            if let Err(e) = cleanup_unsaved_buffer(project_dir, buffer_file) {
                                log::warn!("cleanup_unsaved_buffer({}) failed: {e}", buffer_file);
                            }
                            None
                        } else {
                            let mut editor = Editor::with_config(editor_config);
                            if let Err(e) = editor.insert_text(&content) {
                                log::warn!("Failed to restore unsaved buffer content: {}", e);
                                None
                            } else {
                                editor.set_unsaved_buffer_file(Some(buffer_file.clone()));
                                Some(Box::new(editor) as Box<dyn Panel + Send>)
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to load unsaved buffer {}: {}", buffer_file, e);
                        None
                    }
                }
            } else {
                None
            }
        }
        PanelState::Terminal { working_dir } => {
            Terminal::new_with_cwd(term_height, term_width, nearest_existing_dir(&working_dir))
                .ok()
                .map(|t| Box::new(t) as Box<dyn Panel + Send>)
        }
        PanelState::Journal => Some(Box::new(JournalPanel::default())),
        PanelState::Image { path } => {
            if ImagePanel::graphics_available() {
                ImagePanel::new(path)
                    .ok()
                    .map(|p| Box::new(p) as Box<dyn Panel + Send>)
            } else {
                None
            }
        }
        PanelState::Binary { path } => termide_panel_binary::BinaryPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        PanelState::Markdown { path } => termide_panel_markdown::MarkdownPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        PanelState::Mermaid { path } => termide_panel_mermaid::MermaidPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        PanelState::Html { path } => termide_panel_html::HtmlPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        PanelState::GitStatus { repo_path } => Some(Box::new(
            termide_panel_git_status::GitStatusPanel::new_for_repo(repo_path),
        )),
        PanelState::GitLog { repo_path } => Some(Box::new(
            termide_panel_git_log::GitLogPanel::new_for_repo(repo_path),
        )),
        PanelState::GitDiff {
            repo_path,
            commit_hash,
        } => Some(Box::new(match commit_hash {
            Some(hash) => termide_panel_git_diff::GitDiffPanel::new_for_commit(repo_path, hash),
            None => termide_panel_git_diff::GitDiffPanel::new(repo_path),
        })),
        PanelState::Outline => Some(Box::new(termide_panel_outline::OutlinePanel::new(
            Theme::default(),
        ))),
        PanelState::Diagnostics => Some(Box::new(
            termide_panel_diagnostics::DiagnosticsPanel::new(&Theme::default()),
        )),
        PanelState::Database { url, label } => {
            Some(Box::new(termide_panel_db::DbPanel::new(url, label)))
        }
        PanelState::Agent {
            cwd,
            session,
            agent,
            setup,
        } => {
            crate::app::agent_panel::restore_agent_panel(agent_settings, cwd, session, agent, setup)
                .map(|p| Box::new(p) as Box<dyn Panel + Send>)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal's saved directory can vanish between runs; restoring must
    /// climb to a surviving ancestor rather than fail to spawn the shell.
    #[test]
    fn nearest_existing_dir_climbs_to_a_surviving_ancestor() {
        let base = std::env::temp_dir().join(format!("termide-restore-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();

        assert_eq!(nearest_existing_dir(&base).as_deref(), Some(base.as_path()));
        let gone = base.join("gone/deeper");
        assert_eq!(nearest_existing_dir(&gone).as_deref(), Some(base.as_path()));

        let _ = std::fs::remove_dir_all(&base);
        // With the whole subtree gone, the temp dir itself is the fallback.
        assert_eq!(
            nearest_existing_dir(&gone).as_deref(),
            Some(std::env::temp_dir().as_path())
        );
    }
}
