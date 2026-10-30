//! Parsing of a unified diff that did not come from git: per-file headers
//! (`diff --git`, or `---`/`+++` pairs as `diff -u`, `p4 diff -du` and
//! `svn diff` write them) and hunks read by their line counts.

use crate::{DiffHunk, DiffLine, FileDiff, FileStatus, LineKind};

/// The files of `text`, a unified diff of one or more files.
///
/// Hunk lines are read by the counts of their `@@` header, so a removed
/// line that starts with `--` is not taken for the next file's header.
/// Lines outside files and hunks (`Index:`, `====`, commit headers) are
/// skipped.
pub(crate) fn parse_unified(text: &str) -> Vec<FileDiff> {
    let lines: Vec<&str> = text.lines().collect();
    let mut files: Vec<FileDiff> = Vec::new();
    // Whether the last file was opened by `diff --git` and still waits for
    // its `---`/`+++` pair, which then names it instead of opening another.
    let mut git_header = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.push(file(git_path(rest)));
            git_header = true;
        } else if line.starts_with("--- ")
            && lines
                .get(i + 1)
                .is_some_and(|next| next.starts_with("+++ "))
        {
            let old = header_path(&line[4..]);
            let new = header_path(&lines[i + 1][4..]);
            let status = match (old.as_str(), new.as_str()) {
                ("/dev/null", _) => FileStatus::Added,
                (_, "/dev/null") => FileStatus::Deleted,
                _ => FileStatus::Modified,
            };
            let path = match status {
                FileStatus::Deleted => strip_side(&old, "a/", &new),
                _ => strip_side(&new, "b/", &old),
            };
            match files.last_mut().filter(|_| git_header) {
                Some(last) => {
                    last.path = path;
                    if last.status == FileStatus::Modified {
                        last.status = status;
                    }
                }
                None => {
                    let mut new_file = file(path);
                    new_file.status = status;
                    files.push(new_file);
                }
            }
            git_header = false;
            i += 1;
        } else if let Some(last) = files.last_mut() {
            if line.starts_with("new file mode") {
                last.status = FileStatus::Added;
            } else if line.starts_with("deleted file mode") {
                last.status = FileStatus::Deleted;
            } else if line.starts_with("rename from") || line.starts_with("rename to") {
                last.status = FileStatus::Renamed;
            } else if let Some((old_start, old_count, new_start, new_count)) = hunk_header(line) {
                git_header = false;
                i = read_hunk(
                    &lines,
                    i,
                    last,
                    (old_start, old_count, new_start, new_count),
                );
                continue;
            }
        }
        i += 1;
    }
    files
}

fn file(path: String) -> FileDiff {
    FileDiff {
        path,
        status: FileStatus::Modified,
        staged: false,
        additions: 0,
        deletions: 0,
        hunks: Vec::new(),
    }
}

/// The hunk whose header is `lines[start]`, into `file`; returns the index
/// of the first line after it.
fn read_hunk(
    lines: &[&str],
    start: usize,
    file: &mut FileDiff,
    (old_start, old_count, new_start, new_count): (usize, usize, usize, usize),
) -> usize {
    let mut hunk = DiffHunk {
        header: lines[start].to_string(),
        lines: Vec::new(),
    };
    let (mut old_line, mut new_line) = (old_start, new_start);
    let (mut old_left, mut new_left) = (old_count, new_count);
    let mut i = start + 1;
    while i < lines.len() && (old_left > 0 || new_left > 0) {
        let line = lines[i];
        let (kind, content) = match line.chars().next() {
            Some('+') if new_left > 0 => (LineKind::Added, &line[1..]),
            Some('-') if old_left > 0 => (LineKind::Removed, &line[1..]),
            Some(' ') if old_left > 0 && new_left > 0 => (LineKind::Context, &line[1..]),
            // Some tools drop the space of an empty context line.
            None if old_left > 0 && new_left > 0 => (LineKind::Context, ""),
            // `\ No newline at end of file` counts for neither side.
            Some('\\') => {
                i += 1;
                continue;
            }
            _ => break,
        };
        let (old, new) = match kind {
            LineKind::Added => {
                file.additions += 1;
                new_left -= 1;
                new_line += 1;
                (None, Some(new_line - 1))
            }
            LineKind::Removed => {
                file.deletions += 1;
                old_left -= 1;
                old_line += 1;
                (Some(old_line - 1), None)
            }
            _ => {
                old_left -= 1;
                new_left -= 1;
                old_line += 1;
                new_line += 1;
                (Some(old_line - 1), Some(new_line - 1))
            }
        };
        hunk.lines.push(DiffLine {
            kind,
            content: content.to_string(),
            old_line: old,
            new_line: new,
        });
        i += 1;
    }
    // The `\` marker may follow the hunk's last line.
    if lines.get(i).is_some_and(|line| line.starts_with('\\')) {
        i += 1;
    }
    file.hunks.push(hunk);
    i
}

/// `@@ -a[,b] +c[,d] @@` as `(a, b, c, d)`, a missing count being 1.
fn hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |text: &str| -> Option<(usize, usize)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some((old_start, old_count, new_start, new_count))
}

/// The path of a `---`/`+++` header: the text before a tab, which tools
/// follow with a date or a revision.
fn header_path(text: &str) -> String {
    text.split('\t')
        .next()
        .unwrap_or(text)
        .trim_end()
        .to_string()
}

/// `path` without git's `a/` or `b/` side prefix, dropped only when the
/// other side has its own, so a directory called `b` keeps its name.
fn strip_side(path: &str, prefix: &str, other: &str) -> String {
    let other_prefix = if prefix == "a/" { "b/" } else { "a/" };
    match path.strip_prefix(prefix) {
        Some(rest) if other.starts_with(other_prefix) || other == "/dev/null" => rest.to_string(),
        _ => path.to_string(),
    }
}

/// The path of `diff --git a/<path> b/<path>`.
fn git_path(rest: &str) -> String {
    let old = rest.split(" b/").next().unwrap_or(rest);
    old.strip_prefix("a/").unwrap_or(old).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(file: &FileDiff) -> Vec<(LineKind, &str)> {
        file.hunks[0]
            .lines
            .iter()
            .map(|line| (line.kind, line.content.as_str()))
            .collect()
    }

    #[test]
    fn a_plain_diff_u_names_its_file_by_the_new_side() {
        let text = "\
--- old/readme.txt\t2026-10-01
+++ new/readme.txt\t2026-10-02
@@ -1,2 +1,2 @@
 keep
-gone
+come
";
        let files = parse_unified(text);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "new/readme.txt");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!((files[0].additions, files[0].deletions), (1, 1));
        assert_eq!(
            kinds(&files[0]),
            vec![
                (LineKind::Context, "keep"),
                (LineKind::Removed, "gone"),
                (LineKind::Added, "come"),
            ]
        );
        let numbers: Vec<_> = files[0].hunks[0]
            .lines
            .iter()
            .map(|line| (line.old_line, line.new_line))
            .collect();
        assert_eq!(
            numbers,
            vec![(Some(1), Some(1)), (Some(2), None), (None, Some(2))]
        );
    }

    #[test]
    fn p4_headers_and_several_files() {
        let text = "\
==== //depot/a.txt#1 - /ws/a.txt ====
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-x
+y
==== //depot/b.txt#2 - /ws/b.txt ====
--- a/b.txt
+++ b/b.txt
@@ -3,0 +4 @@
+z
";
        let files = parse_unified(text);
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "b.txt"]);
        assert_eq!(files[1].hunks[0].lines[0].new_line, Some(4));
    }

    #[test]
    fn a_removed_line_that_looks_like_a_header_stays_in_its_hunk() {
        let text = "\
--- a/notes.md
+++ b/notes.md
@@ -1,2 +1,2 @@
--- a rule
+++ a plus
 tail
";
        let files = parse_unified(text);
        assert_eq!(files.len(), 1);
        assert_eq!(
            kinds(&files[0]),
            vec![
                (LineKind::Removed, "-- a rule"),
                (LineKind::Added, "++ a plus"),
                (LineKind::Context, "tail"),
            ]
        );
    }

    #[test]
    fn git_headers_give_status_and_one_file_each() {
        let text = "\
commit 123
    message
diff --git a/new.rs b/new.rs
new file mode 100644
--- /dev/null
+++ b/new.rs
@@ -0,0 +1 @@
+fn main() {}
\\ No newline at end of file
diff --git a/old.rs b/old.rs
deleted file mode 100644
--- a/old.rs
+++ /dev/null
@@ -1 +0,0 @@
-gone
";
        let files = parse_unified(text);
        let summary: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.status)).collect();
        assert_eq!(
            summary,
            [
                ("new.rs", FileStatus::Added),
                ("old.rs", FileStatus::Deleted)
            ]
        );
        assert_eq!(files[0].hunks[0].lines.len(), 1);
    }

    #[test]
    fn a_header_without_hunks_is_a_file_without_changes() {
        let files = parse_unified("--- a/same.txt\n+++ b/same.txt\n");
        assert_eq!(files.len(), 1);
        assert!(files[0].hunks.is_empty());
    }
}
