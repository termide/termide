//! Async git status tracking and application for file manager entries.

use std::collections::HashSet;

use termide_git::{get_git_status_async, GitStatus};

use super::{sort_entries, tree, FileEntry, FileManager};

impl FileManager {
    /// Check for async git status results and update entries if available.
    /// Returns true if git status was updated.
    pub fn check_git_status_async(&mut self) -> bool {
        let result = if let Some(ref rx) = self.git_status_receiver {
            rx.try_recv().ok()
        } else {
            None
        };

        if let Some(git_result) = result {
            // Verify the result is for the current directory
            if git_result.dir == self.current_path {
                self.git_status_cache = git_result.cache;
                self.git_status_receiver = None;

                // Re-apply git statuses to entries
                self.apply_git_statuses();
                return true;
            }
            // Result is for a different directory - discard it
            self.git_status_receiver = None;
        }
        false
    }

    /// Refresh git status without full directory reload.
    /// Used when git watcher detects repository changes (e.g., after commits).
    pub(crate) fn refresh_git_status(&mut self) {
        // Start async git status request for current directory
        self.git_status_receiver = Some(get_git_status_async(self.current_path.clone()));
    }

    /// Apply git statuses from cache to entries
    pub(crate) fn apply_git_statuses(&mut self) {
        let current_path = self.current_path.clone();
        for te in &mut self.tree_entries {
            if te.file_entry.name == ".." {
                continue;
            }

            // For nested entries, compute relative path from panel's current_path
            let git_name = if te.depth == 0 {
                te.file_entry.name.clone()
            } else {
                te.full_path
                    .strip_prefix(&current_path)
                    .ok()
                    .and_then(|p| p.to_str())
                    .unwrap_or(&te.file_entry.name)
                    .to_string()
            };

            te.file_entry.git_status = if te.file_entry.is_dir {
                self.git_status_cache
                    .as_ref()
                    .map(|cache| cache.get_directory_status(&git_name))
                    .unwrap_or(GitStatus::Unmodified)
            } else {
                self.git_status_cache
                    .as_ref()
                    .map(|cache| cache.get_status(&git_name))
                    .unwrap_or(GitStatus::Unmodified)
            };
        }

        // Also add deleted files that weren't in the directory listing
        if let Some(cache) = &self.git_status_cache {
            let deleted_files = cache.get_deleted_files();
            if !deleted_files.is_empty() {
                let existing_names: HashSet<String> = self
                    .tree_entries
                    .iter()
                    .filter(|te| te.depth == 0)
                    .map(|te| te.file_entry.name.clone())
                    .collect();

                let new_entries: Vec<tree::TreeEntry> = deleted_files
                    .into_iter()
                    .filter(|deleted_name| !existing_names.contains(deleted_name))
                    .map(|deleted_name| {
                        let full_path = self.current_path.join(&deleted_name);
                        tree::TreeEntry {
                            file_entry: FileEntry {
                                name: deleted_name,
                                os_name: None,
                                is_dir: false,
                                is_symlink: false,
                                is_executable: false,
                                is_readonly: false,
                                git_status: GitStatus::Deleted,
                                size: None,
                                modified: None,
                            },
                            full_path,
                            depth: 0,
                            expanded: None,
                            is_loading: false,
                        }
                    })
                    .collect();

                if !new_entries.is_empty() {
                    // Insert deleted files among top-level entries and re-sort
                    // Find the end of depth-0 entries to insert before any expanded children
                    self.tree_entries.extend(new_entries);
                    // Re-sort only depth-0 entries while preserving subtree structure
                    // For simplicity, rebuild entire tree
                    self.rebuild_with_expanded_subtrees();
                }
            }
        }
    }

    /// Rebuild tree_entries preserving expanded state (used after adding deleted files).
    pub(crate) fn rebuild_with_expanded_subtrees(&mut self) {
        // Extract top-level entries, sort them, then re-expand
        let mut top_entries: Vec<FileEntry> = self
            .tree_entries
            .iter()
            .filter(|te| te.depth == 0)
            .map(|te| te.file_entry.clone())
            .collect();
        sort_entries(&mut top_entries);
        self.tree_entries = self.build_top_level_tree(top_entries);
        self.load_expanded_subtrees();
        self.recompute_visible();
    }

    /// Check if git status is still loading
    pub fn is_git_status_loading(&self) -> bool {
        self.git_status_receiver.is_some()
    }
}
