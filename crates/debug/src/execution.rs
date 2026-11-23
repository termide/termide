//! The source line a debugger stopped at, and the breakpoints it could not
//! set, for editors to mark. The app sets them from the session; editors
//! read them as they draw.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static POINT: Mutex<Option<(PathBuf, usize)>> = Mutex::new(None);

/// Set or clear the point (file, 1-based line). Returns whether it changed.
pub fn set(point: Option<(PathBuf, usize)>) -> bool {
    let mut current = POINT.lock().unwrap_or_else(|e| e.into_inner());
    if *current == point {
        return false;
    }
    *current = point;
    true
}

pub fn get() -> Option<(PathBuf, usize)> {
    POINT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The 0-based line of `file` the program stopped at, when it stopped in
/// `file`. The adapter names files as the program sees them, through
/// symlinks, so a path that differs from the editor's is compared resolved.
pub fn line_in(file: &Path) -> Option<usize> {
    let guard = POINT.lock().unwrap_or_else(|e| e.into_inner());
    let (path, line) = guard.as_ref()?;
    same_file(path, file).then(|| line.saturating_sub(1))
}

static UNVERIFIED: Mutex<Vec<(PathBuf, usize)>> = Mutex::new(Vec::new());

/// Set the breakpoints the running session's adapter could not set (file,
/// 1-based line); empty when there is no session. Returns whether it
/// changed.
pub fn set_unverified(mut breakpoints: Vec<(PathBuf, usize)>) -> bool {
    breakpoints.sort();
    let mut current = UNVERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    if *current == breakpoints {
        return false;
    }
    *current = breakpoints;
    true
}

/// The 0-based lines of `file` whose breakpoints the debugger could not set.
pub fn unverified_in(file: &Path) -> BTreeSet<usize> {
    let guard = UNVERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .iter()
        .filter(|(path, _)| same_file(path, file))
        .map(|(_, line)| line.saturating_sub(1))
        .collect()
}

/// Whether the debugger's `path` is the editor's `file`; see [`line_in`].
fn same_file(path: &Path, file: &Path) -> bool {
    path == file
        || path.file_name() == file.file_name()
            && dunce::canonicalize(path)
                .ok()
                .is_some_and(|p| dunce::canonicalize(file).ok().as_deref() == Some(p.as_path()))
}
