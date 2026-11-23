//! Which termide instance has a project open.
//!
//! A project is open in one instance at a time. Two would save its layout
//! over each other, run its language servers twice and try again, twice,
//! the agent requests it left failed.
//!
//! The instance holding a project holds an exclusive lock on a file in the
//! project's directory in the store. The operating system releases the lock
//! when the process ends, however it ends, so a crash or a `kill -9` never
//! leaves a project taken by nobody. Who holds it is written beside the
//! lock, in a file of its own: Windows does not let another process read a
//! locked file.
//!
//! A lock file is deleted only by the one holding it: deleting a lock file
//! another instance holds would let the next instance lock a new file of the
//! same name while the old lock still stands. The holder deletes it when the
//! project's directory goes, or when the claim ends and the directory holds
//! nothing else. One that opened the file just before it went finds, once it
//! has the lock, that the path no longer leads to the file it locked, and
//! tries again on the new one.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::get_data_dir;
use crate::store::{holds_only_owner, project_dir_in, remove_owner_files};

pub(crate) const LOCK_FILE: &str = "owner.lock";
pub(crate) const OWNER_FILE: &str = "owner.toml";

/// How many times a lock file deleted under a claim is opened again.
const CLAIM_TRIES: usize = 3;

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

/// A project held by this process. Dropping it lets the project go, and
/// removes its directory in the store when nothing but the lock is left
/// there: a project opened and never saved leaves nothing behind.
#[derive(Debug)]
pub struct ProjectClaim {
    lock: Option<File>,
    dir: PathBuf,
}

impl Drop for ProjectClaim {
    fn drop(&mut self) {
        if holds_only_owner(&self.dir) {
            // Still held: no one else can have it meanwhile.
            remove_owner_files(&self.dir);
        }
        drop(self.lock.take());
    }
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
        Ok(dir) => claim_in(&project_dir_in(&dir, root), instance),
        Err(e) => {
            log::warn!("Projects are not guarded against a second instance: {e:#}");
            Ok(None)
        }
    }
}

/// The instance the project at `root` is open in, `None` when it is free.
///
/// Tries the lock and lets it go at once, so the answer may be stale by the
/// time it is used; [`claim_project`] is what decides. Only looks: it writes
/// no owner note and creates no lock file — a project with none is free.
pub fn project_owner(root: &Path) -> Option<Owner> {
    owner_in(&project_dir_in(&get_data_dir().ok()?, root))
}

pub(crate) fn owner_in(dir: &Path) -> Option<Owner> {
    let lock = File::open(dir.join(LOCK_FILE)).ok()?;
    match lock.try_lock() {
        // Free; dropping the file lets the lock go.
        Ok(()) => None,
        Err(TryLockError::WouldBlock) => Some(read_owner(dir)),
        Err(TryLockError::Error(e)) => {
            log::warn!("Cannot lock {}: {e}", dir.display());
            None
        }
    }
}

pub(crate) fn claim_in(dir: &Path, instance: Option<&str>) -> Result<Option<ProjectClaim>, Owner> {
    for _ in 0..CLAIM_TRIES {
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
        if !still_at(&lock, &dir.join(LOCK_FILE)) {
            // Its holder deleted it between our open and our lock.
            continue;
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
        return Ok(Some(ProjectClaim {
            lock: Some(lock),
            dir: dir.to_path_buf(),
        }));
    }
    log::warn!("Cannot keep a lock file in {}", dir.display());
    Ok(None)
}

/// Whether `path` still leads to the file `file` is open on.
#[cfg(unix)]
fn still_at(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), fs::metadata(path)) {
        (Ok(open), Ok(named)) => open.dev() == named.dev() && open.ino() == named.ino(),
        _ => false,
    }
}

/// Whether `path` still leads to the file `file` is open on. The standard
/// library gives no stable file identity off Unix: the name still being
/// there is the check.
#[cfg(not(unix))]
fn still_at(_file: &File, path: &Path) -> bool {
    path.exists()
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

    fn dir_of(data: &Path, root: &str) -> PathBuf {
        project_dir_in(data, Path::new(root))
    }

    #[test]
    fn a_claimed_project_is_refused_until_let_go() {
        let data = tempfile::tempdir().unwrap();
        let dir = dir_of(data.path(), "/src/demo");
        // Something to keep, so letting go leaves the directory.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("project.toml"), "root = \"/src/demo\"\n").unwrap();

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
        let a = claim_in(&dir_of(data.path(), "/src/a"), None).unwrap();
        let b = claim_in(&dir_of(data.path(), "/src/b"), None).unwrap();
        assert!(a.is_some() && b.is_some());
    }

    #[test]
    fn an_owner_without_a_note_is_still_an_owner() {
        let data = tempfile::tempdir().unwrap();
        let dir = dir_of(data.path(), "/src/demo");
        let _claim = claim_in(&dir, None).unwrap();
        fs::remove_file(dir.join(OWNER_FILE)).unwrap();
        assert_eq!(claim_in(&dir, None).unwrap_err(), Owner::default());
    }

    /// A project opened and let go without anything saved leaves no
    /// directory; one with something kept keeps it, lock file and all.
    #[test]
    fn letting_go_of_a_project_with_nothing_kept_removes_its_directory() {
        let data = tempfile::tempdir().unwrap();
        let bare = dir_of(data.path(), "/src/bare");
        drop(claim_in(&bare, None).unwrap());
        assert!(!bare.exists());

        let kept = dir_of(data.path(), "/src/kept");
        let claim = claim_in(&kept, None).unwrap();
        fs::write(kept.join("project.toml"), "root = \"/src/kept\"\n").unwrap();
        drop(claim);
        assert!(kept.join("project.toml").exists());
        assert!(kept.join(LOCK_FILE).exists());
    }

    /// The lock file deleted by its holder after another process opened it:
    /// the other takes the new file, not the one that is gone.
    #[cfg(unix)]
    #[test]
    fn a_lock_file_deleted_under_a_claim_is_not_taken() {
        let data = tempfile::tempdir().unwrap();
        let dir = dir_of(data.path(), "/src/demo");
        fs::create_dir_all(&dir).unwrap();
        let stale = File::create(dir.join(LOCK_FILE)).unwrap();
        fs::remove_file(dir.join(LOCK_FILE)).unwrap();
        assert!(!still_at(&stale, &dir.join(LOCK_FILE)));
        fs::write(dir.join(LOCK_FILE), "").unwrap();
        assert!(!still_at(&stale, &dir.join(LOCK_FILE)));
        let fresh = File::open(dir.join(LOCK_FILE)).unwrap();
        assert!(still_at(&fresh, &dir.join(LOCK_FILE)));
    }

    #[test]
    fn asking_for_the_owner_only_looks() {
        let data = tempfile::tempdir().unwrap();
        let dir = dir_of(data.path(), "/src/demo");
        assert_eq!(owner_in(&dir), None);
        assert!(!dir.exists(), "asking creates nothing");

        let claim = claim_in(&dir, Some("demo")).unwrap();
        let note = fs::read_to_string(dir.join(OWNER_FILE)).unwrap();
        let owner = owner_in(&dir).unwrap();
        assert_eq!(owner.instance.as_deref(), Some("demo"));
        assert_eq!(
            fs::read_to_string(dir.join(OWNER_FILE)).unwrap(),
            note,
            "the holder's note is left as it was"
        );
        // A lock left by a probe would keep the holder's successor out.
        fs::write(dir.join("project.toml"), "").unwrap();
        drop(claim);
        assert_eq!(owner_in(&dir), None);
        assert!(claim_in(&dir, None).unwrap().is_some());
    }

    #[test]
    fn asking_for_the_owner_of_a_project_never_opened_leaves_nothing() {
        // Through the real data directory: only reads, as the path is new.
        let root = Path::new("/nonexistent/termide-owner-probe");
        assert_eq!(project_owner(root), None);
        if let Ok(data) = get_data_dir() {
            assert!(!project_dir_in(&data, root).exists());
        }
    }
}
