//! Git integration for termide.
//!
//! Provides git status, diff information, and repository utilities.

use std::process::Command;
use std::sync::OnceLock;

// Internal modules
mod blame;
mod branches;
pub(crate) mod command;
mod commits;
mod discovery;
mod files;
pub mod graph;
mod operations;
mod remote_url;
mod repo_manager;
mod stash;
mod status;
mod utils;

// Public submodules
pub mod diff;

// Re-export from internal modules
pub use blame::{get_blame_async, BlameEntry};
pub use branches::{
    checkout_branch, get_ahead_behind, get_all_branches, get_branches, get_current_branch,
};
pub use command::{network_command, SshAuth};
pub use commits::{
    get_commit_details, get_commit_diff, get_file_diff, get_file_diff_stats, get_log,
    get_log_graph_unicode, get_log_with_graph, CommitDetails, CommitInfo, DiffStats,
};
pub use discovery::{
    find_all_repos, find_repo_root, find_repos_from_paths, find_toplevel_repo, find_toplevel_repos,
    get_repo_name, repo_paths_overlap,
};
pub use files::{get_staged_files, get_unstaged_files, StagedFile, UnstagedFile};
pub use operations::{
    commit, fetch, init_repo, pull, push, revert_all, revert_file, stage_all, stage_file,
    stage_files, unstage_all, unstage_file, unstage_files,
};
pub use remote_url::{get_commit_web_url, get_remote_web_url};
pub use repo_manager::RepoManager;
pub use stash::{
    stash_apply, stash_diff, stash_drop, stash_info, stash_list, stash_pop, stash_push,
    stash_rename, StashEntry, StashInfo,
};
pub use status::{
    get_git_status, get_git_status_async, get_repo_status, GitRepoStatus, GitStatus,
    GitStatusAsyncResult, GitStatusCache,
};
pub use utils::{truncate_left, truncate_path_left, truncate_right, truncate_to_width};

// Re-export diff types
pub use diff::{
    compute_inline_diff, load_original_async, GitDiffAsyncResult, GitDiffCache, InlineChange,
    InlineChangeType, LineStatus,
};

/// Global flag for git availability on system.
static GIT_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check if git is available on system.
pub fn is_available() -> bool {
    *GIT_AVAILABLE.get_or_init(|| {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
}
