//! Review of an agent's changes, the way Zed's agent review works: the
//! agent's edits land in the working tree at once and the agent goes on;
//! the user keeps or rejects them afterwards, hunk by hunk, against a base
//! that holds what was there before the agent plus what the user kept.
//!
//! Everything lives in the repository, so the agent panel that takes the
//! snapshots and the diff panel that reviews them share nothing but git:
//! - `refs/termide/review/<session>/base` — a tree: the working tree as the
//!   user accepts it. The review is the working tree's difference from it.
//! - `.../end` — a tree: the working tree when the agent's last request
//!   ended, which tells the user's edits between requests from the agent's.
//! - `.../index` — a tree: the index when the review began, which tells a
//!   hunk the user staged since from one that was staged already.
//! - `<git dir>/termide/review/<session>/both` — files the user edited while
//!   the agent's changes to them waited for review.
//! - `.../notes` — what the user rejected, for the agent's next request.
//!
//! The refs point at trees, not commits: `git log --all` and branch lists
//! never show them, and `git gc` keeps what they hold. Every tree is written
//! through a private index file; the user's own index is never touched. A
//! session's refs go once a request ends with nothing left to review.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::command::{git_command_stdout, hardened_git, nul_fields, path_from_git_bytes};
use crate::commits::PLAIN_PATCH;
use crate::patch::{
    chosen_changes, cut_part, hunk_headers, restore_blob, save_blob, PatchError, PatchPart,
};

const REFS: &str = "refs/termide/review";
/// Changed lines of a rejected hunk quoted to the agent.
const NOTE_LINES: usize = 8;
/// Files named in the note of a rejection of everything.
const NOTE_FILES: usize = 20;

/// The changes waiting for review, as [`Review::diff`] finds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDiff {
    /// The plain patch from the base to the working tree, every file.
    pub patch: String,
    /// Files both the user and the agent edited.
    pub both: Vec<PathBuf>,
}

/// A rejection, as [`Review::undo_reject`] takes it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    pub path: PathBuf,
    /// The file's content before, kept in the object store; `None` when
    /// there was no file.
    blob: Option<String>,
}

/// One agent session's review in one repository.
#[derive(Debug, Clone)]
pub struct Review {
    root: PathBuf,
    git_dir: PathBuf,
    session: String,
}

/// An index file of the review's own, removed when dropped.
struct TempIndex(PathBuf);

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// How [`Review::sync_end`] brings a file of the `end` tree up to date.
enum Sync<'a> {
    /// The file as this tree has it.
    From(&'a str),
    /// This patch applied backwards.
    Reverse(&'a [u8]),
}

impl Review {
    /// The review of `session` in the repository `dir` is in; `None`
    /// outside a repository with a working tree.
    #[must_use]
    pub fn open(dir: &Path, session: &str) -> Option<Self> {
        let out = git_command_stdout(dir, &["rev-parse", "--show-toplevel", "--absolute-git-dir"])?;
        let mut lines = out.lines();
        let root = PathBuf::from(lines.next()?);
        let git_dir = PathBuf::from(lines.next()?);
        let session = session
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Some(Self {
            root,
            git_dir,
            session,
        })
    }

    /// The repository's working tree.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether anything of the session waits for review, or a request of
    /// it is on.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.read("base").is_some()
    }

    /// A request of the agent starts. The first one takes the working tree
    /// as the base; a later one first takes over into the base the user's
    /// edits since the last ended, where the agent's changes do not wait,
    /// and marks the files where they do as edited by both. Returns what
    /// the user rejected since, for the agent, and forgets it.
    pub fn begin_request(&self) -> Result<Vec<String>, String> {
        let now = self.snapshot()?;
        match self.read("base") {
            None => {
                self.write("base", &now, "")?;
                if let Some(index) = self.index_tree() {
                    let old = self.read("index").unwrap_or_default();
                    self.write("index", &index, &old)?;
                }
            }
            Some(base) => {
                if let Some(end) = self.read("end") {
                    let user = self.changed(&end, &now)?;
                    if !user.is_empty() {
                        let agent: HashSet<PathBuf> =
                            self.changed(&base, &end)?.into_iter().collect();
                        let (both, own): (Vec<_>, Vec<_>) =
                            user.into_iter().partition(|path| agent.contains(path));
                        let folded = self.tree_with(&base, &own, &now)?;
                        if folded != base {
                            self.write("base", &folded, &base)?;
                        }
                        let mut marked = self.both();
                        for path in both {
                            if !marked.contains(&path) {
                                marked.push(path);
                            }
                        }
                        self.set_both(&marked);
                    }
                }
            }
        }
        Ok(self.take_notes())
    }

    /// A request of the agent ended: the working tree now is what the
    /// user's edits from here on are told apart from. With nothing left to
    /// review the session's refs go.
    pub fn end_request(&self) -> Result<(), String> {
        let Some(base) = self.read("base") else {
            return Ok(());
        };
        let now = self.snapshot()?;
        if now == base {
            self.clear();
            return Ok(());
        }
        let old = self.read("end").unwrap_or_default();
        self.write("end", &now, &old)
    }

    /// What waits for review, once the hunks the user staged since the
    /// review began are kept: staging or committing a change accepts it.
    /// `None` when nothing waits.
    pub fn diff(&self) -> Result<Option<ReviewDiff>, String> {
        let Some(base) = self.read("base") else {
            return Ok(None);
        };
        let now = self.snapshot()?;
        let base = self.keep_staged(&base, &now)?;
        let patch = self.patch(&base, &now, None)?;
        if patch.is_empty() {
            return Ok(None);
        }
        let changed: HashSet<PathBuf> = self.changed(&base, &now)?.into_iter().collect();
        let mut both = self.both();
        let before = both.len();
        both.retain(|path| changed.contains(path));
        if both.len() != before {
            self.set_both(&both);
        }
        Ok(Some(ReviewDiff {
            patch: String::from_utf8_lossy(&patch).into_owned(),
            both,
        }))
    }

    /// Keep `part` of the agent's changes to `path`, or all of them with
    /// `None`: the base takes them, and the review no longer shows them.
    pub fn keep(&self, path: &Path, part: Option<&PatchPart>) -> Result<(), PatchError> {
        let base = self.read("base").ok_or(PatchError::Stale)?;
        let now = self.snapshot().map_err(PatchError::Git)?;
        let tree = match part {
            None => self
                .tree_with(&base, &[path.to_path_buf()], &now)
                .map_err(PatchError::Git)?,
            Some(part) => {
                let diff = self
                    .patch(&base, &now, Some(path))
                    .map_err(PatchError::Git)?;
                let text = cut_part(&diff, part, false)?;
                let index = self.index_of(&base).map_err(PatchError::Git)?;
                self.apply(Some(&index), &text, false)
                    .map_err(PatchError::Git)?;
                self.write_tree(&index).map_err(PatchError::Git)?
            }
        };
        self.write("base", &tree, &base).map_err(PatchError::Git)
    }

    /// Keep everything that waits.
    pub fn keep_all(&self) -> Result<(), String> {
        let base = self.read("base").ok_or("nothing to review")?;
        let now = self.snapshot()?;
        self.write("base", &now, &base)?;
        self.set_both(&[]);
        Ok(())
    }

    /// Reject `part` of the agent's changes to `path`, or all of them with
    /// `None`: the file goes back to the base there, and the agent hears of
    /// it in its next request.
    pub fn reject(&self, path: &Path, part: Option<&PatchPart>) -> Result<Rejected, PatchError> {
        let base = self.read("base").ok_or(PatchError::Stale)?;
        let now = self.snapshot().map_err(PatchError::Git)?;
        let diff = self
            .patch(&base, &now, Some(path))
            .map_err(PatchError::Git)?;
        let name = path.to_string_lossy();
        let blob = if self.root.join(path).symlink_metadata().is_ok() {
            Some(save_blob(&self.root, path).map_err(PatchError::Git)?)
        } else {
            None
        };
        let note = match part {
            Some(part) => {
                let text = cut_part(&diff, part, true)?;
                self.apply(None, &text, true).map_err(PatchError::Git)?;
                self.sync_end(&[path.to_path_buf()], Sync::Reverse(&text));
                let changes = chosen_changes(&diff, part);
                let mut note = format!("{name} {}", part.header);
                for line in changes.iter().take(NOTE_LINES) {
                    note.push('\n');
                    note.push_str(line);
                }
                if changes.len() > NOTE_LINES {
                    note.push_str(&format!("\n… {} more", changes.len() - NOTE_LINES));
                }
                note
            }
            None => {
                self.restore(&base, path).map_err(PatchError::Git)?;
                self.sync_end(&[path.to_path_buf()], Sync::From(&base));
                format!("{name}: every change to the file")
            }
        };
        self.add_note(&note);
        Ok(Rejected {
            path: path.to_path_buf(),
            blob,
        })
    }

    /// Reject everything that waits, but in the files the user edited too;
    /// returns those, left as they are.
    pub fn reject_all(&self) -> Result<Vec<PathBuf>, String> {
        let base = self.read("base").ok_or("nothing to review")?;
        let now = self.snapshot()?;
        let both: HashSet<PathBuf> = self.both().into_iter().collect();
        let (skipped, paths): (Vec<_>, Vec<_>) = self
            .changed(&base, &now)?
            .into_iter()
            .partition(|path| both.contains(path));
        for path in &paths {
            self.restore(&base, path)?;
        }
        self.sync_end(&paths, Sync::From(&base));
        if !paths.is_empty() {
            let mut note = String::from("every change to");
            for path in paths.iter().take(NOTE_FILES) {
                note.push_str(&format!("\n{}", path.to_string_lossy()));
            }
            if paths.len() > NOTE_FILES {
                note.push_str(&format!("\n… {} more files", paths.len() - NOTE_FILES));
            }
            self.add_note(&note);
        }
        Ok(skipped)
    }

    /// Put back what `rejected` took away; the agent is no longer told of it.
    pub fn undo_reject(&self, rejected: &Rejected) -> Result<(), String> {
        match &rejected.blob {
            Some(id) => restore_blob(&self.root, &rejected.path, id)?,
            None => match std::fs::remove_file(self.root.join(&rejected.path)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {}
            },
        }
        let now = self.snapshot()?;
        self.sync_end(std::slice::from_ref(&rejected.path), Sync::From(&now));
        self.retract_note();
        Ok(())
    }

    /// The working tree as a tree object: tracked files as they are now,
    /// and the untracked ones git does not ignore.
    fn snapshot(&self) -> Result<String, String> {
        let index = self.copy_of_index()?;
        // A file that vanishes or cannot be read meanwhile is left as the
        // index had it rather than failing the snapshot.
        if let Err(error) = self.git(
            Some(&index),
            &["add", "-A", "--ignore-errors", "--", "."],
            None,
        ) {
            log::warn!("review snapshot: {error}");
        }
        self.write_tree(&index)
    }

    /// The tree of the user's index, when it has no conflicts.
    fn index_tree(&self) -> Option<String> {
        let index = self.copy_of_index().ok()?;
        self.write_tree(&index).ok()
    }

    /// The review's base with the hunks and files the user staged since the
    /// review began taken over from `now`. A hunk counts as staged when the
    /// index holds its new side and did not when the review began, so a
    /// change of the agent that only puts back what the index had is not
    /// taken for one the user staged.
    fn keep_staged(&self, base: &str, now: &str) -> Result<String, String> {
        let paths = self.changed(base, now)?;
        if paths.is_empty() {
            return Ok(base.to_string());
        }
        let staged = self.index_entries(&paths)?;
        let current = self.tree_entries(now, &paths)?;
        let start_tree = self.read("index");
        let start = match &start_tree {
            Some(tree) => self.tree_entries(tree, &paths)?,
            None => HashMap::new(),
        };
        let start_index = match &start_tree {
            Some(tree) => Some(self.index_of(tree)?),
            None => None,
        };
        let mut whole = Vec::new();
        let mut hunks = Vec::new();
        for path in paths {
            let now_entry = current.get(&path);
            let staged_entry = staged.get(&path);
            if staged_entry == now_entry {
                if start.get(&path) != now_entry {
                    whole.push(path);
                }
                continue;
            }
            let (Some(start_index), Some(_), Some(_)) = (&start_index, staged_entry, now_entry)
            else {
                continue;
            };
            let diff = self.patch(base, now, Some(&path))?;
            for (i, header) in hunk_headers(&diff).into_iter().enumerate().rev() {
                let part = PatchPart {
                    hunk: i,
                    header,
                    lines: None,
                };
                let Ok(text) = cut_part(&diff, &part, false) else {
                    continue;
                };
                if self.applies_backwards(None, &text)
                    && !self.applies_backwards(Some(start_index), &text)
                {
                    hunks.push(text);
                }
            }
        }
        let mut tree = self.tree_with(base, &whole, now)?;
        if !hunks.is_empty() {
            let index = self.index_of(&tree)?;
            for text in &hunks {
                if let Err(error) = self.apply(Some(&index), text, false) {
                    log::warn!("review: a staged hunk does not apply to the base: {error}");
                }
            }
            tree = self.write_tree(&index)?;
        }
        if tree != base {
            self.write("base", &tree, base)?;
        }
        Ok(tree)
    }

    /// Bring `paths` of the `end` tree in line with a review action on the
    /// working tree, so the next request does not take it for the user's
    /// edit. A patch that does not apply there takes the file as it is now.
    fn sync_end(&self, paths: &[PathBuf], how: Sync) {
        let Some(end) = self.read("end") else {
            return;
        };
        let synced = match how {
            Sync::From(tree) => self.tree_with(&end, paths, tree),
            Sync::Reverse(text) => self
                .index_of(&end)
                .and_then(|index| {
                    self.apply(Some(&index), text, true)?;
                    self.write_tree(&index)
                })
                .or_else(|_| {
                    let now = self.snapshot()?;
                    self.tree_with(&end, paths, &now)
                }),
        };
        match synced.and_then(|tree| self.write("end", &tree, &end)) {
            Ok(()) => {}
            Err(error) => log::warn!("review: cannot update the end of the request: {error}"),
        }
    }

    /// Write `path` as `tree` has it, or remove it where `tree` lacks it.
    fn restore(&self, tree: &str, path: &Path) -> Result<(), String> {
        if self.tree_entries(tree, &[path.to_path_buf()])?.is_empty() {
            return match std::fs::remove_file(self.root.join(path)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            };
        }
        let index = self.index_of(tree)?;
        let name = path.to_string_lossy();
        self.git(Some(&index), &["checkout-index", "-f", "--", &name], None)
            .map(drop)
    }

    /// `tree` with `paths` as `from` has them: taken over, or removed where
    /// `from` lacks them.
    fn tree_with(&self, tree: &str, paths: &[PathBuf], from: &str) -> Result<String, String> {
        if paths.is_empty() {
            return Ok(tree.to_string());
        }
        let entries = self.tree_entries(from, paths)?;
        let zeros = "0".repeat(from.len());
        let mut info = Vec::new();
        for path in paths {
            match entries.get(path) {
                Some(entry) => info.extend_from_slice(entry.as_bytes()),
                None => info.extend_from_slice(format!("0 {zeros}").as_bytes()),
            }
            info.push(b'\t');
            info.extend_from_slice(path.to_string_lossy().as_bytes());
            info.push(0);
        }
        let index = self.index_of(tree)?;
        self.git(
            Some(&index),
            &["update-index", "-z", "--index-info"],
            Some(&info),
        )?;
        self.write_tree(&index)
    }

    /// `mode object` of each of `paths` that `tree` holds.
    fn tree_entries(
        &self,
        tree: &str,
        paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, String>, String> {
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut args = vec!["ls-tree", "-r", "-z", "--full-tree", tree, "--"];
        args.extend(names.iter().map(String::as_str));
        let out = self.git(None, &args, None)?;
        let wanted: HashSet<&PathBuf> = paths.iter().collect();
        let mut entries = HashMap::new();
        for field in nul_fields(&out) {
            // `<mode> <type> <object>\t<path>`
            let Some(tab) = field.iter().position(|&b| b == b'\t') else {
                continue;
            };
            let path = path_from_git_bytes(&field[tab + 1..]);
            let meta = String::from_utf8_lossy(&field[..tab]);
            let mut words = meta.split(' ');
            let (Some(mode), Some(_), Some(object)) = (words.next(), words.next(), words.next())
            else {
                continue;
            };
            if wanted.contains(&path) {
                entries.insert(path, format!("{mode} {object}"));
            }
        }
        Ok(entries)
    }

    /// `mode object` of each of `paths` in the user's index, unconflicted.
    fn index_entries(&self, paths: &[PathBuf]) -> Result<HashMap<PathBuf, String>, String> {
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut args = vec!["ls-files", "-s", "-z", "--"];
        args.extend(names.iter().map(String::as_str));
        let out = self.git(None, &args, None)?;
        let mut entries = HashMap::new();
        for field in nul_fields(&out) {
            // `<mode> <object> <stage>\t<path>`
            let Some(tab) = field.iter().position(|&b| b == b'\t') else {
                continue;
            };
            let meta = String::from_utf8_lossy(&field[..tab]);
            let mut words = meta.split(' ');
            let (Some(mode), Some(object), Some("0")) = (words.next(), words.next(), words.next())
            else {
                continue;
            };
            entries.insert(
                path_from_git_bytes(&field[tab + 1..]),
                format!("{mode} {object}"),
            );
        }
        Ok(entries)
    }

    /// Files that differ between trees `a` and `b`.
    fn changed(&self, a: &str, b: &str) -> Result<Vec<PathBuf>, String> {
        let out = self.git(
            None,
            &["diff-tree", "-r", "-z", "--name-only", "--no-renames", a, b],
            None,
        )?;
        Ok(nul_fields(&out).map(path_from_git_bytes).collect())
    }

    /// The plain patch from tree `a` to tree `b`, of `path` alone when given.
    fn patch(&self, a: &str, b: &str, path: Option<&Path>) -> Result<Vec<u8>, String> {
        let name = path.map(|p| p.to_string_lossy().into_owned());
        let mut args = vec!["diff"];
        args.extend(PLAIN_PATCH);
        args.extend(["--no-renames", "--no-textconv", a, b]);
        if let Some(name) = &name {
            args.extend(["--", name]);
        }
        self.git(None, &args, None)
    }

    /// Apply `text` to `index` (`--cached`), or to the working tree.
    fn apply(&self, index: Option<&TempIndex>, text: &[u8], reverse: bool) -> Result<(), String> {
        let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
        if index.is_some() {
            args.push("--cached");
        }
        if reverse {
            args.push("-R");
        }
        args.push("-");
        self.git(index, &args, Some(text)).map(drop)
    }

    /// Whether `text` goes backwards onto `index`, or the user's index:
    /// whether that holds its new side.
    fn applies_backwards(&self, index: Option<&TempIndex>, text: &[u8]) -> bool {
        self.git(
            index,
            &["apply", "--cached", "--check", "-R", "--recount", "-"],
            Some(text),
        )
        .is_ok()
    }

    fn git(
        &self,
        index: Option<&TempIndex>,
        args: &[&str],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        let mut cmd = hardened_git(&self.root, args);
        cmd.env("GIT_LITERAL_PATHSPECS", "1");
        if let Some(index) = index {
            cmd.env("GIT_INDEX_FILE", &index.0);
        }
        crate::command::run_with_input(cmd, args, input)
    }

    fn temp_index(&self) -> Result<TempIndex, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Ok(TempIndex(
            dir.join(format!("index.{}.{n}", std::process::id())),
        ))
    }

    /// A private index holding `tree`.
    fn index_of(&self, tree: &str) -> Result<TempIndex, String> {
        let index = self.temp_index()?;
        self.git(Some(&index), &["read-tree", tree], None)?;
        Ok(index)
    }

    /// A private copy of the user's index, which keeps its file stats so
    /// adding the working tree to it reads only files that changed.
    fn copy_of_index(&self) -> Result<TempIndex, String> {
        let index = self.temp_index()?;
        let real = self.git_dir.join("index");
        if real.is_file() {
            std::fs::copy(&real, &index.0).map_err(|e| e.to_string())?;
        }
        Ok(index)
    }

    fn write_tree(&self, index: &TempIndex) -> Result<String, String> {
        let out = self.git(Some(index), &["write-tree"], None)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    fn reference(&self, name: &str) -> String {
        format!("{REFS}/{}/{name}", self.session)
    }

    fn read(&self, name: &str) -> Option<String> {
        let out = self
            .git(
                None,
                &["rev-parse", "-q", "--verify", &self.reference(name)],
                None,
            )
            .ok()?;
        Some(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// Point ref `name` at `tree` if it still points at `old` (`""`: if it
    /// does not exist), so two panels acting at once lose nothing.
    fn write(&self, name: &str, tree: &str, old: &str) -> Result<(), String> {
        self.git(
            None,
            &["update-ref", &self.reference(name), tree, old],
            None,
        )
        .map(drop)
    }

    /// Forget the review: its refs and the files edited by both.
    fn clear(&self) {
        for name in ["base", "end", "index"] {
            let _ = self.git(None, &["update-ref", "-d", &self.reference(name)], None);
        }
        let _ = std::fs::remove_file(self.dir().join("both"));
    }

    fn dir(&self) -> PathBuf {
        self.git_dir
            .join("termide")
            .join("review")
            .join(&self.session)
    }

    fn both(&self) -> Vec<PathBuf> {
        std::fs::read(self.dir().join("both"))
            .map(|bytes| nul_fields(&bytes).map(path_from_git_bytes).collect())
            .unwrap_or_default()
    }

    fn set_both(&self, paths: &[PathBuf]) {
        let file = self.dir().join("both");
        if paths.is_empty() {
            let _ = std::fs::remove_file(file);
            return;
        }
        let mut bytes = Vec::new();
        for path in paths {
            bytes.extend_from_slice(path.to_string_lossy().as_bytes());
            bytes.push(0);
        }
        if let Err(error) = std::fs::create_dir_all(self.dir())
            .and_then(|()| termide_fsutil::write_atomic(&file, &bytes))
        {
            log::warn!("review: cannot mark files edited by both: {error}");
        }
    }

    fn notes(&self) -> Vec<String> {
        std::fs::read(self.dir().join("notes"))
            .map(|bytes| {
                nul_fields(&bytes)
                    .map(|note| String::from_utf8_lossy(note).into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set_notes(&self, notes: &[String]) {
        let file = self.dir().join("notes");
        if notes.is_empty() {
            let _ = std::fs::remove_file(file);
            return;
        }
        let bytes: Vec<u8> = notes
            .iter()
            .flat_map(|note| note.bytes().chain(std::iter::once(0)))
            .collect();
        if let Err(error) = std::fs::create_dir_all(self.dir())
            .and_then(|()| termide_fsutil::write_atomic(&file, &bytes))
        {
            log::warn!("review: cannot keep the note for the agent: {error}");
        }
    }

    fn add_note(&self, note: &str) {
        let mut notes = self.notes();
        notes.push(note.to_string());
        self.set_notes(&notes);
    }

    fn retract_note(&self) {
        let mut notes = self.notes();
        notes.pop();
        self.set_notes(&notes);
    }

    fn take_notes(&self) -> Vec<String> {
        let notes = self.notes();
        self.set_notes(&[]);
        notes
    }
}

#[cfg(test)]
mod tests;
