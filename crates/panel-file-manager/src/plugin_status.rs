//! What plugins say of files git says nothing about: the state another
//! version control system gives them, coloured as git's would be.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use termide_git::GitStatus;

use super::FileManager;

/// Statuses plugins set, by absolute path, with the directories above each
/// changed file marked modified as git marks them.
#[derive(Debug, Default, Clone)]
pub(crate) struct PluginStatuses {
    files: Arc<HashMap<PathBuf, GitStatus>>,
    dirs: HashMap<PathBuf, GitStatus>,
}

impl PluginStatuses {
    fn new(files: Arc<HashMap<PathBuf, GitStatus>>) -> Self {
        let mut dirs = HashMap::new();
        for (path, status) in files.iter() {
            if matches!(status, GitStatus::Ignored | GitStatus::Unmodified) {
                continue;
            }
            for dir in path.ancestors().skip(1) {
                if dirs
                    .insert(dir.to_path_buf(), GitStatus::Modified)
                    .is_some()
                {
                    break;
                }
            }
        }
        Self { files, dirs }
    }

    /// The status to show for the entry at `path`: git's, unless git has
    /// nothing to say of it, then the plugins'.
    pub(crate) fn over(&self, git: GitStatus, path: &Path, is_dir: bool) -> GitStatus {
        if git != GitStatus::Unmodified || self.files.is_empty() {
            return git;
        }
        let found = if is_dir {
            self.files.get(path).or_else(|| self.dirs.get(path))
        } else {
            self.files.get(path)
        };
        found.copied().unwrap_or(git)
    }
}

impl FileManager {
    /// What the plugins say of the files, by absolute path, replacing what
    /// they said before.
    pub fn set_plugin_statuses(&mut self, files: Arc<HashMap<PathBuf, GitStatus>>) {
        if Arc::ptr_eq(&self.plugin_statuses.files, &files) {
            return;
        }
        self.plugin_statuses = PluginStatuses::new(files);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_comes_first_and_directories_show_their_changed_files() {
        let files = HashMap::from([
            (PathBuf::from("/ws/src/a.c"), GitStatus::Modified),
            (PathBuf::from("/ws/new.txt"), GitStatus::Added),
            (PathBuf::from("/ws/build/out.o"), GitStatus::Ignored),
        ]);
        let statuses = PluginStatuses::new(Arc::new(files));
        let un = GitStatus::Unmodified;
        assert_eq!(
            statuses.over(un, Path::new("/ws/src/a.c"), false),
            GitStatus::Modified
        );
        assert_eq!(
            statuses.over(GitStatus::Added, Path::new("/ws/src/a.c"), false),
            GitStatus::Added,
            "git's own status wins"
        );
        assert_eq!(
            statuses.over(un, Path::new("/ws/src"), true),
            GitStatus::Modified
        );
        assert_eq!(
            statuses.over(un, Path::new("/ws/build"), true),
            un,
            "an ignored file changes nothing above it"
        );
        assert_eq!(
            statuses.over(un, Path::new("/ws/build/out.o"), false),
            GitStatus::Ignored
        );
        assert_eq!(statuses.over(un, Path::new("/ws/other.c"), false), un);
    }
}
