//! Background file-name and content search walkers.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use ignore::{Walk, WalkBuilder};
use regex::RegexBuilder;
use termide_core::util::is_binary_file;
use termide_git::{GitStatus, GitStatusCache};
use termide_ui::fuzzy::Query;

use super::{ContentResult, FileResult};

/// Maximum total results to collect
const MAX_RESULTS: usize = 500;

/// Matches a path's file name (or relative path) against a search mask.
///
/// Shared by the name and content walkers so both apply the same
/// path-glob / name-glob / substring rules and honor the `[Aa]` Case toggle
/// identically. `use_regex` (name search with `[.*]` on) matches the name as
/// a regular expression; content search always passes `use_regex = false`
/// because its mask is only ever a glob (the regex there matches file
/// *contents*, not names).
struct NameMatcher {
    has_path_sep: bool,
    has_wildcards: bool,
    case_sensitive: bool,
    /// Some only in name-regex mode; matched against the name / relative path.
    regex: Option<regex::Regex>,
    /// Glob over the case-folded mask when Case is off.
    glob: Option<glob::Pattern>,
    /// The mask as a plain "contains", for a name mask without wildcards.
    substring: Query,
}

impl NameMatcher {
    /// Returns `None` only when a regex mask fails to compile (the caller then
    /// yields no results); a glob that fails to parse simply matches nothing.
    fn new(mask: &str, use_regex: bool, case_sensitive: bool) -> Option<Self> {
        let regex = if use_regex {
            match RegexBuilder::new(mask)
                .case_insensitive(!case_sensitive)
                .build()
            {
                Ok(r) => Some(r),
                Err(_) => return None,
            }
        } else {
            None
        };
        // Fold case into the glob when Case is off so pattern and candidate
        // are compared in the same case.
        let glob = if use_regex {
            None
        } else if case_sensitive {
            glob::Pattern::new(mask).ok()
        } else {
            glob::Pattern::new(&mask.to_lowercase()).ok()
        };
        Some(Self {
            has_path_sep: mask.contains('/') || mask.contains('\\'),
            has_wildcards: mask.contains('*') || mask.contains('?'),
            case_sensitive,
            regex,
            glob,
            substring: Query::substring(mask, case_sensitive),
        })
    }

    /// `Some(matched)`, or `None` when the entry has no usable file name.
    fn matches(&mut self, path: &Path, relative_path: &str) -> Option<bool> {
        if let Some(re) = self.regex.as_ref() {
            // Match the path when the pattern spans separators, else the name.
            return Some(if self.has_path_sep {
                re.is_match(relative_path)
            } else {
                re.is_match(&path.file_name()?.to_string_lossy())
            });
        }
        if self.has_path_sep {
            let hay = if self.case_sensitive {
                relative_path.to_string()
            } else {
                relative_path.to_lowercase()
            };
            return Some(self.glob.as_ref().map(|g| g.matches(&hay)).unwrap_or(false));
        }
        let name = path.file_name()?.to_string_lossy();
        Some(if self.has_wildcards {
            let hay = if self.case_sensitive {
                name.into_owned()
            } else {
                name.to_lowercase()
            };
            self.glob.as_ref().map(|g| g.matches(&hay)).unwrap_or(false)
        } else {
            self.substring.score(&name).is_some()
        })
    }
}

/// Every entry under `base`, hidden ones included. With `respect_gitignore`
/// the walk leaves out what git ignores and the `.git` directory itself, as
/// a project-wide search wants; without it, it sees everything, as a search
/// of the directory on screen does.
fn walker(base: &Path, respect_gitignore: bool) -> Walk {
    let mut builder = WalkBuilder::new(base);
    builder
        .hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore);
    if respect_gitignore {
        builder.filter_entry(|entry| entry.file_name() != ".git");
    }
    builder.build()
}

/// The files under `root` a project-wide search offers, as paths relative to
/// `root`: what git ignores and `.git` left out, at most `limit` of them.
pub fn project_files(root: &Path, cancel: &AtomicBool, limit: usize) -> Vec<String> {
    let mut files = Vec::new();
    for entry in walker(root, true) {
        if cancel.load(Ordering::Relaxed) || files.len() >= limit {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(relative) = entry.path().strip_prefix(root) {
            files.push(relative.display().to_string());
        }
    }
    files
}

pub(super) fn search_files(
    base_path: &Path,
    mask: &str,
    use_regex: bool,
    case_sensitive: bool,
    cancel: &AtomicBool,
    git_cache: Option<&GitStatusCache>,
) -> Vec<FileResult> {
    let Some(mut matcher) = NameMatcher::new(mask, use_regex, case_sensitive) else {
        return Vec::new();
    };
    let mut results = Vec::new();

    let walker = walker(base_path, false);

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if path == base_path {
            continue;
        }

        let relative_path = path
            .strip_prefix(base_path)
            .map(|r| r.display().to_string())
            .unwrap_or_default();

        if matcher.matches(path, &relative_path) != Some(true) {
            continue;
        }

        let is_dir = path.is_dir();
        let git_status = git_cache
            .map(|cache| {
                if is_dir {
                    cache.get_directory_status(&relative_path)
                } else {
                    cache.get_status(&relative_path)
                }
            })
            .unwrap_or(GitStatus::Unmodified);

        results.push(FileResult {
            full_path: path.to_path_buf(),
            relative_path,
            git_status,
            is_dir,
        });

        if results.len() >= MAX_RESULTS {
            break;
        }
    }

    results.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    results
}

pub(super) fn search_content(
    base_path: &Path,
    mask: &str,
    content_pattern: &str,
    case_sensitive: bool,
    cancel: &AtomicBool,
    git_cache: Option<&GitStatusCache>,
    max_file_size: u64,
) -> Vec<ContentResult> {
    let regex = match RegexBuilder::new(content_pattern)
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    // The mask filters file names with the same rules (and Case toggle) as the
    // name search; the content `regex` above is what honors case for matches.
    let mut matcher = match NameMatcher::new(mask, false, case_sensitive) {
        Some(m) => m,
        None => return Vec::new(),
    };

    let min_size = content_pattern.len() as u64;
    let mut results = Vec::new();

    let walker = walker(base_path, false);

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if path.is_dir() {
            continue;
        }

        let relative_path = path
            .strip_prefix(base_path)
            .map(|r| r.display().to_string())
            .unwrap_or_default();

        if matcher.matches(path, &relative_path) != Some(true) {
            continue;
        }

        if should_skip_file(path, max_file_size, min_size) {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let git_status = git_cache
            .map(|cache| cache.get_status(&relative_path))
            .unwrap_or(GitStatus::Unmodified);

        for (line_idx, line) in lines.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            if let Some(m) = regex.find(line) {
                results.push(ContentResult {
                    full_path: path.to_path_buf(),
                    relative_path: relative_path.clone(),
                    line_number: line_idx + 1,
                    matched_line: line.to_string(),
                    match_start: m.start(),
                    match_end: m.end(),
                    git_status,
                });

                if results.len() >= MAX_RESULTS {
                    return results;
                }
            }
        }

        if results.len() >= MAX_RESULTS {
            break;
        }
    }

    results
}

fn should_skip_file(path: &Path, max_size: u64, min_size: u64) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true,
    };
    let file_size = meta.len();
    if file_size < min_size {
        return true;
    }
    if max_size > 0 && file_size > max_size {
        return true;
    }
    is_binary_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matcher_applies_glob_substring_regex_and_case() {
        let p = Path::new("/proj/src/Main.rs");
        let rel = "src/Main.rs";

        // Case-insensitive substring (default).
        assert_eq!(
            NameMatcher::new("main", false, false)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        // Case-sensitive substring: "main" no longer matches "Main".
        assert_eq!(
            NameMatcher::new("main", false, true)
                .unwrap()
                .matches(p, rel),
            Some(false)
        );
        // Wildcard glob over the name, case-folded when Case is off.
        assert_eq!(
            NameMatcher::new("*.RS", false, false)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        assert_eq!(
            NameMatcher::new("*.RS", false, true)
                .unwrap()
                .matches(p, rel),
            Some(false)
        );
        // Path-glob when the mask spans separators.
        assert_eq!(
            NameMatcher::new("src/*.rs", false, true)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        // Regex over the name; case-sensitive anchored.
        assert_eq!(
            NameMatcher::new("^Main", true, true)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        // Invalid regex → no matcher at all.
        assert!(NameMatcher::new("(", true, false).is_none());
    }

    #[test]
    fn name_search_honors_case_and_regex_toggles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "").unwrap();
        std::fs::write(dir.path().join("notes.rs"), "").unwrap();

        let names = |mask: &str, regex: bool, case: bool| -> Vec<String> {
            let cancel = AtomicBool::new(false);
            let mut out: Vec<String> = search_files(dir.path(), mask, regex, case, &cancel, None)
                .into_iter()
                .map(|r| r.relative_path)
                .collect();
            out.sort();
            out
        };

        // Substring, case-insensitive (default): both readme files.
        assert_eq!(
            names("readme", false, false),
            vec!["README.md", "readme.txt"]
        );
        // Substring, case-sensitive: only the lowercase one.
        assert_eq!(names("readme", false, true), vec!["readme.txt"]);
        // Regex, case-insensitive: anchored extension match on both readmes.
        assert_eq!(
            names(r"readme\.(md|txt)$", true, false),
            vec!["README.md", "readme.txt"]
        );
        // Regex, case-sensitive: only the uppercase README.
        assert_eq!(names(r"^README", true, true), vec!["README.md"]);
    }

    #[test]
    fn project_files_leave_out_what_git_ignores_and_the_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            ".git/HEAD",
            ".github/workflows/ci.yml",
            "src/main.rs",
            "target/debug/app",
            "notes.log",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();

        let never = AtomicBool::new(false);
        let mut files = project_files(root, &never, 100);
        files.sort();
        assert_eq!(
            files,
            [".github/workflows/ci.yml", ".gitignore", "src/main.rs"],
            "hidden files stay, ignored ones and .git go"
        );
        assert_eq!(project_files(root, &never, 1).len(), 1, "capped");
        assert!(project_files(root, &AtomicBool::new(true), 100).is_empty());
    }
}
