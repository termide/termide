//! Branch listing, checkout, and upstream ahead/behind counts.

use std::path::Path;

use crate::command::{git_command_stdout, run_git_simple};

/// Get current branch name
pub fn get_current_branch(repo: &Path) -> Option<String> {
    git_command_stdout(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string())
}

/// Get list of all local branches
pub fn get_branches(repo: &Path) -> Vec<String> {
    git_command_stdout(repo, &["branch", "--format=%(refname:short)"])
        .map(|s| s.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default()
}

/// Get all branches (local and remote-tracking).
/// Remote branches are included only if there's no local branch with the same name.
/// For example, if "main" exists locally, "origin/main" is not included.
pub fn get_all_branches(repo: &Path) -> Vec<String> {
    // Get local branches first
    let local_branches: Vec<String> = get_branches(repo);

    // Get remote branches
    let remote_branches: Vec<String> =
        git_command_stdout(repo, &["branch", "-r", "--format=%(refname:short)"])
            .map(|s| {
                s.lines()
                    .map(|l| l.to_string())
                    // Filter out HEAD pointer (e.g., "origin/HEAD")
                    .filter(|b| !b.ends_with("/HEAD"))
                    .collect()
            })
            .unwrap_or_default();

    // Combine: local branches + remote branches that don't have a local equivalent
    let mut result = local_branches.clone();
    for remote in remote_branches {
        // Extract branch name after "origin/" (or other remote name)
        if let Some(branch_name) = remote.split('/').nth(1) {
            // Only add if no local branch with this name exists
            if !local_branches.iter().any(|local| local == branch_name) {
                result.push(remote);
            }
        }
    }
    result
}

/// Switch to a different branch.
/// If branch looks like a remote branch (contains '/'), use --track to create a local tracking branch.
pub fn checkout_branch(repo: &Path, branch: &str) -> Result<(), String> {
    // If branch looks like a remote branch (contains '/'), use --track
    let args: Vec<&str> = if branch.contains('/') {
        vec!["checkout", "--track", branch]
    } else {
        vec!["checkout", branch]
    };
    run_git_simple(
        repo,
        &args,
        &format!("Failed to checkout branch: {}", branch),
    )
}

/// Get ahead/behind counts relative to upstream
pub fn get_ahead_behind(repo: &Path) -> (usize, usize) {
    git_command_stdout(
        repo,
        &["rev-list", "--left-right", "--count", "@{u}...HEAD"],
    )
    .and_then(|s| {
        let parts: Vec<&str> = s.split_whitespace().collect();
        if parts.len() == 2 {
            let behind = parts[0].parse().unwrap_or(0);
            let ahead = parts[1].parse().unwrap_or(0);
            Some((ahead, behind))
        } else {
            None
        }
    })
    .unwrap_or_else(|| {
        // No upstream tracking branch — try to find remote default branch
        for remote_ref in &["origin/HEAD", "origin/main", "origin/master"] {
            if let Some(count) = git_command_stdout(
                repo,
                &["rev-list", "--count", &format!("{}..HEAD", remote_ref)],
            )
            .and_then(|s| s.trim().parse().ok())
            {
                return (count, 0);
            }
        }
        // No upstream tracking and no known remote branch — count all local commits
        let ahead = git_command_stdout(repo, &["rev-list", "--count", "HEAD"])
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        (ahead, 0)
    })
}
