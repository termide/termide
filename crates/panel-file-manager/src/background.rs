//! Per-tick background work for the file manager: draining async VFS/git/search
//! results and running the directory-size walk scheduler.

use std::sync::mpsc;

use termide_core::PanelEvent;

use crate::{utils, FileManager};

impl FileManager {
    /// Per-tick background poll. Trait `Panel::tick` delegates here.
    pub(crate) fn on_tick(&mut self) -> Vec<PanelEvent> {
        // --- Always drain async results (even when stale/collapsed) ---
        // VFS and git status receivers must be consumed to prevent stuck spinners.
        // IMPORTANT: never early-return before vfs.tick() — results must always be drained.

        let mut events = Vec::new();

        // Drain any tree-expand list_dir operations that have resolved.
        if self.poll_pending_expansions() {
            events.push(PanelEvent::NeedsRedraw);
        }

        // Check for VFS connection timeout (cancel stuck connections)
        if let Some((status, Some(secs))) = self.vfs.connection_status_with_elapsed() {
            // Opening an archive is local work that can legitimately take a
            // while for a big compressed tar; Esc still cancels it.
            if secs >= self.cached_vfs_timeout_secs && !self.vfs.current_path().is_archive() {
                log::warn!("VFS connection timeout after {}s", secs);
                if self.vfs.cancel_pending().is_some() {
                    self.current_path = self.vfs.path_buf();
                    let _ = self.load_directory();
                    if !self.is_stale {
                        let t = termide_i18n::t();
                        self.show_info_modal(
                            t.connection_timeout_title(),
                            t.connection_timeout_message(),
                        );
                        events.push(PanelEvent::ClearStatus);
                        events.push(PanelEvent::NeedsRedraw);
                        return events;
                    }
                }
            } else if !self.is_stale {
                // Show connection progress in status bar (no early return — must reach vfs.tick)
                events.push(PanelEvent::ShowMessage(format!("{} {}s", status, secs)));
            }
        }

        // Poll VFS operations for completion
        if let Some(result) = self.vfs.tick() {
            match result {
                Ok(entries) => {
                    self.current_path = self.vfs.path_buf();
                    self.update_entries_from_vfs(entries);
                }
                Err(e) => {
                    log::error!("VFS operation failed: {}", e);
                    // Sync to the path `vfs` restored on failure, but do NOT
                    // reload: the listing operation just failed, and reloading
                    // starts another one. On a dead remote session every retry
                    // fails identically, so an auto-reload here spins an
                    // infinite error→reload→error loop (a new alert per tick).
                    // The previous listing is still shown (remote entries aren't
                    // cleared on failure), so the panel stays consistent; the
                    // user reconnects/refreshes explicitly.
                    self.current_path = self.vfs.path_buf();
                    if !self.is_stale {
                        if e.is_archive_error() {
                            let t = termide_i18n::t();
                            self.show_info_modal(t.modal_error_title(), &format!("{}", e));
                        } else if self.vfs.is_remote() && e.is_connection_lost() {
                            // Dead remote session — offer Reconnect / open local /
                            // close instead of a dead-end "OK".
                            self.show_connection_error_modal(&format!("{}", e));
                        } else {
                            let t = termide_i18n::t();
                            self.show_info_modal(t.connection_error_title(), &format!("{}", e));
                        }
                    }
                }
            }
            if !self.is_stale {
                events.push(PanelEvent::ClearStatus);
                events.push(PanelEvent::NeedsRedraw);
                return events;
            }
        }

        // A remote create finished — reveal the new entry and reload, the work
        // `create_file` / `create_directory` used to do inline while the UI
        // thread sat on the round-trip.
        if let Some(result) = self.vfs.take_completed_create() {
            match result {
                Ok((path, is_dir)) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.navigation.set_newly_created_path(path);
                    let _ = self.load_directory();
                    if !self.is_stale {
                        let t = termide_i18n::t();
                        let msg = if is_dir {
                            t.status_dir_created(&name)
                        } else {
                            t.status_file_created(&name)
                        };
                        events.push(PanelEvent::ShowMessage(msg));
                        events.push(PanelEvent::NeedsRedraw);
                    }
                    return events;
                }
                Err(e) => {
                    log::error!("remote create failed: {}", e);
                    if !self.is_stale {
                        let t = termide_i18n::t();
                        self.show_info_modal(t.connection_error_title(), &format!("{}", e));
                        events.push(PanelEvent::ClearStatus);
                        events.push(PanelEvent::NeedsRedraw);
                        return events;
                    }
                }
            }
        }

        // An archive needs its password: ask for it.
        if let Some((archive, wrong)) = self.vfs.take_password_request() {
            self.request_archive_password(archive, wrong);
            events.push(PanelEvent::ClearStatus);
            events.push(PanelEvent::NeedsRedraw);
            return events;
        }

        // A remote login was refused: the app supplies a password, from
        // the vault or the user.
        if let Some((url, attempt)) = self.vfs.take_credential_request() {
            events.push(PanelEvent::ClearStatus);
            events.push(PanelEvent::CredentialsRequired { url, attempt });
            events.push(PanelEvent::NeedsRedraw);
            return events;
        }
        if let Some(url) = self.vfs.take_accepted_login() {
            events.push(PanelEvent::CredentialsAccepted { url });
        }

        // A remote symlink resolved to a file — open it in the editor.
        if let Some(remote) = self.vfs.take_resolved_file_open() {
            events.push(PanelEvent::ClearStatus);
            events.push(PanelEvent::OpenRemoteFile(remote.to_url_string()));
            events.push(PanelEvent::NeedsRedraw);
            return events;
        }

        // Drain git status receiver — redraw if statuses changed
        if self.check_git_status_async() && !self.is_stale {
            events.push(PanelEvent::NeedsRedraw);
        }

        // Poll file search results
        let mut search_updated = false;
        if let Some(ref mut search) = self.file_search {
            if search.poll_results() {
                search_updated = true;
                events.push(PanelEvent::NeedsRedraw);
            }
        }
        if search_updated {
            // Refresh the bar counter now that results (and their count) landed.
            self.sync_bar_status();
        }

        // Skip remaining work when collapsed (stale)
        if self.is_stale {
            return vec![];
        }

        // Directory size scheduler. Each panel runs at most one worker,
        // but all panels share the process-wide cache, so overlapping
        // directories are computed once.
        if self.cached_config.dir_size_in_wide_view && self.cached_config.dir_size_budget_ms > 0 {
            let cache = utils::shared_dir_size_cache();

            // 1. Drain the completion signal for our own worker, if any.
            if let Some((_, rx)) = self.dir_size_pending.as_ref() {
                match rx.try_recv() {
                    Ok(()) => {
                        self.dir_size_pending = None;
                        events.push(PanelEvent::NeedsRedraw);
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.dir_size_pending = None;
                    }
                }
            }

            // 2. Pick up results other panels have just produced.
            let gen = cache.generation();
            if gen != self.dir_size_cache_generation {
                self.dir_size_cache_generation = gen;
                events.push(PanelEvent::NeedsRedraw);
            }

            // 3. Start the next walk if we have no worker in flight.
            if self.dir_size_pending.is_none() {
                // Top up the queue lazily: any visible directory that is
                // either missing from the cache or marked stale needs
                // (re)computing. Stale entries keep their old value
                // visible while they wait in the queue.
                if self.dir_size_queue.is_empty() {
                    for te in &self.tree_entries {
                        if te.file_entry.is_dir && te.file_entry.name != ".." {
                            let path = &te.full_path;
                            if cache.get(path).is_none() || cache.is_stale(path) {
                                self.dir_size_queue.push_back(path.clone());
                            }
                        }
                    }
                }

                while let Some(path) = self.dir_size_queue.pop_front() {
                    match cache.claim(&path) {
                        utils::ClaimOutcome::AlreadyCached => {
                            // Sibling panel populated this while we waited;
                            // the generation bump above will trigger a redraw.
                            continue;
                        }
                        utils::ClaimOutcome::InProgress => {
                            // Another panel owns this walk — defer and try
                            // again next tick. Breaking avoids spinning on
                            // a full queue of contended paths.
                            self.dir_size_queue.push_back(path);
                            break;
                        }
                        utils::ClaimOutcome::Claimed => {
                            let budget = std::time::Duration::from_millis(
                                self.cached_config.dir_size_budget_ms,
                            );
                            let (tx, rx) = mpsc::channel();
                            let worker_path = path.clone();
                            std::thread::spawn(move || {
                                let outcome =
                                    utils::calculate_dir_size_bounded(&worker_path, budget);
                                utils::shared_dir_size_cache().complete(worker_path, outcome);
                                let _ = tx.send(());
                            });
                            self.dir_size_pending = Some((path, rx));
                            break;
                        }
                    }
                }
            }
        }

        events
    }
}
