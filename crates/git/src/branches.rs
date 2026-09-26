//! Branch listing, checkout, and upstream ahead/behind counts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::command::{git_command_stdout, run_git_with_stderr};

/// A branch as the selectors list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    /// The working copy it is checked out in — the repository's own or one
    /// of its linked worktrees; `None` when it is checked out nowhere (a
    /// remote-tracking branch always).
    pub worktree: Option<PathBuf>,
}

/// Every branch [`get_all_branches`] lists, each with the working copy it is
/// checked out in, if any.
pub fn get_branch_list(repo: &Path) -> Vec<BranchInfo> {
    let local: Vec<BranchInfo> = git_command_stdout(
        repo,
        &["branch", "--format=%(refname:short)%00%(worktreepath)"],
    )
    .map(|s| {
        s.lines()
            .filter_map(|line| {
                let (name, worktree) = line.split_once('\0').unwrap_or((line, ""));
                (!name.is_empty()).then(|| BranchInfo {
                    name: name.to_string(),
                    worktree: (!worktree.is_empty()).then(|| PathBuf::from(worktree)),
                })
            })
            .collect()
    })
    .unwrap_or_default();
    let names: Vec<String> = local.iter().map(|b| b.name.clone()).collect();
    let mut result = local;
    result.extend(
        remote_branches(repo, &names)
            .into_iter()
            .map(|name| BranchInfo {
                name,
                worktree: None,
            }),
    );
    result
}

/// The branches of `repo` checked out in a working copy other than `repo`
/// itself, by name, with that copy's directory.
pub fn linked_worktrees(repo: &Path, branches: &[BranchInfo]) -> HashMap<String, PathBuf> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let main = canonical(repo);
    branches
        .iter()
        .filter_map(|b| {
            let dir = b.worktree.as_deref()?;
            (canonical(dir) != main).then(|| (b.name.clone(), dir.to_path_buf()))
        })
        .collect()
}

/// How a branch selector lists `name`: `●` before the branch checked out in
/// the main copy, `⧉` after one checked out in another worktree.
pub fn branch_label(name: &str, is_head: bool, in_worktree: bool) -> String {
    let mark = if is_head { '●' } else { ' ' };
    let worktree = if in_worktree { " ⧉" } else { "" };
    format!("{mark} {name}{worktree}")
}

/// Remote-tracking branches with no local branch of the same name.
fn remote_branches(repo: &Path, local: &[String]) -> Vec<String> {
    git_command_stdout(repo, &["branch", "-r", "--format=%(refname:short)"])
        .map(|s| {
            s.lines()
                .map(|l| l.to_string())
                // Filter out HEAD pointer (e.g., "origin/HEAD")
                .filter(|b| !b.ends_with("/HEAD"))
                .filter(|remote| {
                    remote
                        .split_once('/')
                        .is_some_and(|(_, name)| !local.iter().any(|l| l == name))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How far `branch` is from `HEAD`: `(ahead, behind)`, the commits `branch`
/// has that `HEAD` lacks and the other way round.
pub fn get_ahead_behind_of(repo: &Path, branch: &str) -> (usize, usize) {
    git_command_stdout(
        repo,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{branch}"),
        ],
    )
    .and_then(|s| {
        let mut counts = s.split_whitespace().map(|n| n.parse::<usize>().ok());
        let behind = counts.next()??;
        let ahead = counts.next()??;
        Some((ahead, behind))
    })
    .unwrap_or((0, 0))
}

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
    get_branch_list(repo).into_iter().map(|b| b.name).collect()
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
    // git's own words say why it refused: local changes in the way, a
    // branch checked out in another worktree.
    run_git_with_stderr(repo, &args, "checkout")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    /// A repository on `main` with a branch `side` one commit ahead, and a
    /// branch `wt` checked out in a linked worktree next to it.
    fn repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&repo, &["branch", "side"]);
        git(&repo, &["checkout", "-q", "side"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
        git(&repo, &["checkout", "-q", "main"]);
        let worktree = root.path().join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "wt",
                worktree.to_str().unwrap(),
            ],
        );
        (root, repo, worktree)
    }

    #[test]
    fn branches_carry_the_working_copy_they_are_checked_out_in() {
        let (_root, repo, worktree) = repo_with_worktree();
        let list = get_branch_list(&repo);
        let of = |name: &str| {
            list.iter()
                .find(|b| b.name == name)
                .unwrap()
                .worktree
                .clone()
        };
        let same = |a: Option<PathBuf>, b: &Path| {
            a.and_then(|a| std::fs::canonicalize(a).ok()) == std::fs::canonicalize(b).ok()
        };
        assert!(same(of("main"), &repo));
        assert!(same(of("wt"), &worktree));
        assert_eq!(of("side"), None);
        let linked = linked_worktrees(&repo, &list);
        assert_eq!(linked.keys().collect::<Vec<_>>(), vec!["wt"]);
        assert_eq!(get_all_branches(&repo).len(), list.len());
        // `side` is one commit ahead of HEAD and none behind.
        assert_eq!(get_ahead_behind_of(&repo, "side"), (1, 0));
        // The worktree's directory is not a repository of its own.
        assert!(crate::discovery::is_linked_worktree(&worktree));
        assert!(!crate::discovery::is_linked_worktree(&repo));
        let main = crate::discovery::find_toplevel_repo(&worktree);
        assert!(same(main, &repo), "a worktree resolves to its main copy");
        // git refuses the branch another worktree holds, and says why.
        let refused = checkout_branch(&repo, "wt").unwrap_err();
        assert!(refused.contains("wt"), "{refused}");
    }
}
