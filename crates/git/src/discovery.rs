//! Repository discovery: walking up to a repo root and down to submodules,
//! plus path-overlap and naming helpers used by the repo manager and panels.

use std::path::{Path, PathBuf};

/// Find git repository root by walking up from a path.
pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if current.join(".git").exists() {
            // Canonicalize to resolve symlinks/bind-mounts so that all
            // consumers (panels, watcher, diff) use consistent paths.
            return Some(std::fs::canonicalize(current).unwrap_or_else(|_| current.to_path_buf()));
        }
        current = current.parent()?;
    }
}

/// Whether `repo` overlaps any of `paths` — either `repo` sits inside one of
/// them or one of them sits inside `repo`. The git/file panels use this to
/// decide whether a watcher `OnGitUpdate` (which carries the changed repo
/// roots) touches the repository they currently display.
pub fn repo_paths_overlap(repo: &Path, paths: &[&Path]) -> bool {
    paths
        .iter()
        .any(|p| repo.starts_with(p) || p.starts_with(repo))
}

/// Find the top-level repository root, skipping submodules.
///
/// Submodules have `.git` as a file (not directory) containing `gitdir: ...`.
/// This function continues searching upward until it finds a repository
/// with `.git` as a directory (the actual root repo).
pub fn find_toplevel_repo(path: &Path) -> Option<PathBuf> {
    let mut current = path;

    loop {
        let git_path = current.join(".git");
        if git_path.exists() {
            // If .git is a directory (not a file), this is the top-level repo
            if git_path.is_dir() {
                return Some(current.to_path_buf());
            }
            // Otherwise it's a submodule (.git is a file), continue searching up
        }
        current = current.parent()?;
    }
}

/// Find all git repositories under a root directory up to max_depth
pub fn find_all_repos(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    find_repos_recursive(root, 0, max_depth, &mut repos);
    repos
}

/// Find repositories based on a list of paths.
///
/// For each path:
/// - Searches UP to find the repository root
/// - Searches DOWN (up to submodule_depth) to find submodules
///
/// Deduplicates input paths and discovered roots, so several panels inside
/// one repository cost a single submodule walk.
pub fn find_repos_from_paths(paths: &[PathBuf], submodule_depth: usize) -> Vec<PathBuf> {
    let toplevel = find_toplevel_repos(paths);
    let mut all: std::collections::HashSet<PathBuf> = toplevel.iter().cloned().collect();
    for repo_root in &toplevel {
        for submodule in find_all_repos(repo_root, submodule_depth) {
            all.insert(submodule);
        }
    }
    let mut result: Vec<PathBuf> = all.into_iter().collect();
    result.sort();
    result
}

/// Resolve `paths` to the set of top-level repository roots, skipping the
/// recursive submodule walk. This is the cheap half of
/// [`find_repos_from_paths`] — only the upward search to find each repo
/// root. Used by the async repo manager so the panel can render a baseline
/// list immediately while the submodule walk runs in the background.
pub fn find_toplevel_repos(paths: &[PathBuf]) -> Vec<PathBuf> {
    use std::collections::HashSet;

    // Resolve symlinks so that paths like /home/user/docs -> /Data/docs
    // don't create duplicate entries for the same physical directory.
    let unique_paths: HashSet<PathBuf> = paths
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();

    // Every input path gets its own upward search. A path nested inside
    // another input path can still resolve to a *different* repository — a
    // project under a plain container directory, or under a whole-home
    // dotfiles repo — so filtering nested inputs out beforehand would drop
    // those repos from the panel's list. Repeated roots (several panels
    // inside one repo) collapse in the set below at no cost.
    let mut result: Vec<PathBuf> = unique_paths
        .iter()
        .filter_map(|path| find_toplevel_repo(path))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    result.sort();
    result
}

fn find_repos_recursive(dir: &Path, depth: usize, max_depth: usize, repos: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }

    // Check if this directory is a git repo
    if !dir.join(".git").exists() {
        // Not a git repo, scan subdirectories
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !name.starts_with('.') {
                            find_repos_recursive(&path, depth + 1, max_depth, repos);
                        }
                    }
                }
            }
        }
        return;
    }

    // This is a git repo
    repos.push(dir.to_path_buf());

    // Parse .gitmodules to find submodules
    let gitmodules_path = dir.join(".gitmodules");
    if gitmodules_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&gitmodules_path) {
            for line in content.lines() {
                let line = line.trim();
                if let Some(path_value) = line.strip_prefix("path = ") {
                    let submodule_path = dir.join(path_value.trim());
                    if submodule_path.join(".git").exists() {
                        // Recursively find submodules in this submodule
                        find_repos_recursive(&submodule_path, depth + 1, max_depth, repos);
                    }
                }
            }
        }
    }
}

/// Get repository name (folder name containing .git)
pub fn get_repo_name(repo: &Path) -> String {
    repo.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repository")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_repo_root() {
        let current = std::env::current_dir().unwrap();
        if let Some(root) = find_repo_root(&current) {
            assert!(root.join(".git").exists());
        }
    }

    // One panel sits in a plain container directory, another inside a repo
    // nested under it. The repo must survive: it used to be discarded as a
    // "nested input path" before the upward search ever ran, so it never
    // reached the git panel's repo dropdown.
    #[test]
    fn keeps_repo_nested_under_another_input_path() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("termide-disc-nested-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let deep = tmp.join("container/lvl1/lvl2/deeprepo");
        fs::create_dir_all(deep.join(".git")).unwrap();
        let container = fs::canonicalize(tmp.join("container")).unwrap();
        let deep = fs::canonicalize(&deep).unwrap();

        let roots = find_toplevel_repos(&[container, deep.clone()]);
        let _ = fs::remove_dir_all(&tmp);
        assert!(
            roots.contains(&deep),
            "repo nested under another input path was dropped: {roots:?}"
        );
    }

    // Several panels inside one repository must still collapse to a single
    // root — that was the point of the nested-path filter, and the root-level
    // dedup has to keep covering it.
    #[test]
    fn collapses_multiple_paths_inside_one_repo() {
        use std::fs;
        let tmp =
            std::env::temp_dir().join(format!("termide-disc-collapse-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let repo = tmp.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("src/inner")).unwrap();
        let repo = fs::canonicalize(&repo).unwrap();

        let roots = find_toplevel_repos(&[repo.clone(), repo.join("src"), repo.join("src/inner")]);
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(roots, vec![repo], "paths inside one repo did not collapse");
    }
}
