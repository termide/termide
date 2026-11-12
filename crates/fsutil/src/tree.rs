//! Moving and copying whole directory trees of termide's own state, such as
//! the directories filed under a project's path that follow the project when
//! it moves.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Move (or, with `copy`, copy) the tree at `src` to `dst`, merging it into
/// whatever `dst` holds already: a file of `src` replaces one of the same
/// name. A copy leaves out the files `skip` names (lock files, say); a move
/// carries everything. Returns the files now under `dst` that came from
/// `src`. A `src` that does not exist is nothing to do.
///
/// A move into a `dst` that does not exist is one rename; otherwise entries
/// move one by one, and the directories of `src` left empty go.
pub fn transfer_tree(
    src: &Path,
    dst: &Path,
    copy: bool,
    skip: &dyn Fn(&Path) -> bool,
) -> io::Result<Vec<PathBuf>> {
    if !src.is_dir() {
        return Ok(Vec::new());
    }
    let mut placed = Vec::new();
    if !copy && !dst.exists() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::rename(src, dst).is_ok() {
            list_files(dst, &mut placed)?;
            return Ok(placed);
        }
        // Another file system: entry by entry, copying.
    }
    transfer_entries(src, dst, copy, skip, &mut placed)?;
    if !copy {
        let _ = fs::remove_dir(src);
    }
    Ok(placed)
}

fn transfer_entries(
    src: &Path,
    dst: &Path,
    copy: bool,
    skip: &dyn Fn(&Path) -> bool,
    placed: &mut Vec<PathBuf>,
) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            transfer_entries(&from, &to, copy, skip, placed)?;
            if !copy {
                // Only an emptied directory goes.
                let _ = fs::remove_dir(&from);
            }
            continue;
        }
        if copy {
            if skip(&from) {
                continue;
            }
            fs::copy(&from, &to)?;
        } else if fs::rename(&from, &to).is_err() {
            fs::copy(&from, &to)?;
            fs::remove_file(&from)?;
        }
        placed.push(to);
    }
    Ok(())
}

fn list_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            list_files(&entry.path(), files)?;
        } else {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// `path` with its leading `from` replaced by `to`, when `path` is `from` or
/// lies inside it; `None` otherwise, a path climbing out again through `..`
/// included.
#[must_use]
pub fn rebase(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    if rest
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    })
}

/// Remove `dir` and then its parents for as long as they are empty, never
/// `base` or anything outside it.
pub fn prune_empty_dirs(dir: &Path, base: &Path) {
    let mut dir = dir;
    while dir != base && dir.starts_with(base) && fs::remove_dir(dir).is_ok() {
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn sorted(mut paths: Vec<PathBuf>, base: &Path) -> Vec<String> {
        paths.sort();
        paths
            .iter()
            .map(|p| p.strip_prefix(base).unwrap().display().to_string())
            .collect()
    }

    /// A move to a free place takes the whole tree, nested directories
    /// included, and lists what it placed.
    #[test]
    fn a_move_to_a_free_place_takes_the_whole_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a/proj");
        let dst = tmp.path().join("b/renamed");
        write(&src.join("one.jsonl"), "1");
        write(&src.join("sub/two.jsonl"), "2");

        let placed = transfer_tree(&src, &dst, false, &|_| false).unwrap();

        assert_eq!(sorted(placed, &dst), ["one.jsonl", "sub/two.jsonl"]);
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(dst.join("sub/two.jsonl")).unwrap(), "2");
    }

    /// A move into a directory that holds something already merges, the
    /// moved files winning, and leaves nothing behind.
    #[test]
    fn a_move_merges_into_what_is_there() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("same"), "new");
        write(&src.join("sub/only-src"), "s");
        write(&dst.join("same"), "old");
        write(&dst.join("only-dst"), "d");

        let placed = transfer_tree(&src, &dst, false, &|_| false).unwrap();

        assert_eq!(sorted(placed, &dst), ["same", "sub/only-src"]);
        assert_eq!(fs::read_to_string(dst.join("same")).unwrap(), "new");
        assert!(dst.join("only-dst").exists());
        assert!(!src.exists());
    }

    /// A copy leaves the source whole and skips what it is told to.
    #[test]
    fn a_copy_keeps_the_source_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        write(&src.join("log.jsonl"), "x");
        write(&src.join("log.jsonl.lock"), "");

        let skip = |p: &Path| p.extension().is_some_and(|e| e == "lock");
        let placed = transfer_tree(&src, &dst, true, &skip).unwrap();

        assert_eq!(sorted(placed, &dst), ["log.jsonl"]);
        assert!(src.join("log.jsonl.lock").exists());
        assert!(!dst.join("log.jsonl.lock").exists());
    }

    #[test]
    fn a_missing_source_is_nothing_to_do() {
        let tmp = tempfile::tempdir().unwrap();
        let placed = transfer_tree(
            &tmp.path().join("none"),
            &tmp.path().join("d"),
            false,
            &|_| false,
        )
        .unwrap();
        assert!(placed.is_empty());
        assert!(!tmp.path().join("d").exists());
    }

    #[test]
    fn rebase_moves_only_what_lies_inside() {
        let (from, to) = (Path::new("/a/proj"), Path::new("/b/new"));
        let rebased = |p: &str| rebase(Path::new(p), from, to);
        assert_eq!(rebased("/a/proj"), Some(PathBuf::from("/b/new")));
        assert_eq!(
            rebased("/a/proj/src/x.rs"),
            Some(PathBuf::from("/b/new/src/x.rs"))
        );
        assert_eq!(rebased("/a/project"), None);
        assert_eq!(rebased("/a/proj/../other"), None);
        assert_eq!(rebased("/c"), None);
    }

    #[test]
    fn pruning_stops_at_the_base_and_at_anything_left() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        fs::create_dir_all(base.join("a/b/c")).unwrap();
        write(&base.join("a/keep"), "");
        prune_empty_dirs(&base.join("a/b/c"), &base);
        assert!(!base.join("a/b").exists());
        assert!(base.join("a").exists());
        fs::remove_file(base.join("a/keep")).unwrap();
        prune_empty_dirs(&base.join("a"), &base);
        assert!(!base.join("a").exists());
        assert!(base.exists());
    }
}
