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

/// Where the item at `index` lands once only the items flagged in `kept`
/// remain, or `None` when nothing remains. A dropped item hands its place to
/// the kept one before it (the next one when none precedes it), as closing a
/// panel does.
fn remap_index(kept: &[bool], index: usize) -> Option<usize> {
    let remaining = kept.iter().filter(|&&k| k).count();
    if remaining == 0 {
        return None;
    }
    let index = index.min(kept.len() - 1);
    let before = kept[..index].iter().filter(|&&k| k).count();
    let mapped = if kept[index] {
        before
    } else {
        before.saturating_sub(1)
    };
    Some(mapped.min(remaining - 1))
}

/// The entries of a per-panel vector for the panels flagged in `kept`.
/// A vector that does not match the panel count is stale and dropped, as
/// `PanelGroup::from_parts` would; so is one with nothing left.
fn retain_per_panel(values: Option<Vec<u16>>, kept: &[bool]) -> Option<Vec<u16>> {
    let values = values.filter(|v| v.len() == kept.len())?;
    let retained: Vec<u16> = values
        .into_iter()
        .zip(kept)
        .filter_map(|(v, &k)| k.then_some(v))
        .collect();
    (!retained.is_empty()).then_some(retained)
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
    ///
    /// `project_root` is the project on disk (not its state directory): a
    /// restored Git panel opens on the repository it was saved with, but
    /// lists the whole project's plane in its dropdown.
    fn from_layout(
        layout: ProjectLayout,
        project_dir: &Path,
        project_root: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
        agent_settings: AiSettings,
    ) -> Result<LayoutManager>;
}

impl LayoutPersistence for LayoutManager {
    fn to_state(&mut self, project_dir: &Path) -> ProjectLayout {
        // Panels without a saved form (plugin panels, operations, an empty
        // editor…) are left out, so every per-panel index and vector is
        // remapped to the panels that are written, and a group left with
        // none is not written at all.
        let mut groups_kept = Vec::with_capacity(self.panel_groups.len());
        let mut panel_groups: Vec<PanelGroupState> = Vec::new();
        for group in self.panel_groups.iter_mut() {
            let states: Vec<Option<PanelState>> = group
                .panels_mut()
                .iter_mut()
                .map(|panel| panel.to_state(project_dir))
                .collect();
            let kept: Vec<bool> = states.iter().map(Option::is_some).collect();
            let Some(expanded_index) = remap_index(&kept, group.expanded_index()) else {
                groups_kept.push(false);
                continue;
            };
            groups_kept.push(true);
            panel_groups.push(PanelGroupState {
                panels: states.into_iter().flatten().collect(),
                expanded_index,
                width: group.width,
                // `mode` is legacy — never written by current code.
                mode: GroupLayoutMode::default(),
                split_heights: retain_per_panel(group.split_heights().map(<[u16]>::to_vec), &kept),
                fullscreen_cache: retain_per_panel(
                    group.fullscreen_cache().map(<[u16]>::to_vec),
                    &kept,
                ),
            });
        }

        ProjectLayout {
            panel_groups,
            focused_group: remap_index(&groups_kept, self.focus).unwrap_or(0),
        }
    }

    fn from_layout(
        layout: ProjectLayout,
        project_dir: &Path,
        project_root: &Path,
        term_height: u16,
        term_width: u16,
        editor_config: EditorConfig,
        agent_settings: AiSettings,
    ) -> Result<LayoutManager> {
        let mut manager = LayoutManager::new();
        // Which saved groups came back, to move the focus with them.
        let mut groups_kept = Vec::with_capacity(layout.panel_groups.len());

        for saved_group in layout.panel_groups {
            groups_kept.push(false);
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
            let project_root_owned: PathBuf = project_root.to_path_buf();
            let handles: Vec<_> = saved_group
                .panels
                .into_iter()
                .map(|saved_panel| {
                    let project_dir = project_dir_owned.clone();
                    let project_root = project_root_owned.clone();
                    let editor_config = editor_config.clone();
                    let agent_settings = agent_settings.clone();
                    std::thread::spawn(move || {
                        construct_panel(
                            saved_panel,
                            &project_dir,
                            &project_root,
                            term_height,
                            term_width,
                            editor_config,
                            &agent_settings,
                        )
                    })
                })
                .collect();

            let mut panels: Vec<Box<dyn Panel>> = Vec::with_capacity(handles.len());
            let mut kept = Vec::with_capacity(handles.len());
            for handle in handles {
                let built_before = panels.len();
                match handle.join() {
                    Ok(Some(panel)) => panels.push(panel),
                    Ok(None) => {} // construct_panel logged the failure
                    Err(_) => {
                        log::warn!("panel construction worker panicked during layout restore")
                    }
                }
                kept.push(panels.len() > built_before);
            }

            // Panels that failed to rebuild shift the rest, so the focus
            // and the per-panel heights follow the ones that came back.
            let Some(expanded_idx) = remap_index(&kept, saved_group.expanded_index) else {
                continue;
            };
            let legacy_without_heights = saved_group.split_heights.is_none();
            let saved_split_heights = retain_per_panel(saved_group.split_heights, &kept);
            let saved_fullscreen_cache = retain_per_panel(saved_group.fullscreen_cache, &kept);
            let n_panels = panels.len();

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
            let fullscreen_cache = if let Some(cache) = saved_fullscreen_cache {
                Some(cache)
            } else if matches!(saved_group.mode, GroupLayoutMode::Accordion)
                && legacy_without_heights
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
                saved_split_heights
            };

            let mut group = PanelGroup::from_parts(
                panels,
                expanded_idx,
                restored_width(saved_group.width, term_width),
                split_heights,
                fullscreen_cache,
            );
            // RefreshIfStale on the focused panel — `from_parts` is a
            // raw constructor and skips that signal.
            if let Some(panel) = group.expanded_panel_mut() {
                panel.handle_command(termide_core::PanelCommand::RefreshIfStale);
            }

            manager.panel_groups.push(group);
            *groups_kept.last_mut().expect("pushed above") = true;
        }

        manager.focus = remap_index(&groups_kept, layout.focused_group).unwrap_or(0);

        Ok(manager)
    }
}

/// A saved group width, no wider than the terminal: the file may have been
/// edited by hand, and widths far past the screen would only be squeezed —
/// or, summed, overflow.
fn restored_width(saved: Option<u16>, term_width: u16) -> Option<u16> {
    saved.map(|w| w.min(term_width.max(1)))
}

/// Reopen the file of a saved editor, with the unsaved changes kept in the
/// backup `unsaved_buffer_file` when there is one (left by a termide that was
/// ended without asking). Should the file be gone, the changes still open,
/// in an editor of their own.
fn restore_file_editor(
    file_path: PathBuf,
    unsaved_buffer_file: Option<String>,
    project_dir: &Path,
    editor_config: EditorConfig,
) -> Option<Editor> {
    let opened = Editor::open_file_with_config(file_path.clone(), editor_config.clone());
    let Some(buffer_file) = unsaved_buffer_file else {
        return opened.ok();
    };
    let changes = match load_unsaved_buffer(project_dir, &buffer_file) {
        Ok(changes) => changes,
        Err(e) => {
            log::warn!("Failed to load the unsaved changes to {file_path:?}: {e:#}");
            return opened.ok();
        }
    };
    let restored = opened.and_then(|mut editor| {
        editor.restore_unsaved_changes(&changes)?;
        Ok(editor)
    });
    let mut editor = match restored {
        Ok(editor) => {
            log::warn!("Restored unsaved changes to {file_path:?} from a previous run");
            editor
        }
        Err(e) => {
            log::warn!(
                "Could not reopen {file_path:?} with its unsaved changes ({e:#}); \
                 the changes open in a new editor"
            );
            let mut editor = Editor::with_config(editor_config);
            editor.insert_text(&changes).ok()?;
            editor
        }
    };
    editor.set_unsaved_buffer_file(Some(buffer_file));
    Some(editor)
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
    project_root: &Path,
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
                restore_file_editor(file_path, unsaved_buffer_file, project_dir, editor_config)
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
        PanelState::Csv { path } => Some(Box::new(termide_panel_csv::CsvPanel::new(path))),
        PanelState::Html { path } => termide_panel_html::HtmlPanel::new(path)
            .ok()
            .map(|p| Box::new(p) as Box<dyn Panel + Send>),
        PanelState::GitStatus { repo_path } => Some(Box::new(
            termide_panel_git_status::GitStatusPanel::new_for_repo(repo_path, Some(project_root)),
        )),
        PanelState::GitLog { repo_path } => Some(Box::new(
            termide_panel_git_log::GitLogPanel::new_for_repo(repo_path, Some(project_root)),
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
        } => crate::app::agent_panel::restore_agent_panel(
            agent_settings,
            project_root,
            cwd,
            session,
            agent,
            setup,
        )
        .map(|p| Box::new(p) as Box<dyn Panel + Send>),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_core::{PanelEvent, RenderContext};

    /// A panel with no saved form, like a plugin panel.
    struct UnsavedPanel;

    impl Panel for UnsavedPanel {
        fn name(&self) -> &'static str {
            "unsaved"
        }
        fn title(&self) -> String {
            "unsaved".to_string()
        }
        fn render(&mut self, _area: Rect, _buf: &mut Buffer, _ctx: &RenderContext) {}
        fn handle_key(&mut self, _chord: termide_core::KeyChord) -> Vec<PanelEvent> {
            vec![]
        }
    }

    fn group_state(
        panels: Vec<PanelState>,
        expanded_index: usize,
        split_heights: Option<Vec<u16>>,
    ) -> PanelGroupState {
        PanelGroupState {
            panels,
            expanded_index,
            width: None,
            mode: GroupLayoutMode::default(),
            split_heights,
            fullscreen_cache: None,
        }
    }

    #[test]
    fn restored_widths_stay_within_the_terminal() {
        assert_eq!(restored_width(None, 120), None);
        assert_eq!(restored_width(Some(40), 120), Some(40));
        assert_eq!(restored_width(Some(u16::MAX), 120), Some(120));
    }

    #[test]
    fn remap_index_follows_the_kept_items() {
        assert_eq!(remap_index(&[true, false, true], 2), Some(1));
        assert_eq!(remap_index(&[true, false, true], 1), Some(0));
        assert_eq!(remap_index(&[false, true], 0), Some(0));
        assert_eq!(remap_index(&[true, true], 9), Some(1));
        assert_eq!(remap_index(&[false, false], 0), None);
    }

    /// A panel without a saved form drops out of the layout; the focus and
    /// the heights of the others stay with them, and a group left empty
    /// takes the group focus along.
    #[test]
    fn saving_leaves_unsaved_panels_out_in_step_with_focus_and_heights() {
        let mut manager = LayoutManager::new();
        let mut first = PanelGroup::new(Box::new(JournalPanel::default()));
        first.add_panel(Box::new(UnsavedPanel));
        first.add_panel(Box::new(JournalPanel::default()));
        first.set_split_heights(vec![5, 10, 15]);
        first.set_expanded(2);
        manager.panel_groups.push(first);
        manager
            .panel_groups
            .push(PanelGroup::new(Box::new(UnsavedPanel)));
        manager
            .panel_groups
            .push(PanelGroup::new(Box::new(JournalPanel::default())));
        manager.focus = 2;

        let tmp = tempfile::tempdir().unwrap();
        let layout = manager.to_state(tmp.path());

        assert_eq!(layout.panel_groups.len(), 2);
        assert_eq!(layout.focused_group, 1);
        let saved = &layout.panel_groups[0];
        assert_eq!(saved.panels, vec![PanelState::Journal, PanelState::Journal]);
        assert_eq!(saved.expanded_index, 1);
        assert_eq!(saved.split_heights, Some(vec![5, 15]));
    }

    /// A saved panel that cannot be rebuilt drops out on restore with the
    /// same remapping, and so does a group none of whose panels come back.
    #[test]
    fn restoring_remaps_focus_and_heights_past_panels_that_fail() {
        let gone = || PanelState::Editor {
            path: None,
            unsaved_buffer_file: None,
        };
        let layout = ProjectLayout {
            panel_groups: vec![
                group_state(
                    vec![gone(), PanelState::Journal, PanelState::Journal],
                    2,
                    Some(vec![3, 7, 20]),
                ),
                group_state(vec![gone()], 0, None),
                group_state(vec![PanelState::Journal], 0, None),
            ],
            focused_group: 2,
        };
        let tmp = tempfile::tempdir().unwrap();
        let manager = LayoutManager::from_layout(
            layout,
            tmp.path(),
            tmp.path(),
            32,
            80,
            EditorConfig::default(),
            AiSettings::default(),
        )
        .unwrap();

        assert_eq!(manager.panel_groups.len(), 2);
        assert_eq!(manager.focus, 1);
        let group = &manager.panel_groups[0];
        assert_eq!(group.panels().len(), 2);
        assert_eq!(group.expanded_index(), 1);
        assert_eq!(group.split_heights(), Some(&[7, 20][..]));
    }

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

    /// A saved editor with a backup reopens its file with the changes over
    /// it; with the file gone, the changes still open, in an editor without
    /// a file.
    #[test]
    fn a_file_editor_reopens_with_its_unsaved_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, "on disk\n").unwrap();
        let project_dir = tmp.path().join("state");
        termide_project::save_unsaved_buffer(&project_dir, "unsaved-1.txt", "edited\n").unwrap();
        let reopen = |path: &Path| {
            restore_file_editor(
                path.to_path_buf(),
                Some("unsaved-1.txt".into()),
                &project_dir,
                EditorConfig::default(),
            )
            .unwrap()
        };

        let editor = reopen(&file);
        assert_eq!(editor.file_path(), Some(file.as_path()));
        assert_eq!(editor.buffer().text(), "edited\n");
        assert!(editor.buffer_is_modified());
        assert_eq!(editor.unsaved_buffer_file(), Some("unsaved-1.txt"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "on disk\n");

        let editor = reopen(&tmp.path().join("gone.txt"));
        assert_eq!(editor.file_path(), None);
        assert_eq!(editor.buffer().text(), "edited\n");
        assert_eq!(editor.unsaved_buffer_file(), Some("unsaved-1.txt"));
    }
}
