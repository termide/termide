//! Which termide instance has a project open.
//!
//! A project is open in one instance at a time. Two would save its layout
//! over each other, run its language servers twice and try again, twice,
//! the agent requests it left failed.
//!
//! The instance holding a project holds an exclusive lock on a file kept for
//! it. The operating system releases the lock when the process ends, however
//! it ends, so a crash or a `kill -9` never leaves a project taken by nobody.
//! Who holds it is written beside the lock, in a file of its own: Windows
//! does not let another process read a locked file.
//!
//! The files live apart from the saved layouts, under `locks/`, and are never
//! deleted: deleting a lock file that is held would let the next instance
//! lock a new file of the same name while the old lock still stands.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{get_data_dir, project_key};

const LOCKS_DIR: &str = "locks";
const LOCK_FILE: &str = "owner.lock";
const OWNER_FILE: &str = "owner.toml";

/// The instance a project is open in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    /// Its process id; 0 when it could not be read.
    #[serde(default)]
    pub pid: u32,
    /// The id of the detached instance it runs in, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

/// A project held by this process. Dropping it lets the project go.
#[derive(Debug)]
pub struct ProjectClaim {
    _lock: File,
}

/// Take the project at `root` for this process, which runs in detached
/// instance `instance`, if any.
///
/// `Err` names the instance that has it. `Ok(None)` when no lock could be
/// set up at all — no data directory, a read-only one, a file system that
/// does not lock: the project then opens unguarded, as before projects were
/// claimed, rather than not at all.
pub fn claim_project(root: &Path, instance: Option<&str>) -> Result<Option<ProjectClaim>, Owner> {
    match get_data_dir() {
        Ok(dir) => claim_in(&lock_dir(&dir, root), instance),
        Err(e) => {
            log::warn!("Projects are not guarded against a second instance: {e:#}");
            Ok(None)
        }
    }
}

/// The instance the project at `root` is open in, `None` when it is free.
///
/// Takes the project and lets it go at once, so the answer may be stale by
/// the time it is used; [`claim_project`] is what decides.
pub fn project_owner(root: &Path) -> Option<Owner> {
    claim_project(root, None).err()
}

fn lock_dir(data_dir: &Path, root: &Path) -> PathBuf {
    data_dir.join(LOCKS_DIR).join(project_key(root))
}

fn claim_in(dir: &Path, instance: Option<&str>) -> Result<Option<ProjectClaim>, Owner> {
    let lock = match fs::create_dir_all(dir).and_then(|()| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(LOCK_FILE))
    }) {
        Ok(file) => file,
        Err(e) => {
            log::warn!(
                "Cannot guard {} against a second instance: {e}",
                dir.display()
            );
            return Ok(None);
        }
    };
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(read_owner(dir)),
        Err(TryLockError::Error(e)) => {
            log::warn!("Cannot lock {}: {e}", dir.display());
            return Ok(None);
        }
    }
    let owner = Owner {
        pid: std::process::id(),
        instance: instance.map(str::to_owned),
    };
    if let Err(e) = write_owner(dir, &owner) {
        // The lock is what guards; without the note a second instance is
        // still kept out, only told less about who has the project.
        log::warn!("Cannot record the owner in {}: {e}", dir.display());
    }
    Ok(Some(ProjectClaim { _lock: lock }))
}

fn write_owner(dir: &Path, owner: &Owner) -> std::io::Result<()> {
    let text = toml::to_string(owner).map_err(std::io::Error::other)?;
    let mut file = File::create(dir.join(OWNER_FILE))?;
    file.write_all(text.as_bytes())
}

/// Who holds the lock in `dir`. Unknown fields read as defaults: the holder
/// may have taken the lock and not written its note yet.
fn read_owner(dir: &Path) -> Owner {
    fs::read_to_string(dir.join(OWNER_FILE))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_project_is_refused_until_let_go() {
        let data = tempfile::tempdir().unwrap();
        let dir = lock_dir(data.path(), Path::new("/src/demo"));

        let claim = claim_in(&dir, Some("demo")).unwrap();
        assert!(claim.is_some());
        // A second open of the lock file conflicts even in this process,
        // as it does in another one.
        let owner = claim_in(&dir, None).unwrap_err();
        assert_eq!(owner.pid, std::process::id());
        assert_eq!(owner.instance.as_deref(), Some("demo"));

        drop(claim);
        let again = claim_in(&dir, None).unwrap();
        assert!(again.is_some(), "a project let go can be taken again");
        assert_eq!(read_owner(&dir).instance, None, "the new owner is recorded");
    }

    #[test]
    fn projects_are_claimed_one_by_one() {
        let data = tempfile::tempdir().unwrap();
        let a = claim_in(&lock_dir(data.path(), Path::new("/src/a")), None).unwrap();
        let b = claim_in(&lock_dir(data.path(), Path::new("/src/b")), None).unwrap();
        assert!(a.is_some() && b.is_some());
    }

    #[test]
    fn an_owner_without_a_note_is_still_an_owner() {
        let data = tempfile::tempdir().unwrap();
        let dir = lock_dir(data.path(), Path::new("/src/demo"));
        let _claim = claim_in(&dir, None).unwrap();
        fs::remove_file(dir.join(OWNER_FILE)).unwrap();
        assert_eq!(claim_in(&dir, None).unwrap_err(), Owner::default());
    }
}
