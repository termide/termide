//! Append-only JSONL session log.
//!
//! One file per session. The first line is the header, every later line an
//! entry with an `id` and a `parent_id`, so the file is a tree even though
//! the UI only ever walks one path (leaf to root) today. Moving the leaf to
//! an earlier entry and appending starts a new branch without rewriting
//! anything.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::acp::ACP_PROVIDER;
use crate::compaction::{CompactionCall, CompactionPrompts};
use crate::context::file_timestamp;
use crate::failure::Failure;
use crate::fit::{fit, FitStep};
use crate::message::{now_millis, Message, StopReason};
use crate::permissions::Mode;
use crate::provider::ThinkingLevel;
use crate::prune::{prune_by, prune_to_decisions};

/// Bumped when a line shape changes incompatibly.
pub const SESSION_FORMAT_VERSION: u32 = 1;

/// First line of a session file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeader {
    pub version: u32,
    pub id: String,
    pub created: u64,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The session that delegated this one's task, for a delegate's session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionParent>,
}

/// Where a delegate's session came from: the delegating session and call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionParent {
    /// The delegating session's id, when its agent ran on termide's loop;
    /// an external agent's calls come without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The delegating agent's name.
    pub agent: String,
    /// The id of the `task` call that handed the task over.
    pub call: String,
}

/// How long a logged message took to produce, recorded by the UI so a
/// reopened session shows the same figures. Token counts are not repeated
/// here: a model turn's already travel in its `usage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    /// A model turn: from the request to the first token, and from there to
    /// the end of the reply.
    Turn { prefill_ms: u32, gen_ms: u32 },
    /// A tool call: how long it ran, and how long it waited on a
    /// permission answer before that (kept apart, as a pause).
    Tool {
        duration_ms: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        waited_ms: Option<u32>,
    },
}

/// A message on the current branch as the log recorded it.
#[derive(Debug, Clone, PartialEq)]
pub struct LoggedMessage {
    pub message: Message,
    /// When the entry was written (millis).
    pub timestamp: u64,
    pub timing: Option<Timing>,
}

/// Payload of a tree entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
    Message {
        message: Message,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<Timing>,
    },
    /// The model the branch runs on from here. The context window is the
    /// figure the panel knew at the time (configured or reported by the
    /// endpoint), so a resume needs nothing but the log.
    ModelChange {
        provider: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// The `keep_last` messages before this entry stay verbatim; everything
    /// earlier on the branch is replaced by `summary`.
    Compaction {
        summary: String,
        tokens_before: u64,
        keep_last: usize,
        /// The estimated context right after; absent from older logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens_after: Option<u64>,
        /// How the summary call went; absent from older logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call: Option<CompactionCall>,
    },
    /// A compaction whose summary call was made and refused: the context
    /// went on as it was. `reply` is the refused text, cut short. Changes
    /// nothing on the branch; it is there to tell afterwards what happened.
    CompactionFailed {
        error: String,
        tokens_before: u64,
        call: CompactionCall,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reply: String,
    },
    /// The context was cut down by kinds of content to `budget` characters,
    /// the recent ones within `keep_tail` kept whole, with no summary: see
    /// [`crate::fit::fit`], which replays it from the same inputs. `steps`
    /// and `tokens_before` record what it did, for judging it afterwards.
    Fitted {
        budget: usize,
        keep_tail: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        steps: Vec<(FitStep, usize)>,
        #[serde(default)]
        tokens_before: u64,
    },
    /// A name the user gave this conversation. The last one on the branch
    /// wins, so renaming is just another entry.
    SessionName { name: String },
    /// The agent definition the branch runs as from here, so a reopened
    /// session comes back with the same prompt and tools.
    AgentChange { agent: String },
    /// Whether the branch prefers reasoning from here: the on/off switch
    /// that [`EntryKind::ThinkingChange`] replaced, still read from older
    /// logs.
    ReasoningChange { reasoning: bool },
    /// The reasoning level the branch asks for from here, so a reopened
    /// session comes back with the same choice (picked from the status bar).
    ThinkingChange { level: ThinkingLevel },
    /// What the session switched off from here: tools, skills (`skill:<name>`)
    /// and MCP servers (`mcp:<name>`), kept out of the model's context or
    /// refused, so a reopened session comes back with the same set.
    Toolset { disabled: Vec<String> },
    /// The connection (`[ai.connections.<name>]`) the branch runs on from
    /// here, so a reopened session reconnects to the same endpoint.
    ConnectionChange { connection: String },
    /// The permission mode the branch runs in from here, so a reopened
    /// session — a paused run above all — goes on under the same mode.
    ModeChange { mode: Mode },
    /// The user undid a request: the branch continues from this entry's
    /// parent, the undone messages stay in the file on a dead branch.
    Rewind,
    /// A plan was carried out from a clean context: the messages before
    /// this entry are cut down to the decisions, see
    /// [`crate::prune::prune_to_decisions`]. They stay in the file whole.
    Pruned,
    /// An external agent opened its own session for this branch: `agent` is
    /// the name it runs under, `session_id` the id it gave, `log` the header
    /// id and `cwd` the working directory of this log when it did. A
    /// reopened session asks the agent to resume it, see
    /// [`Session::external_session`].
    ExternalSession {
        agent: String,
        session_id: String,
        log: String,
        cwd: PathBuf,
    },
    /// A setting of the external agent's own picked in the panel from here
    /// (its model, reasoning effort, …, under the id it offers it by), so a
    /// reopened session asks the agent for the same again, see
    /// [`Session::agent_options`].
    AgentOption { option: String, value: String },
    /// A run started: until a [`EntryKind::RunEnd`] follows, it is under
    /// way, so a session reopened after the panel or termide closed mid-run
    /// knows the run was cut off, see [`Session::last_run`].
    RunStart,
    /// The run ended — `paused` when it stopped at a `/pause` with work
    /// left for `/continue`.
    RunEnd {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        paused: bool,
    },
    /// The unattended work (`/goal`, `/loop`) the branch has going from here;
    /// empty once it stopped. A reopened session offers to carry it on, see
    /// [`Session::autorun`].
    Autorun { autorun: Autorun },
    /// A request that failed on something that passes — a limit, an outage —
    /// and was asked about or is waited on, with what it was: the card the
    /// panel raised, so a session reopened after termide closed offers the
    /// same again. `None` takes the offer back (the user gave the request up,
    /// or it was tried again). See [`Session::pending_retry`].
    Retry {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<Failure>,
    },
}

/// The unattended work a session has going: a `/goal`, a `/loop`, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Autorun {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<GoalRecord>,
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub repeat: Option<LoopRecord>,
}

impl Autorun {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.goal.is_none() && self.repeat.is_none()
    }
}

/// A `/goal` under way: what it aims at and the work turns sent so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRecord {
    pub goal: String,
    pub iterations: usize,
}

/// A `/loop` under way: its prompt, the wait between runs (`None`: back to
/// back) and the runs sent so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopRecord {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    pub iterations: usize,
    /// When the next run is due (epoch millis), while the loop waits out its
    /// interval, so a reopen waits only what is left of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_ms: Option<u64>,
}

/// A run the branch left unfinished, see [`Session::unfinished_run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunMark {
    /// [`LastRun::Paused`] or [`LastRun::CutOff`].
    pub state: LastRun,
    /// When the run started and when it stopped (the pause, or the last
    /// thing it logged), epoch millis.
    pub started: u64,
    pub stopped: u64,
}

/// How the branch's last run ended, see [`Session::last_run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastRun {
    /// It ended (or there was none).
    Ended,
    /// It stopped at a `/pause`, its remaining work left for `/continue`.
    Paused,
    /// It never ended: the panel or termide closed while it ran.
    CutOff,
}

/// An external agent's own session that still holds this branch's
/// conversation, see [`Session::external_session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSessionRef {
    pub agent: String,
    pub session_id: String,
}

/// The model a session last recorded, see [`Session::current_model`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionModel {
    pub provider: String,
    pub id: String,
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: u64,
    #[serde(flatten)]
    pub kind: EntryKind,
}

/// One line of the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Line {
    Header(HeaderLine),
    Entry(Entry),
}

/// The header carries `"type": "session"` on disk so a reader can tell the
/// lines apart without positional rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HeaderLine {
    #[serde(rename = "type")]
    kind: HeaderTag,
    #[serde(flatten)]
    header: SessionHeader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HeaderTag {
    Session,
}

/// The header and every entry of the log at `path`, in file order. Malformed
/// lines are skipped with a warning so one bad write does not lose a session.
fn read_file(path: &Path) -> std::io::Result<(SessionHeader, Vec<Entry>)> {
    let reader = BufReader::new(File::open(path)?);
    let mut header = None;
    let mut entries = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Line>(&line) {
            Ok(Line::Header(h)) if header.is_none() => header = Some(h.header),
            Ok(Line::Header(_)) => {
                log::warn!("{}: duplicate header at line {}", path.display(), index + 1)
            }
            Ok(Line::Entry(entry)) => entries.push(entry),
            Err(error) => log::warn!(
                "{}: skipping malformed line {}: {error}",
                path.display(),
                index + 1
            ),
        }
    }
    let header = header.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} has no session header", path.display()),
        )
    })?;
    Ok((header, entries))
}

/// The failed request `branch` last left held, when nothing took it back:
/// see [`EntryKind::Retry`].
fn last_retry<'a>(branch: impl IntoIterator<Item = &'a Entry>) -> Option<Failure> {
    branch
        .into_iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Retry { failure } => Some(*failure),
            _ => None,
        })
        .last()
        .flatten()
}

/// The entries from the root to `leaf`, oldest first; `path` names the log
/// in the warning about a dangling parent.
fn branch_of<'a>(entries: &'a [Entry], leaf: Option<&str>, path: &Path) -> Vec<&'a Entry> {
    let by_id: HashMap<&str, &Entry> = entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut branch = Vec::new();
    let mut current = leaf;
    while let Some(id) = current {
        let Some(entry) = by_id.get(id) else {
            log::warn!("{}: dangling parent id {id}", path.display());
            break;
        };
        branch.push(*entry);
        current = entry.parent_id.as_deref();
    }
    branch.reverse();
    branch
}

#[derive(Debug)]
pub struct Session {
    path: PathBuf,
    header: SessionHeader,
    entries: Vec<Entry>,
    leaf: Option<String>,
    file: File,
    /// The canonical path this session holds an exclusive claim on, when it
    /// was opened with [`Session::open_exclusive`] / [`Session::create_exclusive`].
    /// Released on drop so a session is never live in two panels at once.
    lock: Option<PathBuf>,
    /// The claim other termide processes see, taken with `lock`.
    lease: Option<Lease>,
    /// Opened with [`Session::open_read_only`]: another termide holds the
    /// session, so nothing is written to its log from here.
    read_only: bool,
}

/// A claim on a session log that other termide processes see: an OS lock on
/// `<log>.lock`, the file carrying the holder's process id. The OS drops the
/// lock when the process ends, however it ends, so a crash or a kill leaves
/// no stale claim. The in-process registry stays what tells this process's
/// own panels apart, since the OS lets one process take its lock twice.
#[derive(Debug)]
struct Lease {
    file: File,
    path: PathBuf,
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Removed while still locked, so no one takes the old file's lock
        // between the unlock and the removal; a file Windows will not remove
        // while open stays, unlocked, for the next holder to take.
        let _ = std::fs::remove_file(&self.path);
        let _ = self.file.unlock();
    }
}

/// How a try for a session's lease went.
enum Leased {
    Taken(Lease),
    /// Another process holds it; its id when the lock file names it.
    Held(Option<u32>),
    /// No lock file could be made or locked (a read-only directory, a file
    /// system without locks): the session opens unleased, as it did before
    /// leases.
    Unavailable,
}

/// The lock file beside the log at `log`.
fn lease_path(log: &Path) -> PathBuf {
    let mut name = log.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

fn take_lease(log: &Path) -> Leased {
    let path = lease_path(log);
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) => {
            log::warn!("cannot make {}: {error}", path.display());
            return Leased::Unavailable;
        }
    };
    match file.try_lock() {
        Ok(()) => {
            // The holder's id, for another termide to name it.
            let written = file
                .set_len(0)
                .and_then(|()| write!(file, "{}", std::process::id()))
                .and_then(|()| file.flush());
            if let Err(error) = written {
                log::warn!("cannot write {}: {error}", path.display());
            }
            Leased::Taken(Lease { file, path })
        }
        Err(std::fs::TryLockError::WouldBlock) => Leased::Held(lease_holder_id(&path)),
        Err(std::fs::TryLockError::Error(error)) => {
            log::warn!("cannot lock {}: {error}", path.display());
            Leased::Unavailable
        }
    }
}

/// The process id a lock file names; `None` when it cannot be read, as a
/// file locked on Windows cannot.
fn lease_holder_id(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Paths currently open exclusively, so the same session log cannot back two
/// agent panels (which would interleave writes and corrupt it). Process-wide,
/// which covers every panel of one termide instance.
fn open_registry() -> &'static Mutex<HashSet<PathBuf>> {
    static OPEN: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    OPEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The registry key for `path`: canonical when it exists, else the path as
/// given, so different spellings of one file still collide.
fn lock_key(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Bumped whenever a claim is taken or released, so a session list that
/// leaves out the open sessions can tell cheaply when to look again.
static OPEN_GENERATION: AtomicU64 = AtomicU64::new(0);

fn claim(path: &Path) -> Option<PathBuf> {
    let key = lock_key(path);
    let mut open = open_registry().lock().unwrap_or_else(|e| e.into_inner());
    let claimed = open.insert(key.clone());
    if claimed {
        OPEN_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    claimed.then_some(key)
}

fn release(key: &Path) {
    let released = open_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(key);
    if released {
        OPEN_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The lease goes first: once the registry lets a panel of this
        // process claim the session again, its lock must be free to take.
        self.lease.take();
        if let Some(key) = self.lock.take() {
            release(&key);
        }
    }
}

impl Session {
    /// Create a new session file in `dir` (created if missing).
    pub fn create(dir: &Path, cwd: &Path) -> std::io::Result<Self> {
        Self::create_with(dir, cwd, None)
    }

    /// Create the session of a task delegated by `parent`, in `dir`, held
    /// exclusively as a panel's is: no panel opens it while the task runs.
    pub fn create_delegate(dir: &Path, cwd: &Path, parent: SessionParent) -> std::io::Result<Self> {
        let mut session = Self::create_with(dir, cwd, Some(parent))?;
        session.claim_new();
        Ok(session)
    }

    fn create_with(dir: &Path, cwd: &Path, parent: Option<SessionParent>) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let created = now_millis();
        let id = new_id();
        let header = SessionHeader {
            version: SESSION_FORMAT_VERSION,
            id: id.clone(),
            created,
            cwd: cwd.to_path_buf(),
            name: None,
            parent,
        };
        let path = dir.join(format!("{}_{id}.jsonl", file_timestamp(created)));
        let mut file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        write_line(
            &mut file,
            &Line::Header(HeaderLine {
                kind: HeaderTag::Session,
                header: header.clone(),
            }),
        )?;
        Ok(Self {
            path,
            header,
            entries: Vec::new(),
            leaf: None,
            file,
            lock: None,
            lease: None,
            read_only: false,
        })
    }

    /// Load an existing session; the leaf is the last entry. Malformed lines
    /// are skipped with a warning so one bad write does not lose a session.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let (header, entries) = read_file(path)?;
        let leaf = entries.last().map(|e| e.id.clone());
        let file = OpenOptions::new().append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            header,
            entries,
            leaf,
            file,
            lock: None,
            lease: None,
            read_only: false,
        })
    }

    /// Like [`Session::create`], but claims the session exclusively so it
    /// cannot also be opened in another panel; the claim is released on drop.
    pub fn create_exclusive(dir: &Path, cwd: &Path) -> std::io::Result<Self> {
        let mut session = Self::create(dir, cwd)?;
        session.claim_new();
        Ok(session)
    }

    /// The claims of a session this call just made: no one else can hold
    /// its new file, so the lease is taken unless locks are unavailable.
    fn claim_new(&mut self) {
        self.lock = claim(&self.path);
        if self.lock.is_some() {
            if let Leased::Taken(lease) = take_lease(&self.path) {
                self.lease = Some(lease);
            }
        }
    }

    /// Load the session at `path` to be read and never written: the session
    /// another termide holds, shown here until it lets it go. Every append
    /// fails, and a discard leaves the file alone.
    pub fn open_read_only(path: &Path) -> std::io::Result<Self> {
        let (header, entries) = read_file(path)?;
        let leaf = entries.last().map(|e| e.id.clone());
        let file = OpenOptions::new().read(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            header,
            entries,
            leaf,
            file,
            lock: None,
            lease: None,
            read_only: true,
        })
    }

    /// Whether this session was opened with [`Session::open_read_only`].
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether the session at `path` is open exclusively, in some panel of
    /// this process or in another termide.
    #[must_use]
    pub fn is_open(path: &Path) -> bool {
        open_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&lock_key(path))
            || Self::held_elsewhere(path).is_some()
    }

    /// Whether another termide holds the session at `path`: `Some` with its
    /// process id when the lock file names it, `None` when no other process
    /// does — this one's own claims are not "elsewhere".
    #[must_use]
    pub fn held_elsewhere(path: &Path) -> Option<Option<u32>> {
        let in_this_process = open_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&lock_key(path));
        if in_this_process {
            return None;
        }
        let lock_path = lease_path(path);
        // No lock file, no holder: it is made with the claim.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .ok()?;
        match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
                None
            }
            Err(std::fs::TryLockError::WouldBlock) => Some(lease_holder_id(&lock_path)),
            Err(std::fs::TryLockError::Error(_)) => None,
        }
    }

    /// A counter that changes whenever a session is opened or released
    /// exclusively: while it stays the same, so does [`Session::is_open`].
    #[must_use]
    pub fn open_generation() -> u64 {
        OPEN_GENERATION.load(Ordering::Relaxed)
    }

    /// Copy this session's log to a new file in the same directory and return
    /// its path: a fork. The copy carries the conversation — the entries on
    /// the current branch, in their original ids, so a compaction still
    /// summarises what it summarised — under a new header id and the `cwd` of
    /// the panel asking for it, which is where its work goes on. The original
    /// keeps its name and stays open: two panels then work the same
    /// conversation apart, each writing its own log.
    ///
    /// `name` is the display name the copy carries, which the caller gives it
    /// suffixed so the two are told apart in a picker; `None` leaves the copy
    /// unnamed like its source. A name is another entry on the branch, so it
    /// wins over the ones carried across — and an unnamed copy gets no entry
    /// at all, which keeps a fork of an untouched session empty and therefore
    /// discardable. The copy is not claimed, so the caller opens it as it
    /// would any other session.
    pub fn fork(&self, cwd: &Path, name: Option<&str>) -> std::io::Result<PathBuf> {
        let dir = self.path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} has no parent directory", self.path.display()),
            )
        })?;
        let created = now_millis();
        let id = new_id();
        let path = dir.join(format!("{}_{id}.jsonl", file_timestamp(created)));
        let mut header = self.header.clone();
        header.id = id;
        header.created = created;
        header.cwd = cwd.to_path_buf();
        header.name = name.map(str::to_string);
        // `create_new` first: the file is removed on a failed copy only once
        // this call has made it, so a name already taken is never touched.
        let mut file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        let written = (|| -> std::io::Result<()> {
            write_line(
                &mut file,
                &Line::Header(HeaderLine {
                    kind: HeaderTag::Session,
                    header: header.clone(),
                }),
            )?;
            for entry in self.branch() {
                write_line(&mut file, &Line::Entry(entry.clone()))?;
            }
            if let Some(name) = name {
                // The copy's own name, hanging off the branch's end: the last
                // one on the branch is what reads back.
                write_line(
                    &mut file,
                    &Line::Entry(Entry {
                        id: new_id(),
                        parent_id: self.leaf.clone(),
                        timestamp: created,
                        kind: EntryKind::SessionName {
                            name: name.to_string(),
                        },
                    }),
                )?;
            }
            Ok(())
        })();
        if let Err(error) = written {
            // A half-written copy is no session anyone could open.
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        Ok(path)
    }

    /// Move this session's log to `dir` (created when missing) as a session
    /// working in `cwd`, and return its new path: the session directory of
    /// another working directory, for a session that goes on there. The log
    /// keeps its id, its file name and every entry; only the header's `cwd`
    /// changes; a failed move leaves the session where it was. The moved log
    /// is not claimed, so the caller opens it as it would any other session.
    pub fn move_into(&self, dir: &Path, cwd: &Path) -> std::io::Result<PathBuf> {
        let file_name = self.path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} has no file name", self.path.display()),
            )
        })?;
        std::fs::create_dir_all(dir)?;
        let path = dir.join(file_name);
        let mut header = self.header.clone();
        header.cwd = cwd.to_path_buf();
        let mut file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)?;
        let written = (|| -> std::io::Result<()> {
            write_line(
                &mut file,
                &Line::Header(HeaderLine {
                    kind: HeaderTag::Session,
                    header,
                }),
            )?;
            for entry in &self.entries {
                write_line(&mut file, &Line::Entry(entry.clone()))?;
            }
            Ok(())
        })();
        // The old file goes once the new one is whole; when it cannot go (a
        // file held open on Windows) the copy does instead, so the session is
        // never in two places.
        if let Err(error) = written.and_then(|()| std::fs::remove_file(&self.path)) {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        Ok(path)
    }

    /// Like [`Session::open`], but fails if the session is already open in
    /// another panel, so its log is never written from two places at once.
    pub fn open_exclusive(path: &Path) -> std::io::Result<Self> {
        let Some(key) = claim(path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "this session is already open in another panel",
            ));
        };
        // Another termide's claim: its panel may be running the session
        // right now, so this one must not write to it nor carry it on.
        let lease = match take_lease(path) {
            Leased::Taken(lease) => Some(lease),
            Leased::Held(pid) => {
                release(&key);
                let holder = pid.map_or_else(String::new, |pid| format!(" (process {pid})"));
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("this session is open in another termide{holder}"),
                ));
            }
            Leased::Unavailable => None,
        };
        match Self::open(path) {
            Ok(mut session) => {
                session.lock = Some(key);
                session.lease = lease;
                Ok(session)
            }
            Err(error) => {
                release(&key);
                Err(error)
            }
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.header.id
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn header(&self) -> &SessionHeader {
        &self.header
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether the session holds no conversation: no messages and no
    /// user-given name, only setup entries (the model and agent it was
    /// created with). Such a fresh, untouched session is safe to discard so
    /// it does not clutter the session list or the disk.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.name().is_none()
            && !self
                .entries
                .iter()
                .any(|e| matches!(e.kind, EntryKind::Message { .. }))
    }

    /// Delete the session's file from disk, consuming the session (its lock is
    /// released as it drops). A missing file is not an error.
    pub fn discard(self) -> std::io::Result<()> {
        // Another termide's session is not this one's to delete.
        if self.read_only {
            return Ok(());
        }
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf.as_deref()
    }

    /// Where undoing a request about to start leads back to: the leaf, or
    /// what came before a context cleared just ahead of the request, so the
    /// undo brings back what the clearing left out.
    #[must_use]
    pub fn undo_point(&self) -> Option<&str> {
        let leaf = self.leaf.as_deref()?;
        match self.entries.iter().rev().find(|e| e.id == leaf) {
            Some(entry) if matches!(entry.kind, EntryKind::Pruned) => entry.parent_id.as_deref(),
            _ => Some(leaf),
        }
    }

    /// Move the leaf. `None` restarts from the root; an unknown id is an
    /// error and leaves the leaf unchanged.
    pub fn set_leaf(&mut self, id: Option<&str>) -> Result<(), String> {
        if let Some(id) = id {
            if !self.entries.iter().any(|e| e.id == id) {
                return Err(format!("no entry with id {id}"));
            }
        }
        self.leaf = id.map(str::to_string);
        Ok(())
    }

    pub fn append_message(&mut self, message: &Message) -> std::io::Result<String> {
        self.append_timed_message(message, None)
    }

    /// [`Session::append_message`] with how long the message took, so a
    /// reopened session can show it.
    pub fn append_timed_message(
        &mut self,
        message: &Message,
        timing: Option<Timing>,
    ) -> std::io::Result<String> {
        self.append(EntryKind::Message {
            message: message.clone(),
            timing,
        })
    }

    pub fn append_model_change(
        &mut self,
        provider: &str,
        model: &str,
        context_window: Option<u64>,
    ) -> std::io::Result<String> {
        self.append(EntryKind::ModelChange {
            provider: provider.to_string(),
            model: model.to_string(),
            context_window,
        })
    }

    fn append(&mut self, kind: EntryKind) -> std::io::Result<String> {
        if self.read_only {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "this session is open in another termide; it is only read here",
            ));
        }
        let entry = Entry {
            id: new_id(),
            parent_id: self.leaf.clone(),
            timestamp: now_millis(),
            kind,
        };
        write_line(&mut self.file, &Line::Entry(entry.clone()))?;
        self.leaf = Some(entry.id.clone());
        let id = entry.id.clone();
        self.entries.push(entry);
        Ok(id)
    }

    /// Entries on the path from the root to the leaf, oldest first.
    #[must_use]
    pub fn branch(&self) -> Vec<&Entry> {
        branch_of(&self.entries, self.leaf.as_deref(), &self.path)
    }

    /// The header and the live branch of the log at `path` (root to the last
    /// entry, oldest first), read without opening the file for writing or
    /// claiming it, so a log another panel is appending to can be searched.
    /// Entries a rewind left on a dead branch are not in it.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or has no header.
    pub fn read_branch(path: &Path) -> std::io::Result<(SessionHeader, Vec<Entry>)> {
        let (header, entries) = read_file(path)?;
        let leaf = entries.last().map(|e| e.id.as_str());
        let branch = branch_of(&entries, leaf, path)
            .into_iter()
            .cloned()
            .collect();
        Ok((header, branch))
    }

    /// The transcript the model should see for the current branch, with
    /// summaries worded by the default compaction prompts.
    #[must_use]
    pub fn context_messages(&self) -> Vec<Message> {
        self.context_messages_with(&CompactionPrompts::default())
    }

    /// [`Session::context_messages`] with the compaction prompts in use, so
    /// a summary reads the way `ai/system/compacted.md` words it.
    #[must_use]
    pub fn context_messages_with(&self, prompts: &CompactionPrompts) -> Vec<Message> {
        let mut messages: Vec<Message> = Vec::new();
        for entry in self.branch() {
            match &entry.kind {
                EntryKind::Message { message, .. } => messages.push(message.clone()),
                EntryKind::ModelChange { .. }
                | EntryKind::SessionName { .. }
                | EntryKind::AgentChange { .. }
                | EntryKind::Rewind
                | EntryKind::ReasoningChange { .. }
                | EntryKind::ThinkingChange { .. }
                | EntryKind::Toolset { .. }
                | EntryKind::ConnectionChange { .. }
                | EntryKind::ModeChange { .. }
                | EntryKind::ExternalSession { .. }
                | EntryKind::AgentOption { .. }
                | EntryKind::RunStart
                | EntryKind::RunEnd { .. }
                | EntryKind::Retry { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::CompactionFailed { .. } => {}
                EntryKind::Compaction {
                    summary, keep_last, ..
                } => {
                    let start = messages.len().saturating_sub(*keep_last);
                    let tail = messages.split_off(start);
                    messages = vec![prompts.summary_message(summary)];
                    messages.extend(tail);
                }
                EntryKind::Pruned => messages = prune_to_decisions(&messages),
                EntryKind::Fitted {
                    budget, keep_tail, ..
                } => messages = fit(&messages, *budget, *keep_tail).messages,
            }
        }
        messages
    }

    /// [`Session::context_messages_with`] with each message's wall-clock
    /// timestamp (millis) and recorded timing, so a reopened conversation can
    /// show when each block was written and how long it took. A compaction
    /// summary carries the compaction entry's time and no timing.
    #[must_use]
    pub fn context_messages_with_times(&self, prompts: &CompactionPrompts) -> Vec<LoggedMessage> {
        let mut messages: Vec<LoggedMessage> = Vec::new();
        for entry in self.branch() {
            match &entry.kind {
                EntryKind::Message { message, timing } => messages.push(LoggedMessage {
                    message: message.clone(),
                    timestamp: entry.timestamp,
                    timing: *timing,
                }),
                EntryKind::ModelChange { .. }
                | EntryKind::SessionName { .. }
                | EntryKind::AgentChange { .. }
                | EntryKind::Rewind
                | EntryKind::ReasoningChange { .. }
                | EntryKind::ThinkingChange { .. }
                | EntryKind::Toolset { .. }
                | EntryKind::ConnectionChange { .. }
                | EntryKind::ModeChange { .. }
                | EntryKind::ExternalSession { .. }
                | EntryKind::AgentOption { .. }
                | EntryKind::RunStart
                | EntryKind::RunEnd { .. }
                | EntryKind::Retry { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::CompactionFailed { .. } => {}
                EntryKind::Compaction {
                    summary, keep_last, ..
                } => {
                    let start = messages.len().saturating_sub(*keep_last);
                    let tail = messages.split_off(start);
                    messages = vec![LoggedMessage {
                        message: prompts.summary_message(summary),
                        timestamp: entry.timestamp,
                        timing: None,
                    }];
                    messages.extend(tail);
                }
                EntryKind::Fitted {
                    budget, keep_tail, ..
                } => {
                    let plain: Vec<Message> = messages
                        .iter()
                        .map(|logged| logged.message.clone())
                        .collect();
                    let fitted = fit(&plain, *budget, *keep_tail);
                    // The note on what was left out takes the time of the
                    // first message it stands for.
                    let first = messages.first().map_or(entry.timestamp, |l| l.timestamp);
                    messages = fitted
                        .messages
                        .into_iter()
                        .zip(fitted.origin)
                        .map(|(message, origin)| match origin {
                            Some(index) => LoggedMessage {
                                message,
                                ..messages[index].clone()
                            },
                            None => LoggedMessage {
                                message,
                                timestamp: first,
                                timing: None,
                            },
                        })
                        .collect();
                }
                EntryKind::Pruned => {
                    messages = prune_by(&messages, |logged| &logged.message)
                        .into_iter()
                        .map(|(logged, message)| LoggedMessage {
                            message,
                            ..logged.clone()
                        })
                        .collect();
                }
            }
        }
        messages
    }

    /// The conversation's name: the last one set on the current branch, or
    /// the one the file was created with; an empty one set last clears it.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::SessionName { name } => Some(name.as_str()),
                _ => None,
            })
            .or(self.header.name.as_deref())
            .filter(|name| !name.is_empty())
    }

    /// Name (or rename) the conversation. An empty name clears it.
    pub fn set_name(&mut self, name: &str) -> std::io::Result<String> {
        let name = name.trim().to_string();
        self.header.name = (!name.is_empty()).then(|| name.clone());
        self.append(EntryKind::SessionName { name })
    }

    pub fn append_compaction(
        &mut self,
        summary: &str,
        tokens_before: u64,
        keep_last: usize,
    ) -> std::io::Result<String> {
        self.append_compaction_with(summary, tokens_before, keep_last, None, None)
    }

    /// [`Session::append_compaction`] with the context size after it and
    /// how the summary call went.
    pub fn append_compaction_with(
        &mut self,
        summary: &str,
        tokens_before: u64,
        keep_last: usize,
        tokens_after: Option<u64>,
        call: Option<CompactionCall>,
    ) -> std::io::Result<String> {
        self.append(EntryKind::Compaction {
            summary: summary.to_string(),
            tokens_before,
            keep_last,
            tokens_after,
            call,
        })
    }

    /// Record that the context was cut down by kinds of content to
    /// `budget` characters, keeping `keep_tail` whole, see
    /// [`EntryKind::Fitted`].
    pub fn append_fitted(
        &mut self,
        budget: usize,
        keep_tail: usize,
        steps: Vec<(FitStep, usize)>,
        tokens_before: u64,
    ) -> std::io::Result<String> {
        self.append(EntryKind::Fitted {
            budget,
            keep_tail,
            steps,
            tokens_before,
        })
    }

    /// Record a summary call that was made and refused, with the text it
    /// was refused for; the context is unchanged by it.
    pub fn append_compaction_failed(
        &mut self,
        error: &str,
        tokens_before: u64,
        call: CompactionCall,
        reply: &str,
    ) -> std::io::Result<String> {
        self.append(EntryKind::CompactionFailed {
            error: error.to_string(),
            tokens_before,
            call,
            reply: reply.to_string(),
        })
    }

    /// Record that a plan is carried out from a clean context.
    pub fn append_pruned(&mut self) -> std::io::Result<String> {
        self.append(EntryKind::Pruned)
    }

    /// Model recorded last on the current branch.
    #[must_use]
    pub fn current_model(&self) -> Option<SessionModel> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::ModelChange {
                    provider,
                    model,
                    context_window,
                } => Some(SessionModel {
                    provider: provider.clone(),
                    id: model.clone(),
                    context_window: *context_window,
                }),
                EntryKind::Message { .. }
                | EntryKind::Compaction { .. }
                | EntryKind::SessionName { .. }
                | EntryKind::AgentChange { .. }
                | EntryKind::Rewind
                | EntryKind::Pruned
                | EntryKind::ReasoningChange { .. }
                | EntryKind::ThinkingChange { .. }
                | EntryKind::Toolset { .. }
                | EntryKind::ConnectionChange { .. }
                | EntryKind::ModeChange { .. }
                | EntryKind::ExternalSession { .. }
                | EntryKind::AgentOption { .. }
                | EntryKind::RunStart
                | EntryKind::RunEnd { .. }
                | EntryKind::Retry { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::CompactionFailed { .. }
                | EntryKind::Fitted { .. } => None,
            })
    }

    /// Undo back to `leaf`: the next entries hang off it, and a `rewind`
    /// entry marks the spot so a reopened session lands there too.
    pub fn rewind_to(&mut self, leaf: Option<&str>) -> Result<String, String> {
        self.set_leaf(leaf)?;
        self.append(EntryKind::Rewind)
            .map_err(|error| format!("cannot record the rewind: {error}"))
    }

    /// Record the agent the branch runs as from here.
    pub fn append_agent_change(&mut self, agent: &str) -> std::io::Result<String> {
        self.append(EntryKind::AgentChange {
            agent: agent.to_string(),
        })
    }

    /// Agent recorded last on the current branch.
    #[must_use]
    pub fn current_agent(&self) -> Option<String> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::AgentChange { agent } => Some(agent.clone()),
                _ => None,
            })
    }

    pub fn append_thinking_change(&mut self, level: ThinkingLevel) -> std::io::Result<String> {
        self.append(EntryKind::ThinkingChange { level })
    }

    pub fn append_connection_change(&mut self, connection: &str) -> std::io::Result<String> {
        self.append(EntryKind::ConnectionChange {
            connection: connection.to_string(),
        })
    }

    /// The connection recorded last on the current branch, if any.
    #[must_use]
    pub fn current_connection(&self) -> Option<String> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::ConnectionChange { connection } => Some(connection.clone()),
                _ => None,
            })
    }

    /// Record the permission mode the branch runs in from here.
    pub fn append_mode_change(&mut self, mode: Mode) -> std::io::Result<String> {
        self.append(EntryKind::ModeChange { mode })
    }

    /// The permission mode recorded last on the current branch, if any.
    #[must_use]
    pub fn current_mode(&self) -> Option<Mode> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::ModeChange { mode } => Some(*mode),
                _ => None,
            })
    }

    /// Record that the external agent `agent` opened its own session
    /// `session_id` for this branch.
    pub fn append_external_session(
        &mut self,
        agent: &str,
        session_id: &str,
    ) -> std::io::Result<String> {
        self.append(EntryKind::ExternalSession {
            agent: agent.to_string(),
            session_id: session_id.to_string(),
            log: self.header.id.clone(),
            cwd: self.header.cwd.clone(),
        })
    }

    /// The external agent's session that holds this branch's conversation as
    /// the branch has it, for the agent to resume: the last one recorded,
    /// unless the branch has since gone where that session cannot follow —
    /// a request undone, the context compacted or cleared, another agent or
    /// connection taken. A fork or a move into another directory leaves it
    /// behind too: the session belongs to the log and the place it was
    /// opened for, and two logs resuming one session would tangle it. The
    /// external agent checks the session is still its own when it resumes.
    #[must_use]
    pub fn external_session(&self) -> Option<ExternalSessionRef> {
        for entry in self.branch().into_iter().rev() {
            match &entry.kind {
                EntryKind::ExternalSession {
                    agent,
                    session_id,
                    log,
                    cwd,
                } => {
                    return (*log == self.header.id && *cwd == self.header.cwd).then(|| {
                        ExternalSessionRef {
                            agent: agent.clone(),
                            session_id: session_id.clone(),
                        }
                    });
                }
                EntryKind::Rewind
                | EntryKind::Compaction { .. }
                | EntryKind::Fitted { .. }
                | EntryKind::Pruned
                | EntryKind::AgentChange { .. }
                | EntryKind::ConnectionChange { .. } => return None,
                // The built-in loop answered in the meantime (the agent could
                // not start): the agent's session never saw that exchange.
                EntryKind::Message {
                    message: Message::Assistant(assistant),
                    ..
                } if assistant.provider != ACP_PROVIDER => return None,
                EntryKind::Message { .. }
                | EntryKind::ModelChange { .. }
                | EntryKind::SessionName { .. }
                | EntryKind::ReasoningChange { .. }
                | EntryKind::ThinkingChange { .. }
                | EntryKind::Toolset { .. }
                | EntryKind::ModeChange { .. }
                | EntryKind::AgentOption { .. }
                | EntryKind::RunStart
                | EntryKind::RunEnd { .. }
                | EntryKind::Retry { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::CompactionFailed { .. } => {}
            }
        }
        None
    }

    /// Record that the external agent's setting `option` was set to `value`.
    pub fn append_agent_option(&mut self, option: &str, value: &str) -> std::io::Result<String> {
        self.append(EntryKind::AgentOption {
            option: option.to_string(),
            value: value.to_string(),
        })
    }

    /// The external agent's settings picked on the current branch, the last
    /// value of each, oldest pick first. Those picked before the agent or
    /// the connection last changed belong to another agent and are left out.
    #[must_use]
    pub fn agent_options(&self) -> Vec<(String, String)> {
        let mut picked: Vec<(String, String)> = Vec::new();
        for entry in self.branch() {
            match &entry.kind {
                EntryKind::AgentChange { .. } | EntryKind::ConnectionChange { .. } => {
                    picked.clear();
                }
                EntryKind::AgentOption { option, value } => {
                    picked.retain(|(id, _)| id != option);
                    picked.push((option.clone(), value.clone()));
                }
                _ => {}
            }
        }
        picked
    }

    /// Record that a run started.
    pub fn append_run_start(&mut self) -> std::io::Result<String> {
        self.append(EntryKind::RunStart)
    }

    /// Record that the run ended, `paused` when at a `/pause`.
    pub fn append_run_end(&mut self, paused: bool) -> std::io::Result<String> {
        self.append(EntryKind::RunEnd { paused })
    }

    /// Record the unattended work going from here (empty: none).
    pub fn append_autorun(&mut self, autorun: &Autorun) -> std::io::Result<String> {
        self.append(EntryKind::Autorun {
            autorun: autorun.clone(),
        })
    }

    /// Record a failed request the panel still holds (`None`: it let it go),
    /// see [`Self::pending_retry`].
    pub fn append_retry(&mut self, failure: Option<Failure>) -> std::io::Result<String> {
        self.append(EntryKind::Retry { failure })
    }

    /// The failed request the branch last left held by a card or a wait, when
    /// nothing has taken it back: see [`EntryKind::Retry`].
    #[must_use]
    pub fn pending_retry(&self) -> Option<Failure> {
        last_retry(self.branch())
    }

    /// [`Self::pending_retry`] of the log at `path`, read without opening it
    /// the way [`Self::read_branch`] reads: so the log of a panel not loaded
    /// yet can tell whether a request in it waits to be tried again.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or has no header.
    pub fn pending_retry_at(path: &Path) -> std::io::Result<Option<Failure>> {
        let (_, branch) = Self::read_branch(path)?;
        Ok(last_retry(&branch))
    }

    /// How the current branch's last run ended, see [`Self::unfinished_run`].
    #[must_use]
    pub fn last_run(&self) -> LastRun {
        self.unfinished_run()
            .map_or(LastRun::Ended, |mark| mark.state)
    }

    /// The branch's last run, when it did not end: paused, or cut off — it
    /// started and never ended, and its last message leaves work to do (a
    /// request or results the model has not answered, calls not run, a reply
    /// that stopped to wait on calls). One
    /// whose last reply finished, or failed, is as good as ended: only its
    /// end was not written. A log written before runs were recorded holds
    /// none.
    #[must_use]
    pub fn unfinished_run(&self) -> Option<RunMark> {
        let branch = self.branch();
        let at = branch
            .iter()
            .rposition(|e| matches!(e.kind, EntryKind::RunStart | EntryKind::RunEnd { .. }))?;
        // The start is written after the run's first message.
        let started_of = |start: usize| branch[start.saturating_sub(1)].timestamp;
        match branch[at].kind {
            EntryKind::RunEnd { paused: true } => {
                let start = branch[..at]
                    .iter()
                    .rposition(|e| matches!(e.kind, EntryKind::RunStart))
                    .unwrap_or(at);
                Some(RunMark {
                    state: LastRun::Paused,
                    started: started_of(start),
                    stopped: branch[at].timestamp,
                })
            }
            EntryKind::RunStart => {
                let last = branch.iter().rev().find_map(|e| match &e.kind {
                    EntryKind::Message { message, .. } => Some((message, e.timestamp)),
                    _ => None,
                })?;
                let open = match last.0 {
                    // A reply that waits on calls: its own, or — from an
                    // external agent, which logs a call only with its result —
                    // the text it closed before a call that was still running.
                    Message::Assistant(reply) => {
                        (reply.tool_calls().next().is_some()
                            || matches!(reply.stop_reason, StopReason::ToolUse))
                            && !matches!(reply.stop_reason, StopReason::Error | StopReason::Aborted)
                    }
                    Message::User(_) | Message::ToolResult(_) => true,
                };
                open.then(|| RunMark {
                    state: LastRun::CutOff,
                    started: started_of(at),
                    stopped: last.1,
                })
            }
            _ => None,
        }
    }

    /// The unattended work recorded last on the current branch; empty when
    /// none was, or it stopped.
    #[must_use]
    pub fn autorun(&self) -> Autorun {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::Autorun { autorun } => Some(autorun.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn append_toolset(&mut self, disabled: &[String]) -> std::io::Result<String> {
        self.append(EntryKind::Toolset {
            disabled: disabled.to_vec(),
        })
    }

    /// What the session switched off, as recorded last on the current
    /// branch; `None` when it never changed the set.
    #[must_use]
    pub fn current_toolset(&self) -> Option<Vec<String>> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::Toolset { disabled } => Some(disabled.clone()),
                _ => None,
            })
    }

    /// Reasoning level recorded last on the current branch, if any. The old
    /// on/off switch reads as `high` or `off`.
    #[must_use]
    pub fn current_thinking(&self) -> Option<ThinkingLevel> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::ThinkingChange { level } => Some(*level),
                EntryKind::ReasoningChange { reasoning: true } => Some(ThinkingLevel::High),
                EntryKind::ReasoningChange { reasoning: false } => Some(ThinkingLevel::Off),
                _ => None,
            })
    }

    /// Summaries of every session in `dir`, newest first.
    pub fn list(dir: &Path) -> std::io::Result<Vec<SessionSummary>> {
        let mut summaries = Vec::new();
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return Ok(summaries);
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            match Session::open(&path) {
                Ok(session) => summaries.push(SessionSummary::from(&session)),
                Err(error) => log::warn!("skipping {}: {error}", path.display()),
            }
        }
        summaries.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.created.cmp(&a.created)));
        Ok(summaries)
    }
}

impl SessionSummary {
    /// One line for a session picker: the name if it has one, else the first
    /// prompt, else the session id.
    #[must_use]
    pub fn label(&self) -> String {
        self.name
            .clone()
            .or_else(|| {
                self.first_prompt
                    .as_ref()
                    .map(|prompt| prompt.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|prompt| !prompt.is_empty())
            })
            .unwrap_or_else(|| self.id.clone())
    }

    /// [`Self::label`] marked `⏸` when the session was left with work to
    /// carry on and `&` when it is a delegate's, the way the panel marks a
    /// delegated task, for the lists that offer sessions to open.
    #[must_use]
    pub fn display_label(&self) -> String {
        let mut label = self.label();
        if self.parent.is_some() {
            label = format!("& {label}");
        }
        if self.unfinished {
            label = format!("⏸ {label}");
        }
        label
    }
}

/// What a session picker shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub path: PathBuf,
    pub id: String,
    pub name: Option<String>,
    pub created: u64,
    /// Timestamp of the last entry, or `created` for an empty session.
    pub modified: u64,
    /// The first user prompt, as a title fallback.
    pub first_prompt: Option<String>,
    pub message_count: usize,
    /// The session was left with work to carry on: a run cut off or
    /// paused, a `/goal` or `/loop` going.
    pub unfinished: bool,
    /// Where a delegate's session came from.
    pub parent: Option<SessionParent>,
    /// The agent the session runs on, as its log records it last; `None`
    /// for a log written before agents were recorded.
    pub agent: Option<String>,
}

impl From<&Session> for SessionSummary {
    fn from(session: &Session) -> Self {
        let messages = session.entries.iter().filter_map(|e| match &e.kind {
            EntryKind::Message { message, .. } => Some(message),
            EntryKind::ModelChange { .. }
            | EntryKind::Compaction { .. }
            | EntryKind::SessionName { .. }
            | EntryKind::AgentChange { .. }
            | EntryKind::Rewind
            | EntryKind::Pruned
            | EntryKind::ReasoningChange { .. }
            | EntryKind::ThinkingChange { .. }
            | EntryKind::Toolset { .. }
            | EntryKind::ConnectionChange { .. }
            | EntryKind::ModeChange { .. }
            | EntryKind::ExternalSession { .. }
            | EntryKind::AgentOption { .. }
            | EntryKind::RunStart
            | EntryKind::RunEnd { .. }
            | EntryKind::Retry { .. }
            | EntryKind::Autorun { .. }
            | EntryKind::CompactionFailed { .. }
            | EntryKind::Fitted { .. } => None,
        });
        let first_prompt = messages.clone().find_map(|m| match m {
            Message::User(user) => Some(user.plain_text()),
            _ => None,
        });
        Self {
            path: session.path.clone(),
            id: session.header.id.clone(),
            name: session.name().map(str::to_string),
            created: session.header.created,
            modified: session
                .entries
                .last()
                .map_or(session.header.created, |e| e.timestamp),
            first_prompt,
            message_count: messages.count(),
            unfinished: session.unfinished_run().is_some()
                || !session.autorun().is_empty()
                || session.pending_retry().is_some(),
            parent: session.header.parent.clone(),
            agent: session.current_agent(),
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Eight hex characters unique enough for one machine: time, a process-wide
/// counter and the pid go through the standard hasher.
#[must_use]
pub fn new_id() -> String {
    let mut hasher = DefaultHasher::new();
    now_millis().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

fn write_line(file: &mut File, line: &Line) -> std::io::Result<()> {
    let mut text = serde_json::to_string(line)?;
    text.push('\n');
    file.write_all(text.as_bytes())?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_support::text_reply;
    use crate::failure::FailureKind;
    use crate::message::{AssistantMessage, UserMessage};

    #[test]
    fn is_empty_tracks_conversation_and_discard_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        // Setup entries alone (model, agent) leave the session empty.
        session
            .append_model_change("local", "qwen", Some(32_000))
            .unwrap();
        session.append_agent_change("default").unwrap();
        assert!(session.is_empty());
        // A user-given name marks intent to keep it.
        session.set_name("keep me").unwrap();
        assert!(!session.is_empty());
        // Clearing the name gives the default label back and the intent up.
        session.set_name(" ").unwrap();
        assert_eq!(session.name(), None);
        assert!(session.is_empty());
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.name(), None);
        assert_eq!(SessionSummary::from(&reopened).name, None);
        session.set_name("keep me").unwrap();
        assert!(!session.is_empty());

        // A fresh session with a message is not empty either.
        let mut chatted = Session::create(dir.path(), Path::new("/work")).unwrap();
        chatted
            .append_message(&Message::User(UserMessage::text("hi")))
            .unwrap();
        assert!(!chatted.is_empty());

        // Discard deletes the file (and a missing file is not an error).
        let path = chatted.path().to_path_buf();
        assert!(path.exists());
        chatted.discard().unwrap();
        assert!(!path.exists());
    }

    /// A reply of the external agent, as its backend logs it.
    fn agent_reply(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            provider: ACP_PROVIDER.into(),
            ..text_reply(text)
        })
    }

    #[test]
    fn runs_and_unattended_work_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        assert_eq!(session.last_run(), LastRun::Ended);
        let before = session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        session.append_run_start().unwrap();
        let autorun = Autorun {
            goal: Some(GoalRecord {
                goal: "ship".into(),
                iterations: 2,
            }),
            repeat: Some(LoopRecord {
                prompt: "check".into(),
                interval_ms: None,
                iterations: 5,
                due_ms: None,
            }),
        };
        session.append_autorun(&autorun).unwrap();
        let path = session.path().to_path_buf();
        drop(session);

        let mut session = Session::open(&path).unwrap();
        assert_eq!(session.last_run(), LastRun::CutOff);
        assert_eq!(session.autorun(), autorun);
        // None of it reaches the model.
        assert_eq!(session.context_messages().len(), 1);
        session.append_run_end(true).unwrap();
        assert_eq!(session.last_run(), LastRun::Paused);
        session.append_autorun(&Autorun::default()).unwrap();
        assert!(Session::open(&path).unwrap().autorun().is_empty());
        // Undone back to before the run, the branch holds none of it.
        session.rewind_to(Some(&before)).unwrap();
        assert_eq!(session.last_run(), LastRun::Ended);
    }

    #[test]
    fn a_run_whose_last_reply_finished_counts_as_ended() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        session.append_run_start().unwrap();
        assert_eq!(session.last_run(), LastRun::CutOff);
        // The reply came in whole; only the run's end was not written.
        session.append_message(&agent_reply("answer")).unwrap();
        assert_eq!(session.last_run(), LastRun::Ended);
        assert!(!SessionSummary::from(&session).unfinished);
    }

    /// The external agent closes its text before a call and logs the call
    /// only with its result: a run cut off while that call ran ends on the
    /// text, which waits on the call, so the run is still open.
    #[test]
    fn a_run_cut_off_in_an_external_agents_call_stays_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("deploy")))
            .unwrap();
        session.append_run_start().unwrap();
        session
            .append_message(&Message::Assistant(AssistantMessage {
                provider: ACP_PROVIDER.into(),
                stop_reason: StopReason::ToolUse,
                ..text_reply("Running the script.")
            }))
            .unwrap();
        assert_eq!(session.last_run(), LastRun::CutOff);
        assert!(SessionSummary::from(&session).unfinished);
    }

    /// A held request survives a reopen and is taken back when it is
    /// retried or given up; the branch's own end says nothing about it.
    #[test]
    fn a_held_retry_survives_reopen_and_is_taken_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("build it")))
            .unwrap();
        session.append_run_start().unwrap();
        session.append_run_end(false).unwrap();
        assert_eq!(session.pending_retry(), None);
        assert!(!SessionSummary::from(&session).unfinished);

        let failure = Failure {
            kind: FailureKind::QuotaExhausted,
            resets_at: Some(1_800),
        };
        session.append_retry(Some(failure)).unwrap();
        let path = session.path().to_path_buf();
        assert_eq!(session.pending_retry(), Some(failure));
        assert!(SessionSummary::from(&session).unfinished);
        drop(session);

        // Read without opening, as a project not loaded yet is asked.
        assert_eq!(Session::pending_retry_at(&path).unwrap(), Some(failure));
        let reopened = Session::open(&path).unwrap();
        assert_eq!(reopened.pending_retry(), Some(failure));
        // The run itself ended: the held request is a choice, not a run.
        assert_eq!(reopened.last_run(), LastRun::Ended);
        drop(reopened);

        let mut session = Session::open(&path).unwrap();
        session.append_retry(None).unwrap();
        assert_eq!(session.pending_retry(), None);
        assert_eq!(Session::pending_retry_at(&path).unwrap(), None);
        assert!(!SessionSummary::from(&session).unfinished);
    }

    #[test]
    fn an_external_session_is_resumed_while_the_branch_is_still_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session.append_agent_change("claude").unwrap();
        assert_eq!(session.external_session(), None);
        session.append_external_session("claude", "s1").unwrap();
        let start = session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        session.append_message(&agent_reply("answer")).unwrap();
        // What the session holds does not change with these.
        session.append_toolset(&["bash".into()]).unwrap();
        session.set_name("named").unwrap();
        let expected = Some(ExternalSessionRef {
            agent: "claude".into(),
            session_id: "s1".into(),
        });
        assert_eq!(session.external_session(), expected);
        // It survives a reopen.
        let path = session.path().to_path_buf();
        drop(session);
        let mut session = Session::open(&path).unwrap();
        assert_eq!(session.external_session(), expected);

        // An undone request is still in the agent's session.
        session.rewind_to(Some(&start)).unwrap();
        assert_eq!(session.external_session(), None);
        // A new one after it is resumed again.
        session.append_external_session("claude", "s2").unwrap();
        assert_eq!(
            session.external_session().map(|r| r.session_id),
            Some("s2".into())
        );
        // The built-in loop answered in between: the agent never saw it.
        session
            .append_message(&Message::Assistant(text_reply("built-in")))
            .unwrap();
        assert_eq!(session.external_session(), None);
    }

    #[test]
    fn an_external_agents_settings_are_kept_until_another_agent_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session.append_agent_change("claude").unwrap();
        session.append_external_session("claude", "s1").unwrap();
        session.append_agent_option("effort", "low").unwrap();
        session.append_agent_option("model", "opus").unwrap();
        session.append_agent_option("effort", "high").unwrap();
        let expected = vec![
            ("model".to_string(), "opus".to_string()),
            ("effort".to_string(), "high".to_string()),
        ];
        assert_eq!(session.agent_options(), expected);
        // They change nothing the agent's own session holds.
        assert!(session.external_session().is_some());
        let path = session.path().to_path_buf();
        drop(session);
        let mut session = Session::open(&path).unwrap();
        assert_eq!(session.agent_options(), expected);
        // Another agent's settings are its own.
        session.append_agent_change("codex").unwrap();
        assert!(session.agent_options().is_empty());
    }

    #[test]
    fn another_agent_a_compaction_a_fork_or_a_move_leave_the_external_session_behind() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = |dir: &Path| {
            let mut session = Session::create(dir, Path::new("/work")).unwrap();
            session.append_external_session("claude", "s1").unwrap();
            session
                .append_message(&Message::User(UserMessage::text("one")))
                .unwrap();
            assert!(session.external_session().is_some());
            session
        };
        let mut switched = fresh(dir.path());
        switched.append_agent_change("default").unwrap();
        assert_eq!(switched.external_session(), None);
        let mut reconnected = fresh(dir.path());
        reconnected.append_connection_change("cloud").unwrap();
        assert_eq!(reconnected.external_session(), None);
        let mut compacted = fresh(dir.path());
        compacted.append_compaction("gist", 100, 1).unwrap();
        assert_eq!(compacted.external_session(), None);
        let mut cleared = fresh(dir.path());
        cleared.append_pruned().unwrap();
        assert_eq!(cleared.external_session(), None);

        let original = fresh(dir.path());
        let fork = Session::open(&original.fork(Path::new("/work"), None).unwrap()).unwrap();
        assert_eq!(fork.external_session(), None);
        assert!(original.external_session().is_some());
        let elsewhere = tempfile::tempdir().unwrap();
        let moved = original
            .move_into(elsewhere.path(), Path::new("/elsewhere"))
            .unwrap();
        assert_eq!(Session::open(&moved).unwrap().external_session(), None);
    }

    #[test]
    fn create_append_reopen_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session
            .append_model_change("local", "qwen", Some(32_000))
            .unwrap();
        session.append_agent_change("review").unwrap();
        session
            .append_message(&Message::User(UserMessage::text("hi")))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply("hello")))
            .unwrap();

        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.id(), session.id());
        assert_eq!(reopened.header().cwd, PathBuf::from("/work"));
        assert_eq!(reopened.entries().len(), 4);
        assert_eq!(reopened.current_agent().as_deref(), Some("review"));
        assert_eq!(reopened.leaf_id(), session.leaf_id());
        assert_eq!(
            reopened.current_model(),
            Some(SessionModel {
                provider: "local".into(),
                id: "qwen".into(),
                context_window: Some(32_000),
            })
        );
        let messages = reopened.context_messages();
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], Message::User(u) if u.plain_text() == "hi"));

        let first_line = std::fs::read_to_string(session.path())
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        assert!(first_line.starts_with("{\"type\":\"session\""));
    }

    #[test]
    fn moving_the_leaf_starts_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
        let first = session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply("answer one")))
            .unwrap();
        session.set_leaf(Some(&first)).unwrap();
        session
            .append_message(&Message::Assistant(text_reply("answer two")))
            .unwrap();

        let branch = session.context_messages();
        assert_eq!(branch.len(), 2);
        assert!(matches!(&branch[1], Message::Assistant(a) if a.plain_text() == "answer two"));
        assert_eq!(session.entries().len(), 3, "nothing was rewritten");
        assert!(session.set_leaf(Some("nope")).is_err());
        session.set_leaf(None).unwrap();
        assert!(session.context_messages().is_empty());
    }

    #[test]
    fn a_pruned_entry_replays_as_the_live_pruning_and_a_rewind_past_it_undoes_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
        let read = crate::message::ToolCall {
            id: "r1".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
            extra_content: None,
        };
        let mut looking = text_reply("looking");
        looking
            .content
            .push(crate::message::AssistantContent::ToolCall(read.clone()));
        session
            .append_message(&Message::User(UserMessage::text("plan it")))
            .unwrap();
        session
            .append_message(&Message::Assistant(looking))
            .unwrap();
        session
            .append_message(&Message::ToolResult(
                crate::message::ToolResultMessage::text(&read, "contents"),
            ))
            .unwrap();
        let plan = session
            .append_message(&Message::Assistant(text_reply("the plan")))
            .unwrap();
        let whole = session.context_messages();
        session.append_pruned().unwrap();
        // One message, logged and expected alike: two would differ in their
        // timestamps whenever a millisecond passes between them.
        let next = Message::User(UserMessage::text("do it"));
        session.append_message(&next).unwrap();

        let mut expected = prune_to_decisions(&whole);
        expected.push(next);
        assert_eq!(expected.len(), 3);
        assert_eq!(session.context_messages(), expected);
        let timed: Vec<Message> = session
            .context_messages_with_times(&CompactionPrompts::default())
            .into_iter()
            .map(|logged| logged.message)
            .collect();
        assert_eq!(timed, expected);

        session.rewind_to(Some(&plan)).unwrap();
        assert_eq!(session.context_messages(), whole);

        // A request made right after a clearing undoes to before it.
        let cleared = session.append_pruned().unwrap();
        assert_eq!(session.leaf_id(), Some(cleared.as_str()));
        let point = session.undo_point().map(str::to_string);
        assert_ne!(point.as_deref(), Some(cleared.as_str()));
        session.rewind_to(point.as_deref()).unwrap();
        assert_eq!(session.context_messages(), whole);
    }

    #[test]
    fn read_branch_gives_the_live_path_and_leaves_dead_branches_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
        let first = session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply("undone")))
            .unwrap();
        session.rewind_to(Some(&first)).unwrap();
        session
            .append_message(&Message::Assistant(text_reply("kept")))
            .unwrap();
        let path = session.path().to_path_buf();

        // Read while the session is still open for appending.
        let (header, branch) = Session::read_branch(&path).unwrap();
        assert_eq!(&header.id, session.id());
        let texts: Vec<String> = branch
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::Message {
                    message: Message::User(user),
                    ..
                } => Some(user.plain_text()),
                EntryKind::Message {
                    message: Message::Assistant(reply),
                    ..
                } => Some(reply.plain_text()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["one", "kept"]);
        assert!(branch.iter().any(|e| matches!(e.kind, EntryKind::Rewind)));
    }

    #[test]
    fn malformed_lines_are_skipped_and_missing_header_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("ok")))
            .unwrap();
        let path = session.path().to_path_buf();
        drop(session);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{{ this is not json").unwrap();
        let reopened = Session::open(&path).unwrap();
        assert_eq!(reopened.entries().len(), 1);

        let headerless = dir.path().join("bad.jsonl");
        std::fs::write(&headerless, "{\"id\":\"x\",\"parent_id\":null,\"timestamp\":1,\"type\":\"model_change\",\"provider\":\"p\",\"model\":\"m\"}\n").unwrap();
        assert!(Session::open(&headerless).is_err());
    }

    #[test]
    fn a_compactions_details_are_optional_and_a_refused_call_changes_nothing() {
        // A line written before the details existed still reads.
        let old: EntryKind = serde_json::from_str(
            r#"{"type":"compaction","summary":"s","tokens_before":5,"keep_last":1}"#,
        )
        .unwrap();
        assert_eq!(
            old,
            EntryKind::Compaction {
                summary: "s".into(),
                tokens_before: 5,
                keep_last: 1,
                tokens_after: None,
                call: None,
            }
        );
        // Without details it is written as before.
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"type":"compaction","summary":"s","tokens_before":5,"keep_last":1}"#
        );

        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session.append_external_session("claude", "s1").unwrap();
        session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        session.append_message(&agent_reply("answer")).unwrap();
        let before = session.context_messages();
        let call = CompactionCall {
            reason: crate::compaction::CompactionReason::Threshold,
            stop_reason: StopReason::ToolUse,
            duration_ms: 198_000,
            usage: crate::message::Usage::default(),
            thinking_chars: 0,
            tool_calls: vec!["bash".into()],
            input_cut: Vec::new(),
        };
        session
            .append_compaction_failed("degenerate", 247_861, call.clone(), "Looking:")
            .unwrap();
        let path = session.path().to_path_buf();
        drop(session);
        let session = Session::open(&path).unwrap();
        assert_eq!(session.context_messages(), before);
        assert!(session.external_session().is_some());
        assert!(session.branch().iter().any(|e| e.kind
            == EntryKind::CompactionFailed {
                error: "degenerate".into(),
                tokens_before: 247_861,
                call: call.clone(),
                reply: "Looking:".into(),
            }));
    }

    #[test]
    fn a_fitted_entry_replays_the_cut_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session.append_external_session("claude", "s1").unwrap();
        for i in 0..10 {
            session
                .append_message(&Message::User(UserMessage::text(format!(
                    "request {i} {}",
                    "x".repeat(100)
                ))))
                .unwrap();
            session
                .append_message(&agent_reply(&format!("answer {i}")))
                .unwrap();
        }
        let whole = session.context_messages();
        let expected = crate::fit::fit(&whole, 600, 50);
        assert!(expected.changed());
        session
            .append_fitted(600, 50, expected.steps.clone(), 1_000)
            .unwrap();
        let path = session.path().to_path_buf();
        drop(session);
        let session = Session::open(&path).unwrap();
        assert_eq!(session.context_messages(), expected.messages);
        // The external agent's session holds what was there before.
        assert_eq!(session.external_session(), None);
        let timed = session.context_messages_with_times(&CompactionPrompts::default());
        let plain: Vec<Message> = timed.iter().map(|l| l.message.clone()).collect();
        assert_eq!(plain, expected.messages);
        // The note takes the time of the first message; the rest keep theirs.
        let all = Session::open(&path).unwrap();
        let first = all
            .branch()
            .into_iter()
            .find(|e| matches!(e.kind, EntryKind::Message { .. }))
            .unwrap()
            .timestamp;
        assert_eq!(timed[0].timestamp, first);
        assert!(timed.windows(2).all(|w| w[0].timestamp <= w[1].timestamp));
    }

    #[test]
    fn compaction_entry_rebuilds_the_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
        for text in ["one", "two", "three"] {
            session
                .append_message(&Message::User(UserMessage::text(text)))
                .unwrap();
            session
                .append_message(&Message::Assistant(text_reply(&format!("answer {text}"))))
                .unwrap();
        }
        session.append_compaction("the gist", 5000, 2).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("four")))
            .unwrap();

        let reopened = Session::open(session.path()).unwrap();
        let context = reopened.context_messages();
        assert_eq!(context.len(), 4);
        assert!(matches!(
            &context[0],
            Message::User(u) if u.plain_text().contains("the gist")
        ));
        assert!(matches!(&context[1], Message::User(u) if u.plain_text() == "three"));
        assert!(matches!(&context[3], Message::User(u) if u.plain_text() == "four"));
        assert_eq!(SessionSummary::from(&reopened).message_count, 7);
    }

    /// Another termide's claim, as this process sees one: the lock of the
    /// lease file taken on a handle of its own, with the holder's id in it.
    fn hold_elsewhere(log: &Path, pid: u32) -> File {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(lease_path(log))
            .unwrap();
        file.try_lock().unwrap();
        write!(file, "{pid}").unwrap();
        file
    }

    #[test]
    fn a_session_held_by_another_termide_opens_only_to_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("hi")))
            .unwrap();
        let path = session.path().to_path_buf();
        drop(session);

        let other = hold_elsewhere(&path, 4242);
        assert_eq!(Session::held_elsewhere(&path), Some(Some(4242)));
        assert!(Session::is_open(&path));
        let error = Session::open_exclusive(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(error.to_string().contains("4242"), "{error}");
        // The refusal leaves no claim behind in this process.
        assert!(!open_registry().lock().unwrap().contains(&lock_key(&path)));

        // Read, it shows the conversation, and nothing is written to it.
        let mut view = Session::open_read_only(&path).unwrap();
        assert!(view.is_read_only());
        assert_eq!(view.context_messages().len(), 1);
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(view
            .append_message(&Message::User(UserMessage::text("more")))
            .is_err());
        view.discard().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // Once the other lets it go, it is this one's to take.
        drop(other);
        assert_eq!(Session::held_elsewhere(&path), None);
        let taken = Session::open_exclusive(&path).unwrap();
        assert!(!taken.is_read_only());
        assert_eq!(
            lease_holder_id(&lease_path(&path)),
            Some(std::process::id())
        );
        // This process's own claim is not another termide's.
        assert_eq!(Session::held_elsewhere(&path), None);
        drop(taken);
        assert!(
            !lease_path(&path).exists(),
            "the lease file goes with the claim"
        );
    }

    #[test]
    fn a_new_session_is_leased_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::create_exclusive(dir.path(), dir.path()).unwrap();
        let lock = lease_path(session.path());
        assert_eq!(lease_holder_id(&lock), Some(std::process::id()));
        // Another process trying for it now finds it taken.
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock)
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(probe);
        drop(session);
        assert!(!lock.exists());
    }

    #[test]
    fn list_sorts_newest_first_with_first_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let mut older = Session::create(dir.path(), Path::new("/w")).unwrap();
        older
            .append_message(&Message::User(UserMessage::text("first task")))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let mut newer = Session::create(dir.path(), Path::new("/w")).unwrap();
        newer
            .append_message(&Message::User(UserMessage::text("second task")))
            .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

        newer.set_name("the important one").unwrap();

        let list = Session::list(dir.path()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].first_prompt.as_deref(), Some("second task"));
        assert_eq!(list[1].first_prompt.as_deref(), Some("first task"));
        assert_eq!(list[0].message_count, 1);
        // A named session shows its name; an unnamed one its first prompt.
        assert_eq!(list[0].label(), "the important one");
        assert_eq!(list[1].label(), "first task");
        assert!(Session::list(&dir.path().join("missing"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_message_timing_survives_reopen_and_is_optional() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("hi")))
            .unwrap();
        let timing = Timing::Turn {
            prefill_ms: 800,
            gen_ms: 2400,
        };
        session
            .append_timed_message(&Message::Assistant(text_reply("hello")), Some(timing))
            .unwrap();
        let reopened = Session::open(session.path()).unwrap();
        let logged = reopened.context_messages_with_times(&CompactionPrompts::default());
        assert_eq!(logged.len(), 2);
        assert_eq!(logged[0].timing, None);
        assert_eq!(logged[1].timing, Some(timing));
        // An untimed entry writes no `timing` key, so older logs read the same.
        let text = std::fs::read_to_string(session.path()).unwrap();
        assert_eq!(text.matches("\"timing\"").count(), 1, "{text}");
    }

    #[test]
    fn the_connection_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        assert_eq!(session.current_connection(), None);
        session.append_connection_change("cloud").unwrap();
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.current_connection(), Some("cloud".to_string()));
    }

    #[test]
    fn the_reasoning_level_survives_reopen_and_reads_the_old_switch() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        assert_eq!(session.current_thinking(), None);
        // A log written before levels holds the on/off switch.
        session
            .append(EntryKind::ReasoningChange { reasoning: false })
            .unwrap();
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::Off));
        session
            .append(EntryKind::ReasoningChange { reasoning: true })
            .unwrap();
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::High));

        session
            .append_thinking_change(ThinkingLevel::XHigh)
            .unwrap();
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::XHigh));
    }

    #[test]
    fn the_permission_mode_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        assert_eq!(session.current_mode(), None);
        session.append_mode_change(Mode::Plan).unwrap();
        session.append_mode_change(Mode::Edit).unwrap();
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.current_mode(), Some(Mode::Edit));
        // A setup entry: the session stays empty.
        assert!(reopened.is_empty());
    }

    #[test]
    fn the_toolset_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        assert_eq!(session.current_toolset(), None);
        session.append_toolset(&["bash".into()]).unwrap();
        session
            .append_toolset(&["bash".into(), "mcp:github".into()])
            .unwrap();
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(
            reopened.current_toolset(),
            Some(vec!["bash".to_string(), "mcp:github".to_string()])
        );
    }

    #[test]
    fn ids_are_unique() {
        let ids: std::collections::HashSet<String> = (0..1000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1000);
        assert!(ids.iter().all(|id| id.len() == 8));
    }

    #[test]
    fn a_fork_copies_the_branch_under_a_new_id_and_leaves_the_source_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create_exclusive(dir.path(), Path::new("/work")).unwrap();
        session.append_connection_change("cloud").unwrap();
        session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply("a")))
            .unwrap();
        session.set_name("keep me").unwrap();
        let source = session.path().to_path_buf();

        let copy = session
            .fork(Path::new("/other"), Some("keep me (2)"))
            .unwrap();
        assert_ne!(copy, source);
        let forked = Session::open_exclusive(&copy).unwrap();
        // A new log of its own: new id, the asking panel's cwd, and the name
        // it was given — not the source's, which would leave two alike rows.
        assert_ne!(forked.id(), session.id());
        assert_eq!(forked.header().cwd, PathBuf::from("/other"));
        assert_eq!(forked.name(), Some("keep me (2)"));
        // The conversation came across whole, entries and ids included.
        assert_eq!(forked.current_connection().as_deref(), Some("cloud"));
        // The copy's leaf is the name entry it was given, hanging off the
        // source's leaf: what came across keeps its ids and its shape.
        assert_eq!(forked.entries().len(), session.entries().len() + 1);
        let names: Vec<&str> = forked
            .entries()
            .iter()
            .rev()
            .filter_map(|e| match &e.kind {
                EntryKind::SessionName { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["keep me (2)", "keep me"]);
        let texts: Vec<String> = forked
            .context_messages()
            .iter()
            .map(|m| match m {
                Message::User(u) => u.plain_text(),
                Message::Assistant(a) => a.plain_text(),
                Message::ToolResult(_) => String::new(),
            })
            .collect();
        assert_eq!(texts, ["one", "a"]);

        // Writing to one log leaves the other where it was: the source is
        // still claimed by this panel and still holds two messages.
        assert!(Session::is_open(&source));
        session
            .append_message(&Message::User(UserMessage::text("later")))
            .unwrap();
        assert_eq!(Session::open(&copy).unwrap().context_messages().len(), 2);
        assert_eq!(Session::open(&source).unwrap().context_messages().len(), 3);
    }

    #[test]
    fn a_fork_takes_the_current_branch_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
        let keep = session
            .append_message(&Message::User(UserMessage::text("keep")))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply("a")))
            .unwrap();
        session.rewind_to(Some(&keep)).unwrap();

        let forked = Session::open(&session.fork(Path::new("/w"), None).unwrap()).unwrap();
        let texts: Vec<String> = forked
            .context_messages()
            .iter()
            .map(|m| match m {
                Message::User(u) => u.plain_text(),
                Message::Assistant(a) => a.plain_text(),
                Message::ToolResult(_) => String::new(),
            })
            .collect();
        assert_eq!(texts, ["keep"]);
    }

    #[test]
    fn a_move_refiles_the_whole_log_under_the_new_cwd_and_removes_the_old_one() {
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::create(&root.path().join("a"), Path::new("/a")).unwrap();
        session.append_connection_change("cloud").unwrap();
        session.set_name("named").unwrap();
        let source = session.path().to_path_buf();
        let other = root.path().join("b");

        let moved = session.move_into(&other, Path::new("/b")).unwrap();
        assert_eq!(moved, other.join(source.file_name().unwrap()));
        assert!(!source.exists());
        let reopened = Session::open(&moved).unwrap();
        assert_eq!(reopened.id(), session.id());
        assert_eq!(reopened.header().cwd, PathBuf::from("/b"));
        assert_eq!(reopened.current_connection().as_deref(), Some("cloud"));
        assert_eq!(reopened.name(), Some("named"));
    }

    #[test]
    fn a_rewind_drops_the_undone_messages_from_the_branch_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        let keep = session
            .append_message(&Message::Assistant(text_reply("a")))
            .unwrap();
        session
            .append_message(&Message::User(UserMessage::text("two")))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply("b")))
            .unwrap();
        session.rewind_to(Some(&keep)).unwrap();
        assert_eq!(session.context_messages().len(), 2);
        session
            .append_message(&Message::User(UserMessage::text("three")))
            .unwrap();
        let reopened = Session::open(session.path()).unwrap();
        let texts: Vec<String> = reopened
            .context_messages()
            .iter()
            .map(|m| match m {
                Message::User(u) => u.plain_text(),
                Message::Assistant(a) => a.plain_text(),
                Message::ToolResult(_) => String::new(),
            })
            .collect();
        assert_eq!(texts, ["one", "a", "three"]);
        assert!(session.rewind_to(Some("missing")).is_err());
    }
}
#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn an_exclusive_session_cannot_be_opened_twice_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::create_exclusive(dir.path(), Path::new("/work")).unwrap();
        let path = session.path().to_path_buf();
        // A second exclusive open is refused while the first is alive.
        let again = Session::open_exclusive(&path);
        assert_eq!(again.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        // A plain open (for reads/tests) is still allowed.
        assert!(Session::open(&path).is_ok());
        // Dropping the holder releases the claim.
        drop(session);
        assert!(Session::open_exclusive(&path).is_ok());
    }

    #[test]
    fn an_exclusive_claim_is_visible_and_moves_the_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = Session::create(dir.path(), Path::new("/work"))
            .unwrap()
            .path()
            .to_path_buf();
        assert!(!Session::is_open(&path));
        let before = Session::open_generation();
        let session = Session::open_exclusive(&path).unwrap();
        assert!(Session::is_open(&path));
        let claimed = Session::open_generation();
        assert_ne!(claimed, before);
        drop(session);
        assert!(!Session::is_open(&path));
        assert_ne!(Session::open_generation(), claimed);
    }
}
