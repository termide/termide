//! Async commit-log refresh: the branch list on a worker, the commits from a
//! [`LogStream`] read as far as the view needs, and state reset.

use std::collections::HashSet;
use std::path::PathBuf;

use termide_git::{self as git, CommitInfo, LogChunk, LogStream};

use crate::GitLogPanel;

/// Commits asked for at a time while scrolling.
const PAGE: usize = 500;
/// Commits asked for at a time while reading to the end (`End`).
const END_PAGE: usize = 5000;
/// Read ahead while fewer than this many screens are left below the view.
const AHEAD_SCREENS: usize = 2;

/// What a log stream shows: the repository, the branch (`None` = HEAD) and
/// the graph style. A refresh of the same view keeps the reader's place.
pub(crate) type LogView = (PathBuf, Option<String>, bool);

/// A restarted stream of the view already shown: the old rows stay up until
/// the new ones arrive, then the selected commit is found again.
pub(crate) struct Reload {
    /// The selected commit's hash, if a commit (not a connector) was selected.
    hash: Option<String>,
    /// How far below the top of the view the selection sat.
    offset: usize,
}

/// Snapshot returned by the background refresh worker.
pub(crate) struct GitLogRefreshResult {
    pub(crate) branch: Option<String>,
    pub(crate) branches: Vec<String>,
    pub(crate) worktrees: HashSet<String>,
}

impl GitLogPanel {
    /// Trigger a refresh of the commit log.
    ///
    /// Returns immediately; the `get_all_branches` and
    /// `get_log_with_graph` git commands run on a worker thread and
    /// `tick()` folds the result in via [`Self::poll_refresh`].
    /// Reset displayed log state to empty — used when no repository is selected
    /// (e.g. the current repo's `.git` was deleted) so stale commits/branch
    /// don't linger.
    pub(crate) fn clear_git_state(&mut self) {
        self.branch = None;
        self.branches.clear();
        self.worktrees.clear();
        self.selected_branch = None;
        self.log = None;
        self.log_view = None;
        self.reload = None;
        self.reset_rows();
    }

    /// Forget the rows read so far.
    fn reset_rows(&mut self) {
        self.commits.clear();
        self.loaded_commits = 0;
        self.graph_width = 0;
        self.selected = 0;
        self.scroll = 0;
        self.follow_end = false;
        self.scroll_target = None;
    }

    pub fn refresh(&mut self) {
        // Coalesce: a worker is already running, so mark that one more pass is
        // needed when it finishes rather than spawning a parallel worker (and a
        // fresh batch of git subprocesses) for every queued event.
        if self.refresh_rx.is_some() {
            self.refresh_pending = true;
            return;
        }
        let Some(repo) = self.repo_manager.current() else {
            self.clear_git_state();
            return;
        };
        let repo = repo.to_path_buf();

        let (tx, rx) = std::sync::mpsc::channel();
        self.refresh_rx = Some(rx);
        std::thread::spawn(move || {
            let branch = git::get_current_branch(&repo);
            let list = git::get_branch_list(&repo);
            let worktrees = git::linked_worktrees(&repo, &list).into_keys().collect();
            let branches = list.into_iter().map(|b| b.name).collect();
            let _ = tx.send(GitLogRefreshResult {
                branch,
                branches,
                worktrees,
            });
        });
    }

    /// Apply an async refresh result if one is ready. Returns `true`
    /// when the panel state changed so the caller can emit
    /// `NeedsRedraw`.
    pub(crate) fn poll_refresh(&mut self) -> bool {
        let Some(rx) = self.refresh_rx.as_ref() else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.refresh_rx = None;
                self.run_pending_refresh();
                return true;
            }
        };
        self.refresh_rx = None;

        self.branch = result.branch;
        self.branches = result.branches;
        self.worktrees = result.worktrees;

        // If the previously selected branch no longer exists, reset to HEAD
        if let Some(ref b) = self.selected_branch {
            if !self.branches.contains(b) {
                self.selected_branch = None;
            }
        }

        self.restart_log();
        self.run_pending_refresh();
        true
    }

    /// Start reading the history anew. The same view keeps its rows until the
    /// new ones arrive and reads at least as far as it had, so a watcher
    /// refresh neither blanks the list nor loses the reader's place; another
    /// repository or branch starts at the top.
    fn restart_log(&mut self) {
        let Some(repo) = self.repo_manager.current() else {
            self.clear_git_state();
            return;
        };
        let view: LogView = (
            repo.to_path_buf(),
            self.selected_branch.clone(),
            self.unicode_graph,
        );
        let stream = LogStream::start(&view.0, view.1.as_deref(), view.2);
        if self.log_view.as_ref() == Some(&view) {
            stream.request(self.loaded_commits.max(PAGE));
            self.reload = Some(Reload {
                hash: self
                    .selected_commit()
                    .filter(|c| !c.hash.is_empty())
                    .map(|c| c.hash.clone()),
                offset: self.selected.saturating_sub(self.scroll),
            });
        } else {
            stream.request(PAGE);
            self.reload = None;
            self.reset_rows();
        }
        self.log = Some(stream);
        self.log_view = Some(view);
        self.log_waiting = true;
        self.log_done = false;
        self.total_commits = None;
    }

    /// Fold in what the log stream sent, then ask for more if the view is
    /// near the end of what was read. Returns `true` when the rows changed.
    pub(crate) fn poll_log(&mut self) -> bool {
        let mut changed = false;
        while let Some(chunk) = self.log.as_ref().and_then(LogStream::try_recv) {
            changed = true;
            match chunk {
                LogChunk::Total(total) => self.total_commits = Some(total),
                LogChunk::Rows {
                    rows,
                    commits,
                    done,
                } => {
                    self.log_waiting = false;
                    self.log_done = done;
                    self.take_rows(rows, commits);
                }
            }
        }
        self.ensure_loaded();
        changed
    }

    fn take_rows(&mut self, rows: Vec<CommitInfo>, commits: usize) {
        let width = |rows: &[CommitInfo]| {
            rows.iter()
                .filter_map(|row| row.graph.as_deref())
                .map(|graph| graph.chars().count())
                .max()
                .unwrap_or(0)
        };
        if let Some(reload) = self.reload.take() {
            self.graph_width = width(&rows);
            self.commits = rows;
            self.loaded_commits = commits;
            let found = reload
                .hash
                .and_then(|hash| self.commits.iter().position(|c| c.hash == hash));
            match found {
                Some(index) => {
                    self.selected = index;
                    self.scroll = index.saturating_sub(reload.offset);
                }
                None => {
                    self.selected = self.selected.min(self.commits.len().saturating_sub(1));
                    self.ensure_visible();
                }
            }
        } else {
            self.graph_width = self.graph_width.max(width(&rows));
            self.commits.extend(rows);
            self.loaded_commits += commits;
        }
        let last = self.commits.len().saturating_sub(1);
        if self.follow_end {
            self.selected = last;
            self.ensure_visible();
            self.follow_end = !self.log_done;
        }
        if let Some(target) = self.scroll_target {
            let visible = self.visible_rows();
            if target + visible <= self.commits.len() || self.log_done {
                self.scroll_target = None;
                self.scroll = target.min(self.commits.len().saturating_sub(visible));
                self.selected = self.selected.clamp(self.scroll, last.max(self.scroll));
            }
        }
    }

    /// Ask the stream for more when fewer than [`AHEAD_SCREENS`] screens are
    /// left below the view, when a thumb drag went past the rows read, or
    /// while `End` reads to the end.
    pub(crate) fn ensure_loaded(&mut self) {
        if self.log_done || self.log_waiting || self.reload.is_some() {
            return;
        }
        let Some(stream) = &self.log else {
            return;
        };
        let visible = self.visible_rows();
        let bottom = (self.scroll + visible)
            .max(self.selected + 1)
            .max(self.scroll_target.map_or(0, |t| t + visible));
        let wanted = bottom + AHEAD_SCREENS * visible;
        let request = if self.follow_end {
            END_PAGE
        } else if self.commits.len() < wanted {
            PAGE.max(wanted - self.commits.len())
        } else {
            return;
        };
        stream.request(request);
        self.log_waiting = true;
    }

    /// Rows the scrollbar spans: those read, and while more remain, one per
    /// commit not read yet (a graph's connector rows are not known ahead).
    pub(crate) fn content_rows(&self) -> usize {
        let rest = if self.log_done {
            0
        } else {
            self.total_commits
                .map_or(0, |total| total.saturating_sub(self.loaded_commits))
        };
        self.commits.len() + rest
    }

    /// Whether history remains to be read below the last row.
    pub(crate) fn has_more(&self) -> bool {
        self.log.is_some() && !self.log_done
    }

    /// If a refresh was requested while a worker was in flight, run the single
    /// coalesced follow-up pass now that the receiver is free.
    fn run_pending_refresh(&mut self) {
        if self.refresh_pending {
            self.refresh_pending = false;
            self.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use ratatui::layout::Rect;

    use super::PAGE;
    use crate::GitLogPanel;

    /// A repository whose `main` holds `n` commits, written in one
    /// `git fast-import` so a long history costs no time.
    fn repo(n: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
        };
        assert!(git(&["init", "-q", "-b", "main"]).status.success());
        let mut stream = String::new();
        for i in 0..n {
            let message = format!("commit {i}");
            stream.push_str(&format!(
                "commit refs/heads/main\nmark :{}\ncommitter t <t@t> {} +0000\ndata {}\n{message}\n",
                i + 1,
                1_700_000_000 + i,
                message.len()
            ));
            if i > 0 {
                stream.push_str(&format!("from :{i}\n"));
            }
            stream.push('\n');
        }
        let mut import = Command::new("git")
            .args(["fast-import", "--quiet"])
            .current_dir(dir.path())
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        import
            .stdin
            .take()
            .unwrap()
            .write_all(stream.as_bytes())
            .unwrap();
        assert!(import.wait().unwrap().success());
        assert!(git(&["reset", "-q", "--hard", "main"]).status.success());
        dir
    }

    fn panel(repo: &Path) -> GitLogPanel {
        let mut panel = GitLogPanel::new_for_repo(repo.to_path_buf(), None);
        panel.last_area = Rect::new(0, 0, 80, 22);
        panel
    }

    /// Tick until `done` holds.
    fn settle(panel: &mut GitLogPanel, done: impl Fn(&GitLogPanel) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(panel) {
            crate::Panel::tick(panel);
            assert!(Instant::now() < deadline, "the log did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn idle(panel: &GitLogPanel) -> bool {
        panel.refresh_rx.is_none() && !panel.log_waiting && panel.reload.is_none()
    }

    #[test]
    fn scrolling_toward_the_end_reads_more() {
        let dir = repo(1200);
        let mut panel = panel(dir.path());
        settle(&mut panel, |p| idle(p) && p.log.is_some());
        assert_eq!(panel.loaded_commits, PAGE);
        settle(&mut panel, |p| p.total_commits.is_some());
        assert_eq!(panel.content_rows(), 1200, "the scrollbar spans it all");

        // Past two screens from the end of what was read, the next page comes.
        while panel.selected + 2 * panel.visible_rows() < PAGE {
            panel.move_down();
        }
        panel.ensure_loaded();
        settle(&mut panel, idle);
        assert_eq!(panel.loaded_commits, 2 * PAGE);
        assert!(panel.has_more());
        assert_eq!(
            panel.commits[PAGE].message,
            format!("commit {}", 1199 - PAGE)
        );
    }

    #[test]
    fn end_reads_the_whole_history() {
        let dir = repo(1200);
        let mut panel = panel(dir.path());
        settle(&mut panel, |p| idle(p) && p.log.is_some());
        panel.go_to_end();
        settle(&mut panel, |p| !p.has_more());
        assert_eq!(panel.commits.len(), 1200);
        assert_eq!(panel.selected, 1199);
        assert_eq!(panel.commits[1199].message, "commit 0");
    }

    #[test]
    fn a_refresh_keeps_the_selected_commit() {
        let dir = repo(1200);
        let mut panel = panel(dir.path());
        settle(&mut panel, |p| idle(p) && p.log.is_some());
        for _ in 0..700 {
            panel.move_down();
            panel.ensure_loaded();
            settle(&mut panel, idle);
        }
        let hash = panel.selected_commit().unwrap().hash.clone();
        let offset = panel.selected - panel.scroll;

        panel.refresh();
        // The old rows stay up while the log is read again.
        assert_eq!(panel.selected_commit().unwrap().hash, hash);
        settle(&mut panel, |p| idle(p) && p.log_view.is_some());
        assert_eq!(panel.selected_commit().unwrap().hash, hash);
        assert_eq!(panel.selected - panel.scroll, offset);
        assert!(panel.loaded_commits >= 700);
    }
}
