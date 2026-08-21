//! Layout manager session serialization.
//!
//! Provides session save/restore functionality for the layout manager.

use std::path::{Path, PathBuf};

use anyhow::Result;

use termide_core::Panel;
use termide_layout::{LayoutManager, PanelGroup, MIN_PANEL_HEIGHT};

/// First existing directory at or above `dir`, or `None` when even the root is
/// unreachable.
///
/// The session stores the directory a terminal was last working in, which can
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
use termide_panel_editor::{Editor, EditorConfig};
use termide_panel_file_manager::FileManager;
use termide_panel_image::ImagePanel;
use termide_panel_misc::JournalPanel;
use termide_panel_terminal::Terminal;
use termide_session::{
    cleanup_unsaved_buffer, load_unsaved_buffer, Session, SessionGroupMode, SessionPanel,
    SessionPanelGroup,
};
use termide_theme::Theme;

/// Extension trait for session serialization.
pub trait LayoutManagerSession {
    /// Serialize current layout to Session.
    fn to_session(&mut self, session_dir: &Path) -> Session;

    /// Restore layout from Session.
    fn from_session(
        session: Session,
        session_dir: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
    ) -> Result<LayoutManager>;
}

impl LayoutManagerSession for LayoutManager {
    fn to_session(&mut self, session_dir: &Path) -> Session {
        let panel_groups: Vec<SessionPanelGroup> = self
            .panel_groups
            .iter_mut()
            .map(|group| {
                let panels: Vec<_> = group
                    .panels_mut()
                    .iter_mut()
                    .filter_map(|panel| panel.to_session(session_dir))
                    .collect();

                SessionPanelGroup {
                    panels,
                    expanded_index: group.expanded_index(),
                    width: group.width,
                    // `mode` is legacy — never written by current code.
                    mode: SessionGroupMode::default(),
                    split_heights: group.split_heights().map(|s| s.to_vec()),
                    fullscreen_cache: group.fullscreen_cache().map(|c| c.to_vec()),
                }
            })
            .collect();

        Session {
            panel_groups,
            focused_group: self.focus,
        }
    }

    fn from_session(
        session: Session,
        session_dir: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
    ) -> Result<LayoutManager> {
        let mut layout = LayoutManager::new();

        for session_group in session.panel_groups {
            if session_group.panels.is_empty() {
                continue;
            }

            // Construct every panel in this group on its own worker
            // thread so heavy initializers (file reads, PTY spawn, VFS
            // probes, local directory walks) run concurrently instead
            // of stacking up on the main thread. The session-dir,
            // editor config and terminal dimensions are cheap to clone
            // per worker; everything else is move-by-value out of the
            // session struct.
            let session_dir_owned: PathBuf = session_dir.to_path_buf();
            let handles: Vec<_> = session_group
                .panels
                .into_iter()
                .map(|session_panel| {
                    let session_dir = session_dir_owned.clone();
                    let editor_config = editor_config.clone();
                    std::thread::spawn(move || {
                        construct_panel(
                            session_panel,
                            &session_dir,
                            term_height,
                            term_width,
                            editor_config,
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
                        log::warn!("panel construction worker panicked during session restore")
                    }
                }
            }

            if panels.is_empty() {
                continue;
            }

            let n_panels = panels.len();
            let expanded_idx = session_group.expanded_index.min(n_panels - 1);

            // Decide what fullscreen-cache to seed the group with so
            // toggle-off in this run restores the user's pre-fullscreen
            // layout from the previous run, not a generated preset.
            //
            // 1. New sessions explicitly carry `fullscreen_cache` when
            //    the preset was active at save time.
            // 2. Legacy sessions with `mode = Accordion` and no
            //    `split_heights` (= old binary accordion view) get a
            //    fresh equal-distribution cache so toggling off lands
            //    the user in a sane free-resize layout.
            let area_height = term_height.saturating_sub(2);
            let fullscreen_cache = if let Some(cache) = session_group.fullscreen_cache {
                Some(cache)
            } else if matches!(session_group.mode, SessionGroupMode::Accordion)
                && session_group.split_heights.is_none()
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
            // is the preset shape (or absent for legacy sessions); apply
            // the preset for the focused panel.
            let in_fullscreen = fullscreen_cache.is_some();
            let split_heights = if in_fullscreen {
                Some(fullscreen_preset(n_panels, expanded_idx, area_height))
            } else {
                session_group.split_heights
            };

            let mut group = PanelGroup::from_parts(
                panels,
                expanded_idx,
                session_group.width,
                split_heights,
                fullscreen_cache,
            );
            // RefreshIfStale on the focused panel — `from_parts` is a
            // raw constructor and skips that signal.
            if let Some(panel) = group.expanded_panel_mut() {
                panel.handle_command(termide_core::PanelCommand::RefreshIfStale);
            }

            layout.panel_groups.push(group);
        }

        layout.focus = session
            .focused_group
            .min(layout.panel_groups.len().saturating_sub(1));

        Ok(layout)
    }
}

/// Build one panel from its saved descriptor.
///
/// Runs from a worker thread (see `from_session`), so any blocking I/O
/// — file reads, VFS probes, PTY spawn — overlaps with other panels in
/// the group instead of stacking on the main thread. All inputs are
/// owned so the closure is `Send` without extra dances; logging of
/// failures happens here so the caller can just `match` the result.
fn construct_panel(
    session_panel: SessionPanel,
    session_dir: &Path,
    term_height: u16,
    term_width: u16,
    editor_config: EditorConfig,
) -> Option<Box<dyn Panel + Send>> {
    match session_panel {
        SessionPanel::FileManager { path_or_url } => {
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
        SessionPanel::Editor {
            path,
            unsaved_buffer_file,
        } => {
            if let Some(file_path) = path {
                Editor::open_file_with_config(file_path, editor_config)
                    .ok()
                    .map(|e| Box::new(e) as Box<dyn Panel + Send>)
            } else if let Some(ref buffer_file) = unsaved_buffer_file {
                match load_unsaved_buffer(session_dir, buffer_file) {
                    Ok(content) => {
                        if content.trim().is_empty() {
                            if let Err(e) = cleanup_unsaved_buffer(session_dir, buffer_file) {
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
        SessionPanel::Terminal { working_dir } => {
            Terminal::new_with_cwd(term_height, term_width, nearest_existing_dir(&working_dir))
                .ok()
                .map(|t| Box::new(t) as Box<dyn Panel + Send>)
        }
        SessionPanel::Journal => Some(Box::new(JournalPanel::default())),
        SessionPanel::Image { path } => {
            if ImagePanel::graphics_available() {
                ImagePanel::new(path)
                    .ok()
                    .map(|p| Box::new(p) as Box<dyn Panel + Send>)
            } else {
                None
            }
        }
        SessionPanel::Binary { path } => termide_panel_binary::BinaryPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        SessionPanel::Markdown { path } => termide_panel_markdown::MarkdownPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        SessionPanel::Mermaid { path } => termide_panel_mermaid::MermaidPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        SessionPanel::Html { path } => termide_panel_html::HtmlPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        SessionPanel::GitStatus { repo_path } => Some(Box::new(
            termide_panel_git_status::GitStatusPanel::new_for_repo(repo_path),
        )),
        SessionPanel::GitLog { repo_path } => Some(Box::new(
            termide_panel_git_log::GitLogPanel::new_for_repo(repo_path),
        )),
        SessionPanel::GitDiff {
            repo_path,
            commit_hash,
        } => Some(Box::new(match commit_hash {
            Some(hash) => termide_panel_git_diff::GitDiffPanel::new_for_commit(repo_path, hash),
            None => termide_panel_git_diff::GitDiffPanel::new(repo_path),
        })),
        SessionPanel::Outline => Some(Box::new(termide_panel_outline::OutlinePanel::new(
            Theme::default(),
        ))),
        SessionPanel::Diagnostics => Some(Box::new(
            termide_panel_diagnostics::DiagnosticsPanel::new(&Theme::default()),
        )),
        SessionPanel::Database { url, label } => {
            Some(Box::new(termide_panel_db::DbPanel::new(url, label)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal's saved directory can vanish between sessions; restoring must
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
