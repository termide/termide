//! Type definitions for Git Status Panel.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::tree;

/// Grouped tree state for one file section (unstaged or staged).
pub(crate) struct FileTree {
    pub tree: Vec<tree::TreeNode>,
    pub visible: Vec<usize>,
    pub prefixes: Vec<String>,
    /// Per-node `(status, untracked)`, indexed like `tree`. For directories
    /// this is the aggregate over descendants, computed once when the tree is
    /// (re)derived so rendering never re-walks the subtree per frame.
    pub node_status: Vec<(char, bool)>,
    pub collapsed: HashSet<PathBuf>,
}

impl FileTree {
    pub fn new() -> Self {
        Self {
            tree: Vec::new(),
            visible: Vec::new(),
            prefixes: Vec::new(),
            node_status: Vec::new(),
            collapsed: HashSet::new(),
        }
    }

    /// Recompute `visible`, `prefixes`, and `node_status` from the current
    /// `tree`.
    pub fn recompute_visible(&mut self) {
        self.visible = tree::compute_visible_nodes(&self.tree);
        self.prefixes = tree::compute_tree_prefixes(&self.tree, &self.visible);
        self.node_status = self
            .tree
            .iter()
            .enumerate()
            .map(|(i, node)| match node.kind {
                tree::TreeNodeKind::Directory { .. } => tree::aggregate_dir_status(&self.tree, i),
                tree::TreeNodeKind::File {
                    status, untracked, ..
                } => (status, untracked),
            })
            .collect();
    }
}

/// Section of the Git Status panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Repository selector
    RepoSelector,
    /// Branch selector
    BranchSelector,
    /// Files list (both unstaged and staged)
    Files,
    /// Action buttons
    Buttons,
}

/// Current selection in the files area
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Cursor on Unstaged header (selecting [Stage all] button)
    UnstagedHeader,
    /// Cursor on an unstaged file at given index
    UnstagedFile(usize),
    /// Cursor on an unstaged directory node (index into unstaged full tree)
    UnstagedDir(usize),
    /// Cursor on Staged header (selecting [Unstage all] button)
    StagedHeader,
    /// Cursor on a staged file at given index
    StagedFile(usize),
    /// Cursor on a staged directory node (index into staged full tree)
    StagedDir(usize),
}

/// Button in the Git Status panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Stage all unstaged files
    StageAll,
    /// Unstage all staged files
    UnstageAll,
    /// Revert all local changes (with confirmation)
    RevertAll,
    /// Open Git Log panel
    Log,
    /// Show all diffs in Git Diff panel
    Diff,
    Commit,
    Pull,
    Push,
    /// Push operation in progress (shows spinner, click cancels)
    Pushing,
    /// Pull operation in progress (shows spinner, click cancels)
    Pulling,
    /// Initialize a new git repository
    Init,
    /// Open stash panel — contains the stash count
    Stash(usize),
}

impl Button {
    /// Get the label for this button
    pub fn label(&self, spinner_frame: usize) -> String {
        let t = termide_i18n::t();
        let spinner = termide_config::constants::SPINNER_FRAMES
            [spinner_frame % termide_config::constants::SPINNER_FRAMES.len()];
        match self {
            Button::StageAll => t.git_stage_all_btn().to_string(),
            Button::UnstageAll => t.git_unstage_all_btn().to_string(),
            Button::RevertAll => t.git_revert_all_btn().to_string(),
            Button::Log => t.git_log_btn().to_string(),
            Button::Diff => t.git_action_diff().to_string(),
            Button::Commit => t.git_action_commit().to_string(),
            Button::Pull => t.git_action_pull().to_string(),
            Button::Push => t.git_action_push().to_string(),
            Button::Pushing => {
                format!("{} {}", spinner, t.git_pushing())
            }
            Button::Pulling => {
                format!("{} {}", spinner, t.git_pulling())
            }
            Button::Init => t.git_action_init().to_string(),
            Button::Stash(n) => format!("{} ({})", t.git_stash_button(), n),
        }
    }
}
