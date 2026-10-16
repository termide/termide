//! The source line a debugger stopped at, for editors to mark. The app sets
//! it from the session; editors read it as they draw.

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
    let same = path == file
        || path.file_name() == file.file_name()
            && dunce::canonicalize(path)
                .ok()
                .is_some_and(|p| dunce::canonicalize(file).ok().as_deref() == Some(p.as_path()));
    same.then(|| line.saturating_sub(1))
}
