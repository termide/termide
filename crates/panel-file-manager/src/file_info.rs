use std::fs;
use std::sync::mpsc;

use super::{utils, FileManager};
use termide_modal::{ActionButton, ActiveModal};
use termide_state::{DirSizeResult, PendingAction};
use termide_ui::system_monitor::DiskSpaceInfo;

/// File information for display
#[derive(Clone, Debug)]
pub struct FileInfo {
    pub name: String,
    pub file_type: String,
    pub size: String,
    pub owner: String,
    pub group: String,
    pub mode: String, // Access permissions in format "0755"
    pub symlink_target: Option<String>,
    /// Whether the effective target is a directory (follows symlinks)
    pub target_is_dir: bool,
}

impl FileManager {
    /// Get information about the currently selected file
    pub fn get_current_file_info(&mut self) -> Option<FileInfo> {
        let te = self.tree_entry_at(self.selected)?;
        let entry = &te.file_entry;

        // Handle ".." directory for remote paths
        if entry.name == ".." && self.is_remote() {
            return Some(FileInfo {
                name: "..".to_string(),
                file_type: "Directory".to_string(),
                size: "DIR".to_string(),
                owner: "remote".to_string(),
                group: "remote".to_string(),
                mode: "????".to_string(),
                symlink_target: None,
                target_is_dir: true,
            });
        }

        // For remote files, use FileEntry metadata directly (from VfsEntry)
        if self.is_remote() {
            let file_type = if entry.is_dir {
                "Directory"
            } else if entry.is_symlink {
                "Symlink"
            } else {
                "File"
            };

            let size = if entry.is_dir {
                "DIR".to_string()
            } else {
                entry
                    .size
                    .map(utils::format_size)
                    .unwrap_or_else(|| "Unknown".to_string())
            };

            let mode = if entry.is_executable {
                "0755".to_string()
            } else if entry.is_readonly {
                "0444".to_string()
            } else {
                "0644".to_string()
            };

            return Some(FileInfo {
                name: entry.name.clone(),
                file_type: file_type.to_string(),
                size,
                owner: "remote".to_string(),
                group: "remote".to_string(),
                mode,
                symlink_target: None,
                target_is_dir: entry.is_dir,
            });
        }

        // Local file handling — use full_path so nested tree entries resolve correctly
        let file_path = if entry.name == ".." {
            self.current_path
                .parent()
                .unwrap_or(&self.current_path)
                .to_path_buf()
        } else {
            te.full_path.clone()
        };

        let symlink_metadata = fs::symlink_metadata(&file_path).ok()?;

        let file_type = if symlink_metadata.is_dir() {
            "Directory"
        } else if symlink_metadata.is_symlink() {
            "Symlink"
        } else {
            "File"
        };

        let symlink_target = if symlink_metadata.is_symlink() {
            fs::read_link(&file_path)
                .ok()
                .map(|p| termide_core::util::shorten_home_path(&p.display().to_string()))
        } else {
            None
        };

        // For size/owner/group/mode, follow the symlink
        let metadata = fs::metadata(&file_path).unwrap_or(symlink_metadata);

        let target_is_dir = metadata.is_dir();

        // Files: formatted byte size. Directories: computed further below
        // (recursive size from the shared cache + immediate child count).
        let file_size = if target_is_dir {
            None
        } else {
            Some(utils::format_size(metadata.len()))
        };

        #[cfg(unix)]
        let (owner, group, mode) = {
            use std::os::unix::fs::MetadataExt;
            (
                utils::get_user_name(metadata.uid()),
                utils::get_group_name(metadata.gid()),
                format!("{:04o}", metadata.mode() & 0o7777),
            )
        };

        #[cfg(not(unix))]
        let (owner, group, mode) = {
            let owner = std::env::var("USERNAME").unwrap_or_else(|_| "owner".to_string());
            let mode = if metadata.permissions().readonly() {
                "0444".to_string()
            } else {
                "0644".to_string()
            };
            (owner, String::new(), mode)
        };

        // Capture the name before touching `self` mutably below (this is the
        // last read through the `te`/`entry` borrow).
        let name = entry.name.clone();

        // Size string. Files: the plain byte size. Directories: the recursive
        // size (from the shared cache the size-scheduler / a prior info-modal
        // populates; `…` until known) plus the immediate, non-recursive child
        // count in parentheses — matching the info modal, e.g. "3.0 KB (5 items)".
        let size = if target_is_dir {
            let count = match &self.dir_item_count_memo {
                // Memoized by path so repeated redraws on the same folder don't
                // re-read the directory.
                Some((p, n)) if *p == file_path => *n,
                _ => {
                    let n = fs::read_dir(&file_path).map(|rd| rd.count()).unwrap_or(0);
                    self.dir_item_count_memo = Some((file_path.clone(), n));
                    n
                }
            };
            let t = termide_i18n::t();
            let size_part = match utils::shared_dir_size_cache().get(&file_path) {
                Some(o) if o.overflowed => format!("{}+", utils::format_size(o.size)),
                Some(o) => utils::format_size(o.size),
                None => "…".to_string(),
            };
            format!("{} ({})", size_part, t.file_info_items(count))
        } else {
            file_size.unwrap_or_default()
        };

        Some(FileInfo {
            name,
            file_type: file_type.to_string(),
            size,
            owner,
            group,
            mode,
            symlink_target,
            target_is_dir,
        })
    }

    /// Show file/directory information (Space)
    pub(crate) fn show_file_info(&mut self) {
        use std::time::SystemTime;

        // Clone the data we need to avoid borrow issues with self
        let te = match self.tree_entry_at(self.selected) {
            Some(te) => te.clone(),
            None => return,
        };
        let entry = &te.file_entry;

        // Handle remote file info display
        if self.is_remote() {
            let t = termide_i18n::t();

            // Determine type and title
            let (modal_title, is_dir) = if entry.is_dir {
                (t.file_info_title_directory(&entry.name), true)
            } else if entry.is_symlink {
                (t.file_info_title_symlink(&entry.name), false)
            } else {
                (t.file_info_title_file(&entry.name), false)
            };

            let size = if is_dir {
                "DIR".to_string()
            } else {
                entry
                    .size
                    .map(utils::format_size)
                    .unwrap_or_else(|| "Unknown".to_string())
            };

            let modified = entry
                .modified
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| {
                    chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| {
                            dt.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_else(|| "Unknown".to_string())
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let mode = if entry.is_executable {
                "0755"
            } else if entry.is_readonly {
                "0444"
            } else {
                "0644"
            };

            // Collect data for remote file (no git status)
            let data = vec![
                (t.file_info_path().to_string(), self.display_path()),
                (t.file_info_size().to_string(), size),
                (t.file_info_owner().to_string(), "remote".to_string()),
                (t.file_info_group().to_string(), "remote".to_string()),
                (t.file_info_modified().to_string(), modified),
                ("Mode".to_string(), mode.to_string()),
            ];

            let modal = termide_modal::InfoModal::new(modal_title, data);
            self.modal_request = Some((
                PendingAction::ClosePanel,
                ActiveModal::Info(Box::new(modal)),
            ));

            return;
        }

        // Local file handling
        let file_path = te.full_path.clone();

        // Use symlink_metadata to detect symlinks before following them
        let symlink_meta = fs::symlink_metadata(&file_path).ok();
        let is_symlink = symlink_meta
            .as_ref()
            .map(|m| m.is_symlink())
            .unwrap_or(false);

        if let Ok(metadata) = fs::metadata(&file_path) {
            let t = termide_i18n::t();

            // Determine type and title (use symlink_metadata for type detection)
            let (modal_title, is_dir) = if is_symlink {
                (t.file_info_title_symlink(&entry.name), metadata.is_dir())
            } else if metadata.is_dir() {
                (t.file_info_title_directory(&entry.name), true)
            } else {
                (t.file_info_title_file(&entry.name), false)
            };

            // Immediate (non-recursive) child count, shown in parentheses on
            // the size line. Cheap read_dir; the recursive byte total is
            // computed asynchronously below and patched in later.
            let dir_item_count = if is_dir {
                fs::read_dir(&file_path).map(|rd| rd.count()).ok()
            } else {
                None
            };

            let size = if is_dir {
                match dir_item_count {
                    Some(n) => {
                        format!(
                            "{}... ({})",
                            t.file_info_calculating(),
                            t.file_info_items(n)
                        )
                    }
                    None => format!("{}...", t.file_info_calculating()),
                }
            } else {
                utils::format_size(metadata.len())
            };

            #[cfg(unix)]
            let (owner, group, file_mode, perm_access) = {
                use std::os::unix::fs::MetadataExt;
                use termide_modal::PermAccess;
                let euid = unsafe { libc::geteuid() };
                let file_uid = metadata.uid();
                let access = if euid == 0 {
                    PermAccess::Root
                } else if euid == file_uid {
                    PermAccess::Owner
                } else {
                    PermAccess::ReadOnly
                };
                (
                    utils::get_user_name(file_uid),
                    utils::get_group_name(metadata.gid()),
                    Some(metadata.mode()),
                    access,
                )
            };

            #[cfg(not(unix))]
            let (owner, group, file_mode, perm_access) = {
                let owner = std::env::var("USERNAME").unwrap_or_else(|_| "owner".to_string());
                (
                    owner,
                    String::new(),
                    None::<u32>,
                    termide_modal::PermAccess::ReadOnly,
                )
            };

            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| {
                    chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| {
                            dt.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_else(|| "Unknown".to_string())
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let created = metadata
                .created()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| {
                    chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                        .map(|dt| {
                            dt.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_else(|| "Unknown".to_string())
                })
                .unwrap_or_else(|| "Unknown".to_string());

            // Collect data without Name and Type
            let mut data = vec![(
                t.file_info_path().to_string(),
                termide_core::util::shorten_home_path(&file_path.display().to_string()),
            )];

            if is_symlink {
                if let Ok(target) = fs::read_link(&file_path) {
                    data.push((
                        t.file_info_target().to_string(),
                        termide_core::util::shorten_home_path(&target.display().to_string()),
                    ));
                }
            }

            data.extend([
                (t.file_info_size().to_string(), size),
                (t.file_info_owner().to_string(), owner),
                (t.file_info_group().to_string(), group),
                (t.file_info_created().to_string(), created),
                (t.file_info_modified().to_string(), modified),
            ]);

            // Add git status if in repository (filtered by specific file/directory)
            // Special case: if directory is itself a git repo root, show its git info
            let (git_status, repo_path) = if is_dir && file_path.join(".git").exists() {
                // Directory is a git repo root - get its own status
                (
                    termide_git::get_repo_status(&file_path, &file_path),
                    Some(file_path.clone()),
                )
            } else {
                // Regular file/directory - check status in parent repo
                let repo = termide_git::find_repo_root(&self.current_path);
                (
                    termide_git::get_repo_status(&self.current_path, &file_path),
                    repo,
                )
            };

            // Track whether file has actionable git status (any git actions available)
            let has_git_actions = git_status
                .as_ref()
                .map(|s| {
                    !s.is_ignored && (s.uncommitted_changes > 0 || s.ahead > 0 || s.behind > 0)
                })
                .unwrap_or(false);

            if let Some(ref git_status) = git_status {
                if git_status.is_ignored {
                    // If file is ignored, show only one line
                    data.push((
                        t.file_info_git().to_string(),
                        t.file_info_git_ignored().to_string(),
                    ));
                } else {
                    // Otherwise show three lines for uncommitted, ahead, behind
                    data.push((
                        t.file_info_git().to_string(),
                        t.file_info_git_uncommitted(git_status.uncommitted_changes),
                    ));
                    data.push((
                        String::new(), // Empty key - aligns with first line's value
                        t.file_info_git_ahead(git_status.ahead),
                    ));
                    data.push((
                        String::new(), // Empty key
                        t.file_info_git_behind(git_status.behind),
                    ));
                }
            }

            // Helper: attach permissions widget to modal if available
            let attach_permissions =
                |mut modal: termide_modal::InfoActionModal| -> termide_modal::InfoActionModal {
                    if let Some(mode) = file_mode {
                        modal = modal.with_permissions(mode, perm_access);
                    }
                    modal
                };

            // If file has git actions, use InfoActionModal with smart buttons
            if let (true, Some(ref status), Some(repo)) = (has_git_actions, &git_status, repo_path)
            {
                let buttons = Self::build_git_action_buttons(status, is_dir);
                let selected_button = buttons.len().saturating_sub(1); // Select [Close]
                let modal = attach_permissions(
                    termide_modal::InfoActionModal::new(modal_title, data.clone(), buttons)
                        .with_selected_button(selected_button),
                );
                self.modal_request = Some((
                    PendingAction::GitFileAction {
                        file_path: file_path.clone(),
                        repo_path: repo,
                        is_staged: false, // File manager shows unstaged files
                    },
                    ActiveModal::InfoAction(Box::new(modal)),
                ));
            } else if is_symlink {
                // Symlink without git actions — show "Follow symlink" button
                let target_path =
                    fs::canonicalize(&file_path).unwrap_or_else(|_| file_path.clone());
                let buttons = vec![
                    ActionButton::new(t.file_info_follow_symlink(), "follow"),
                    ActionButton::new(t.git_action_close(), "close"),
                ];
                let modal = attach_permissions(
                    termide_modal::InfoActionModal::new(modal_title, data.clone(), buttons)
                        .with_selected_button(1),
                );
                self.modal_request = Some((
                    PendingAction::FollowSymlink { target_path },
                    ActiveModal::InfoAction(Box::new(modal)),
                ));
            } else if file_mode.is_some() {
                // Regular file/dir with permissions editor
                let buttons = vec![ActionButton::new(t.git_action_close(), "close")];
                let modal = attach_permissions(
                    termide_modal::InfoActionModal::new(modal_title, data.clone(), buttons)
                        .with_selected_button(0),
                );
                self.modal_request = Some((
                    PendingAction::ChangePermissions {
                        file_path: file_path.clone(),
                    },
                    ActiveModal::InfoAction(Box::new(modal)),
                ));
            } else {
                let modal = termide_modal::InfoModal::new(modal_title, data);
                self.modal_request = Some((
                    PendingAction::ClosePanel,
                    ActiveModal::Info(Box::new(modal)),
                ));
            }

            if is_dir {
                let (tx, rx) = mpsc::channel();
                let cache_path = file_path;

                std::thread::spawn(move || {
                    let size = utils::calculate_dir_size(&cache_path);
                    // The modal triggers an unbounded walk, so the result
                    // is the exact size. Publish it to the shared dir-size
                    // cache so any panel showing this directory in the wide
                    // view picks it up immediately instead of re-running
                    // its own budgeted walk.
                    utils::shared_dir_size_cache().insert(
                        cache_path,
                        utils::DirSizeOutcome {
                            size,
                            overflowed: false,
                        },
                    );
                    let _ = tx.send(DirSizeResult {
                        size,
                        item_count: dir_item_count,
                    });
                });

                self.dir_size_receiver = Some(rx);
            }
        }
    }

    /// Build action buttons for git info modal
    fn build_git_action_buttons(
        _git_status: &termide_git::GitRepoStatus,
        is_dir: bool,
    ) -> Vec<ActionButton> {
        let t = termide_i18n::t();

        let mut buttons = Vec::new();
        if !is_dir {
            buttons.push(ActionButton::new(t.git_action_edit(), "edit"));
        }
        buttons.push(ActionButton::new(t.git_action_close(), "close"));
        buttons
    }

    /// Get disk space information for the current directory.
    pub fn get_disk_space_info(&self) -> Option<DiskSpaceInfo> {
        // Don't show disk info during VFS connection (status bar should show connection status)
        if self.vfs.has_pending_operation() {
            return None;
        }
        termide_system_monitor::get_disk_space_info(&self.current_path)
    }
}
