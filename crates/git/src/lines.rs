//! Staging, unstaging and reverting lines of a file as an editor holds it.
//!
//! The editor's text may be unsaved, so nothing here goes through the file
//! on disk or a patch git cuts from it: the index's version of the file is
//! read, the changes between it and the text are worked out line by line,
//! and the chosen ones are carried over. A new index version is written as a
//! blob and put in the index with `update-index`; a revert is handed back to
//! the editor as an edit of its own, which its undo takes back.
//!
//! Lines are counted from 0 in the editor's text. A change that only removes
//! lines belongs to the line above the gap, where the editor's gutter marks
//! it (the first line for a gap at the top).

use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};

use similar::{capture_diff_slices, Algorithm, DiffOp};

use crate::command::{git_command, git_command_stdout, hardened_git, run_git_with_input};

/// The lines `old` has in `old` and `new` has in their place.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Change {
    old: Range<usize>,
    new: Range<usize>,
}

impl Change {
    /// Whether the change shows on one of `lines` of the newer text.
    fn touches(&self, lines: &RangeInclusive<usize>) -> bool {
        if self.new.is_empty() {
            lines.contains(&self.new.start.saturating_sub(1))
        } else {
            self.new.start <= *lines.end() && *lines.start() < self.new.end
        }
    }
}

/// A text cut after each `\n`, each line keeping its own.
fn split(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The changes that turn `old` into `new`, adjacent ones merged.
fn changes(old: &[&str], new: &[&str]) -> Vec<Change> {
    let mut changes: Vec<Change> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, old, new) {
        if let DiffOp::Equal { .. } = op {
            continue;
        }
        let (old_range, new_range) = (op.old_range(), op.new_range());
        match changes.last_mut() {
            Some(last) if last.old.end == old_range.start && last.new.end == new_range.start => {
                last.old.end = old_range.end;
                last.new.end = new_range.end;
            }
            _ => changes.push(Change {
                old: old_range,
                new: new_range,
            }),
        }
    }
    changes
}

/// `old` with the changes `take` picks carried over from `new`.
fn blend(old: &[&str], new: &[&str], changes: &[Change], take: impl Fn(&Change) -> bool) -> String {
    let mut out = String::new();
    let mut at = 0;
    for change in changes {
        out.extend(old[at..change.old.start].iter().copied());
        if take(change) {
            out.extend(new[change.new.clone()].iter().copied());
        } else {
            out.extend(old[change.old.clone()].iter().copied());
        }
        at = change.old.end;
    }
    out.extend(old[at..].iter().copied());
    out
}

/// The text with `\r\n` read as `\n`, and whether it had any: the editor
/// holds its lines so, and what goes back into git gets them back.
fn normalised(text: &str) -> (String, bool) {
    if text.contains("\r\n") {
        (text.replace("\r\n", "\n"), true)
    } else {
        (text.to_string(), false)
    }
}

/// An edit that puts lines of the editor's text back as the index has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRevert {
    /// The lines of the editor's text to replace.
    pub lines: Range<usize>,
    /// What replaces them, each line ending in `\n` as the editor holds it.
    pub text: String,
}

/// The edit that puts the changes `text` has against `index` on `lines`
/// back as `index` has them; `None` when they show none.
pub fn revert_lines(index: &str, text: &str, lines: RangeInclusive<usize>) -> Option<LineRevert> {
    let (index, _) = normalised(index);
    let (old, new) = (split(&index), split(text));
    let chosen: Vec<Change> = changes(&old, &new)
        .into_iter()
        .filter(|change| change.touches(&lines))
        .collect();
    let (first, last) = (chosen.first()?, chosen.last()?);
    let mut replacement = String::new();
    let mut at = first.new.start;
    for change in &chosen {
        replacement.extend(new[at..change.new.start].iter().copied());
        replacement.extend(old[change.old.clone()].iter().copied());
        at = change.new.end;
    }
    Some(LineRevert {
        lines: first.new.start..last.new.end,
        text: replacement,
    })
}

/// Where a file stands in its repository.
struct Entry {
    root: PathBuf,
    /// The path from the root, `/`-separated as git names it.
    name: String,
    file: PathBuf,
}

impl Entry {
    fn of(file: &Path) -> Result<Self, String> {
        let file = dunce::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        let dir = file.parent().unwrap_or_else(|| Path::new("/"));
        let root = git_command_stdout(dir, &["rev-parse", "--show-toplevel"])
            .map(|root| PathBuf::from(root.trim()))
            .ok_or_else(|| format!("{} is not in a git repository", file.display()))?;
        let name = file
            .strip_prefix(&root)
            .map_err(|_| format!("{} is outside {}", file.display(), root.display()))?
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Ok(Self { root, name, file })
    }

    /// The file's mode in the index, `None` when it has no entry there;
    /// an error for one git keeps in several stages, a merge conflict.
    fn index_mode(&self) -> Result<Option<String>, String> {
        let out = hardened_git(&self.root, &["ls-files", "-s", "--", &self.name])
            .env("GIT_LITERAL_PATHSPECS", "1")
            .output()
            .map_err(|e| format!("git ls-files: {e}"))?;
        let listing = String::from_utf8_lossy(&out.stdout);
        let mut entries = listing.lines();
        let Some(first) = entries.next() else {
            return Ok(None);
        };
        let mut fields = first.split_whitespace();
        let mode = fields.next().unwrap_or_default().to_string();
        let stage = fields.nth(1).unwrap_or("0");
        if stage != "0" || entries.next().is_some() {
            return Err(format!("{} has a merge conflict", self.name));
        }
        if mode != "100644" && mode != "100755" {
            return Err(format!("{} is not a regular file in git", self.name));
        }
        Ok(Some(mode))
    }

    /// The file's text in `rev` (`""` the index, else a commit), `None`
    /// where it has none.
    fn text_in(&self, rev: &str) -> Option<String> {
        let out = git_command(&self.root, &["show", &format!("{rev}:{}", self.name)])?;
        String::from_utf8(out.stdout).ok()
    }

    /// The mode a file new to the index gets.
    fn worktree_mode(&self) -> &'static str {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = std::fs::metadata(&self.file)
                .map(|meta| meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false);
            if executable {
                return "100755";
            }
        }
        "100644"
    }

    /// Put `text` in the index as the file's version.
    fn set_index(&self, mode: &str, text: &str) -> Result<(), String> {
        let blob = run_git_with_input(
            &self.root,
            &["hash-object", "-w", "--stdin", "--no-filters"],
            text.as_bytes(),
        )?;
        let blob = String::from_utf8_lossy(&blob).trim().to_string();
        let info = format!("{mode},{blob},{}", self.name);
        let out = hardened_git(&self.root, &["update-index", "--add", "--cacheinfo", &info])
            .output()
            .map_err(|e| format!("git update-index: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    fn remove_from_index(&self) -> Result<(), String> {
        let out = hardened_git(
            &self.root,
            &["update-index", "--force-remove", "--", &self.name],
        )
        .output()
        .map_err(|e| format!("git update-index: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

/// The index's version of `file`, `None` when the index has none.
pub fn index_file_content(file: &Path) -> Option<String> {
    Entry::of(file).ok()?.text_in("")
}

/// Stage the changes `text` (the file as the editor holds it) has against
/// the index on `lines`. Returns whether there were any.
pub fn stage_lines(file: &Path, text: &str, lines: RangeInclusive<usize>) -> Result<bool, String> {
    let entry = Entry::of(file)?;
    let mode = entry.index_mode()?;
    let (index, crlf) = normalised(&entry.text_in("").unwrap_or_default());
    let (old, new) = (split(&index), split(text));
    let all = changes(&old, &new);
    if !all.iter().any(|change| change.touches(&lines)) {
        return Ok(false);
    }
    let mut staged = blend(&old, &new, &all, |change| change.touches(&lines));
    if crlf {
        staged = staged.replace('\n', "\r\n");
    }
    let mode = mode.unwrap_or_else(|| entry.worktree_mode().to_string());
    entry.set_index(&mode, &staged)?;
    Ok(true)
}

/// Unstage the staged changes that show on `lines` of `text`. Returns
/// whether there were any.
pub fn unstage_lines(
    file: &Path,
    text: &str,
    lines: RangeInclusive<usize>,
) -> Result<bool, String> {
    let entry = Entry::of(file)?;
    let Some(mode) = entry.index_mode()? else {
        return Ok(false);
    };
    let (index, crlf) = normalised(&entry.text_in("").unwrap_or_default());
    let head = entry.text_in("HEAD");
    let (head_text, _) = normalised(head.as_deref().unwrap_or_default());
    let (base, staged, worktree) = (split(&head_text), split(&index), split(text));
    let in_index = index_lines(&staged, &worktree, &lines);
    let all = changes(&base, &staged);
    let touched = |change: &Change| in_index.iter().any(|range| change.touches(range));
    if !all.iter().any(touched) {
        return Ok(false);
    }
    let mut kept = blend(&base, &staged, &all, |change| !touched(change));
    if head.is_none() && kept.is_empty() {
        entry.remove_from_index()?;
        return Ok(true);
    }
    if crlf {
        kept = kept.replace('\n', "\r\n");
    }
    entry.set_index(&mode, &kept)?;
    Ok(true)
}

/// The lines of the index's version that `lines` of the editor's text
/// stand for: the same line where the two agree, the whole of the index's
/// side where they differ.
fn index_lines(
    index: &[&str],
    text: &[&str],
    lines: &RangeInclusive<usize>,
) -> Vec<RangeInclusive<usize>> {
    let mut ranges = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, index, text) {
        let (old, new) = (op.old_range(), op.new_range());
        match op {
            DiffOp::Equal { .. } => {
                let from = new.start.max(*lines.start());
                let to = new.end.min(lines.end() + 1);
                if from < to {
                    let shift = |line: usize| line - new.start + old.start;
                    ranges.push(shift(from)..=shift(to - 1));
                }
            }
            _ => {
                let change = Change {
                    old: old.clone(),
                    new,
                };
                if change.touches(lines) && !old.is_empty() {
                    ranges.push(old.start..=old.end - 1);
                }
            }
        }
    }
    ranges
}

#[cfg(test)]
mod tests;
