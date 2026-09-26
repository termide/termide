//! Suggestions for the path prompts — Open…, `Ctrl+G` and the file manager's
//! "go to path". A path being typed (`/`, `~`, `./`, `../`) completes from
//! its directory; anything else is matched fuzzily against the project's
//! files, walked once in the background when the prompt opens. A URL gets no
//! suggestions.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use termide_modal::Suggest;
use termide_ui::fuzzy::{rank, Query};
use termide_ui::{expand_tilde, CompletionItem};

/// Files a project walk collects at most; a bigger tree is cut off there.
const MAX_PROJECT_FILES: usize = 200_000;

/// Suggestions one query yields at most.
const MAX_SUGGESTIONS: usize = 100;

/// A typed path made absolute: `~` is the home directory, a relative path
/// is taken from `base_dir`.
pub(in crate::app) fn resolve_typed_path(input: &str, base_dir: &Path) -> PathBuf {
    let path = expand_tilde(input);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

/// What the typed text is, which decides where suggestions come from.
#[derive(Debug, PartialEq, Eq)]
enum Typed {
    Nothing,
    Url,
    Path,
    Name,
}

fn classify(text: &str) -> Typed {
    if text.trim().is_empty() {
        Typed::Nothing
    } else if text.contains("://") {
        Typed::Url
    } else if text.starts_with('/')
        || text.starts_with('~')
        || text.starts_with("./")
        || text.starts_with("../")
        || text == "."
        || text == ".."
    {
        Typed::Path
    } else {
        Typed::Name
    }
}

pub(in crate::app) struct PathSuggestions {
    /// Where a relative path is taken from.
    base_dir: PathBuf,
    /// The project whose files a name is matched against.
    root: PathBuf,
    /// The project's files relative to `root`, once the walk is in.
    files: Vec<String>,
    walk: Option<Receiver<Vec<String>>>,
    cancel: Arc<AtomicBool>,
}

impl PathSuggestions {
    /// Start walking `root` for its files; relative paths complete from
    /// `base_dir`.
    pub(in crate::app) fn new(base_dir: PathBuf, root: PathBuf) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let walk_root = root.clone();
        let walk_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("project-files".into())
            .spawn(move || {
                let files = termide_panel_file_manager::project_files(
                    &walk_root,
                    &walk_cancel,
                    MAX_PROJECT_FILES,
                );
                let _ = tx.send(files);
            });
        if let Err(e) = spawned {
            log::warn!("Could not walk the project for file suggestions: {e}");
        }
        Self {
            base_dir,
            root,
            files: Vec::new(),
            walk: Some(rx),
            cancel,
        }
    }

    /// Entries of the directory the text names up to its last `/`, fuzzily
    /// matched on the part after it; directories first, then by name.
    fn complete_path(&self, text: &str) -> Vec<CompletionItem> {
        // `~`, `.` and `..` alone name a directory: list it.
        let (dir_part, name_part) = match text.rfind('/') {
            Some(i) => (text[..=i].to_string(), &text[i + 1..]),
            None => (format!("{text}/"), ""),
        };
        let Ok(read) = std::fs::read_dir(resolve_typed_path(&dir_part, &self.base_dir)) else {
            return Vec::new();
        };
        let mut entries: Vec<(String, bool)> = read
            .filter_map(Result::ok)
            .map(|entry| {
                // `Path::is_dir` follows a symlink to a directory.
                let is_dir = entry.path().is_dir();
                (entry.file_name().to_string_lossy().into_owned(), is_dir)
            })
            .collect();
        entries.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        let mut query = Query::fuzzy(name_part);
        rank(entries.iter().map(|(name, _)| query.score(name)))
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .map(|i| {
                let (name, is_dir) = &entries[i];
                let matched = query.positions(name).unwrap_or_default();
                let label = if *is_dir {
                    format!("{name}/")
                } else {
                    name.clone()
                };
                CompletionItem::new(format!("{dir_part}{label}"))
                    .with_label(label)
                    .with_matched(matched)
            })
            .collect()
    }

    /// The project's files that fuzzily match `text`, best first, as
    /// absolute paths labelled relative to the project.
    fn match_files(&self, text: &str) -> Vec<CompletionItem> {
        let mut query = Query::fuzzy_path(text);
        rank(self.files.iter().map(|file| query.score(file)))
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .map(|i| {
                let file = &self.files[i];
                CompletionItem::new(self.root.join(file).display().to_string())
                    .with_label(file.clone())
                    .with_matched(query.positions(file).unwrap_or_default())
            })
            .collect()
    }
}

impl Suggest for PathSuggestions {
    fn suggest(&mut self, text: &str) -> Vec<CompletionItem> {
        match classify(text) {
            Typed::Nothing | Typed::Url => Vec::new(),
            Typed::Path => self.complete_path(text),
            Typed::Name => self.match_files(text),
        }
    }

    fn poll(&mut self) -> bool {
        let Some(walk) = self.walk.as_ref() else {
            return false;
        };
        match walk.try_recv() {
            Ok(files) => {
                self.files = files;
                self.walk = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.walk = None;
                false
            }
        }
    }
}

impl Drop for PathSuggestions {
    /// A prompt closed before the walk finished stops it.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "src/main.rs",
            "src/panel/lib.rs",
            "Cargo.toml",
            "docs/guide.md",
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        dir
    }

    /// Suggestions once the background walk is in.
    fn walked(root: &Path, base_dir: &Path) -> PathSuggestions {
        let mut suggestions = PathSuggestions::new(base_dir.to_path_buf(), root.to_path_buf());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !suggestions.poll() {
            assert!(std::time::Instant::now() < deadline, "walk timed out");
            std::thread::yield_now();
        }
        suggestions
    }

    fn values(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.value.as_str()).collect()
    }

    #[test]
    fn classifies_what_is_typed() {
        assert_eq!(classify(" "), Typed::Nothing);
        assert_eq!(classify("https://example.com"), Typed::Url);
        assert_eq!(classify("postgres://db/x"), Typed::Url);
        for path in ["/etc", "~", "~/x", "./a", "../b", ".", ".."] {
            assert_eq!(classify(path), Typed::Path, "{path}");
        }
        assert_eq!(classify("src/main"), Typed::Name);
        assert_eq!(classify(".gitignore"), Typed::Name);
    }

    #[test]
    fn a_name_matches_project_files_fuzzily_as_absolute_paths() {
        let dir = project();
        let root = dir.path();
        let mut suggestions = walked(root, root);
        let items = suggestions.suggest("panlib");
        assert_eq!(
            values(&items),
            [root.join("src/panel/lib.rs").display().to_string()]
        );
        assert_eq!(items[0].label, "src/panel/lib.rs");
        assert!(!items[0].matched.is_empty());
        assert!(suggestions.suggest("https://x.org").is_empty());
    }

    #[test]
    fn a_path_completes_from_its_directory_directories_first() {
        let dir = project();
        let root = dir.path();
        let mut suggestions = walked(root, &root.join("docs"));

        let absolute = format!("{}/", root.display());
        let items = suggestions.suggest(&absolute);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["docs/", "src/", "Cargo.toml"]);
        assert_eq!(items[0].value, format!("{absolute}docs/"));

        // Relative to the base directory, keeping the typed form.
        let items = suggestions.suggest("../sr");
        assert_eq!(values(&items), ["../src/"]);
        assert_eq!(values(&suggestions.suggest("./")), ["./guide.md"]);
    }

    #[test]
    fn typed_paths_resolve_home_and_relative() {
        let base = Path::new("/base");
        assert_eq!(resolve_typed_path("a/b", base), Path::new("/base/a/b"));
        assert_eq!(resolve_typed_path("/abs", base), Path::new("/abs"));
        if let Some(home) = dirs::home_dir() {
            assert_eq!(resolve_typed_path("~/x", base), home.join("x"));
        }
    }
}
