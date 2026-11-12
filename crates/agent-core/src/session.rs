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
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::acp::ACP_PROVIDER;
use crate::compaction::{context_tokens, CompactionCall, CompactionPrompts};
use crate::context::file_timestamp;
use crate::failure::Failure;
use crate::fit::{fit, FitStep};
use crate::goal::GoalOutcome;
use crate::message::{now_millis, Message, StopReason, Usage, UserMessage};
use crate::permissions::Mode;
use crate::provider::ThinkingLevel;
use crate::prune::{prune_by, prune_to_decisions};
use crate::spend::{Spender, Spending, Timeline};

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
    /// How many of the log's first entries were copied from another log —
    /// a fork's source branch, whose spend is the source's — `0` for a
    /// log begun empty. `None` in a log older than the field, which is
    /// told by its entries' timestamps instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copied: Option<usize>,
}

/// Where the entries this log wrote itself begin in `entries`, those
/// before having been copied from a fork's source: as `header` counts them,
/// or in an older log, the first entry stamped after the header.
pub(crate) fn own_start(header: &SessionHeader, entries: &[Entry]) -> usize {
    match header.copied {
        Some(copied) => copied.min(entries.len()),
        None => entries
            .iter()
            .position(|entry| entry.timestamp >= header.created)
            .unwrap_or(entries.len()),
    }
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

/// A step of the conversation on the current branch, as
/// [`Session::history_with_times`] gives it.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryStep {
    Message(LoggedMessage),
    /// The context was summarised, its last `kept` messages kept as they were.
    Compacted {
        tokens_before: u64,
        kept: usize,
    },
    /// The context was cut down by kinds of content, with no summary.
    Fitted {
        tokens_before: u64,
        tokens_after: u64,
    },
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
    /// The answers of the tasks handed over in the background that the user
    /// stopped with a run, kept for the next request rather than starting
    /// one; empty once they went with it. A reopened session sends them with
    /// its next request, see [`Session::held_reports`].
    HeldReports {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reports: Vec<UserMessage>,
    },
    /// What the `/goal` judge answered: the `outcome` read from it with its
    /// `reason`, or no outcome and the `error` the check failed on — no
    /// answer, or one holding no verdict. `reply` is the answer as it came,
    /// cut to [`JUDGE_REPLY_CHARS`]. Changes nothing on the branch; it is
    /// there to tell afterwards what the judge said, and so a reopened
    /// session shows how its goal ended, see [`Session::goal_ending`].
    GoalVerdict {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<GoalOutcome>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        error: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reply: String,
    },
    /// Tokens a model call spent that nothing else in the log records — the
    /// `auto` reviewer's, the judge's, a handoff brief's — on `model` when
    /// known. Changes nothing on the branch; see [`crate::spend::Spending`].
    Spent {
        by: Spender,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        model: String,
        usage: Usage,
    },
    /// How full an external agent's context stood at a turn's end, `used`
    /// of `size` tokens, as the agent reported it: its turns' usage is what
    /// they spent, not the fill, so a reopened session reads it from here.
    /// Changes nothing on the branch; see [`Session::context_fill`].
    ContextFill { used: u64, size: u64 },
}

/// How much of a judge's answer a [`EntryKind::GoalVerdict`] keeps.
pub const JUDGE_REPLY_CHARS: usize = 4_000;

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
pub(crate) fn read_file(path: &Path) -> std::io::Result<(SessionHeader, Vec<Entry>)> {
    let reader = BufReader::new(File::open(path)?);
    let mut header = None;
    let mut entries = Vec::new();
    // Split on bytes: a write torn inside a character must not make the
    // whole log unreadable.
    for (index, line) in reader.split(b'\n').enumerate() {
        let line = line?;
        let line = String::from_utf8_lossy(&line);
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Line>(&line) {
            Ok(Line::Header(h)) if header.is_none() => header = Some(h.header),
            Ok(Line::Header(_)) => {
                log::warn!("{}: duplicate header at line {}", path.display(), index + 1)
            }
            Ok(Line::Entry(entry)) => entries.push(entry),
            Err(error) => match salvage_entry(&line) {
                Some(entry) => {
                    log::warn!(
                        "{}: line {} starts with a torn write; kept the entry after it",
                        path.display(),
                        index + 1
                    );
                    entries.push(entry);
                }
                None => log::warn!(
                    "{}: skipping malformed line {}: {error}",
                    path.display(),
                    index + 1
                ),
            },
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

/// The entry at the end of a line that does not parse as a whole: a write cut
/// off by a crash, with the next entry appended to it before the file was
/// mended (see [`end_with_newline`]). An entry's line starts with `{"id":"`;
/// inside one, that text is either escaped in a string or a nested object
/// followed by more of its parent, so only a whole entry at the end parses.
fn salvage_entry(line: &str) -> Option<Entry> {
    line.match_indices(r#"{"id":""#)
        .filter(|(at, _)| *at > 0)
        .find_map(|(at, _)| serde_json::from_str::<Entry>(&line[at..]).ok())
}

/// Before appending to a log, finish a last line a crash left without its
/// newline, so the next entry starts a line of its own instead of being
/// glued to the torn one and lost with it.
fn end_with_newline(file: &mut File) -> std::io::Result<()> {
    if file.metadata()?.len() == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    if last[0] != b'\n' {
        // The file is in append mode: the write lands at the end.
        file.write_all(b"\n")?;
        file.flush()?;
    }
    Ok(())
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
///
/// A parent that is not in the log was lost to a torn write. The branch then
/// carries on from the entry written just before its child — where the lost
/// one was in a log that was not rewound there — so one lost line does not
/// cut off everything said before it.
fn branch_of<'a>(entries: &'a [Entry], leaf: Option<&str>, path: &Path) -> Vec<&'a Entry> {
    let by_id: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, e)| (e.id.as_str(), index))
        .collect();
    let mut branch = Vec::new();
    let mut current = leaf;
    let mut child: Option<usize> = None;
    while let Some(id) = current {
        let index = match by_id.get(id) {
            Some(&index) => index,
            None => {
                log::warn!("{}: dangling parent id {id}", path.display());
                match child.and_then(|child| child.checked_sub(1)) {
                    Some(before) => before,
                    None => break,
                }
            }
        };
        // Parents come before their children; anything else is a loop.
        if child.is_some_and(|child| index >= child) {
            log::warn!("{}: parent chain loops at {id}", path.display());
            break;
        }
        branch.push(&entries[index]);
        child = Some(index);
        current = entries[index].parent_id.as_deref();
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
            copied: Some(0),
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
        let mut file = OpenOptions::new().read(true).append(true).open(path)?;
        end_with_newline(&mut file)?;
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
        let branch = self.branch();
        header.copied = Some(branch.len());
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
            for entry in &branch {
                write_line(&mut file, &Line::Entry((*entry).clone()))?;
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
                | EntryKind::HeldReports { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::GoalVerdict { .. }
                | EntryKind::Spent { .. }
                | EntryKind::ContextFill { .. }
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
                | EntryKind::HeldReports { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::GoalVerdict { .. }
                | EntryKind::Spent { .. }
                | EntryKind::ContextFill { .. }
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

    /// The current branch as the conversation went, for showing it again:
    /// every message, those a compaction or a fitting took out of the
    /// context included, and a step where the context was cut down. A
    /// fitting's size after it is replayed from the context, worded by
    /// `prompts`, as the agent measured it.
    #[must_use]
    pub fn history_with_times(&self, prompts: &CompactionPrompts) -> Vec<HistoryStep> {
        let mut steps: Vec<HistoryStep> = Vec::new();
        // The context as the model saw it, for what a fitting left of it.
        let mut context: Vec<Message> = Vec::new();
        for entry in self.branch() {
            match &entry.kind {
                EntryKind::Message { message, timing } => {
                    context.push(message.clone());
                    steps.push(HistoryStep::Message(LoggedMessage {
                        message: message.clone(),
                        timestamp: entry.timestamp,
                        timing: *timing,
                    }));
                }
                EntryKind::Compaction {
                    summary,
                    keep_last,
                    tokens_before,
                    ..
                } => {
                    let tail = context.split_off(context.len().saturating_sub(*keep_last));
                    context = vec![prompts.summary_message(summary)];
                    context.extend(tail);
                    steps.push(HistoryStep::Compacted {
                        tokens_before: *tokens_before,
                        kept: *keep_last,
                    });
                }
                EntryKind::Fitted {
                    budget,
                    keep_tail,
                    tokens_before,
                    ..
                } => {
                    context = fit(&context, *budget, *keep_tail).messages;
                    steps.push(HistoryStep::Fitted {
                        tokens_before: *tokens_before,
                        tokens_after: context_tokens(&context),
                    });
                }
                EntryKind::Pruned => context = prune_to_decisions(&context),
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
                | EntryKind::HeldReports { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::GoalVerdict { .. }
                | EntryKind::Spent { .. }
                | EntryKind::ContextFill { .. }
                | EntryKind::CompactionFailed { .. } => {}
            }
        }
        steps
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

    /// A name split into its base and the fork counter a fork's copy is
    /// named with: `Refactor (2)` → (`Refactor`, `Some(2)`). Only the suffix
    /// forks write counts — digits in brackets after a space, two or more —
    /// so a name the user gave, `work (draft)` or `fix (1)`, is all base.
    #[must_use]
    pub fn split_fork_counter(name: &str) -> (&str, Option<u32>) {
        let trimmed = name.trim_end();
        let counter = trimmed
            .strip_suffix(')')
            .and_then(|head| head.rsplit_once('('))
            .and_then(|(before, digits)| {
                let n = digits.parse::<u32>().ok()?;
                (n >= 2 && before.ends_with(' ') && !digits.starts_with('+'))
                    .then(|| (before.trim_end(), n))
            });
        match counter {
            Some((base, n)) => (base, Some(n)),
            None => (name, None),
        }
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
                | EntryKind::HeldReports { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::CompactionFailed { .. }
                | EntryKind::GoalVerdict { .. }
                | EntryKind::Spent { .. }
                | EntryKind::ContextFill { .. }
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

    /// Whether the current branch ever switched to `mode`.
    #[must_use]
    pub fn had_mode(&self, mode: Mode) -> bool {
        self.branch()
            .into_iter()
            .any(|entry| matches!(entry.kind, EntryKind::ModeChange { mode: m } if m == mode))
    }

    /// The permission mode the last user message outside plan mode was sent
    /// in on the current branch: the mode a plan made now was planned from.
    /// A mode only passed through on the way to another is not one. `None`
    /// when no such message was sent after a recorded mode change.
    #[must_use]
    pub fn working_mode(&self) -> Option<Mode> {
        let mut mode = None;
        let mut working = None;
        for entry in self.branch() {
            match &entry.kind {
                EntryKind::ModeChange { mode: changed } => mode = Some(*changed),
                EntryKind::Message {
                    message: Message::User(_),
                    ..
                } if mode != Some(Mode::Plan) => working = mode.or(working),
                _ => {}
            }
        }
        working
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
                | EntryKind::HeldReports { .. }
                | EntryKind::Autorun { .. }
                | EntryKind::GoalVerdict { .. }
                | EntryKind::Spent { .. }
                | EntryKind::ContextFill { .. }
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

    /// Record the tokens a call of `by`'s spent on `model`, see
    /// [`EntryKind::Spent`].
    pub fn append_spent(
        &mut self,
        by: Spender,
        model: &str,
        usage: Usage,
    ) -> std::io::Result<String> {
        self.append(EntryKind::Spent {
            by,
            model: model.to_string(),
            usage,
        })
    }

    /// Record how full an external agent's context stands, see
    /// [`EntryKind::ContextFill`].
    pub fn append_context_fill(&mut self, used: u64, size: u64) -> std::io::Result<String> {
        self.append(EntryKind::ContextFill { used, size })
    }

    /// How full the context stood when the branch was last written to, as
    /// far as the log tells: the latest of an external agent's reported
    /// fill (with the window it reported), the size a compaction left, or
    /// the last call of the built-in loop's (its usage is one call's, the
    /// prompt it sent and the reply). `None` when the branch tells nothing.
    #[must_use]
    pub fn context_fill(&self) -> Option<(u64, Option<u64>)> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::ContextFill { used, size } => Some((*used, Some(*size))),
                EntryKind::Compaction {
                    tokens_after: Some(after),
                    ..
                } => Some((*after, None)),
                EntryKind::Message {
                    message: Message::Assistant(reply),
                    ..
                } if reply.usage.total() > 0 && reply.provider != crate::ACP_PROVIDER => {
                    Some((reply.usage.total(), None))
                }
                _ => None,
            })
    }

    /// What the whole log spent, by whom and on which model — a fork not
    /// counting what its source spent before it.
    #[must_use]
    pub fn spending(&self) -> Spending {
        Spending::of_entries(&self.entries, self.own_start())
    }

    /// [`Self::spending`] with the subagents' logs read, see
    /// [`Spending::with_subagents`]: files are read, so not for every frame.
    #[must_use]
    pub fn spending_with_subagents(&self) -> Spending {
        Spending::with_subagents(&self.entries, self.own_start())
    }

    /// [`Self::spending_with_subagents`] hour by hour, and the delegates'
    /// logs read for it; see [`Timeline::with_delegates`].
    #[must_use]
    pub fn timeline_with_delegates(&self) -> (Timeline, HashSet<PathBuf>) {
        Timeline::with_delegates(&self.entries, self.own_start())
    }

    /// The entries this log wrote itself, after those a fork copied from its
    /// source; see [`SessionHeader::copied`].
    #[must_use]
    pub fn own_entries(&self) -> &[Entry] {
        &self.entries[self.own_start()..]
    }

    fn own_start(&self) -> usize {
        own_start(&self.header, &self.entries)
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

    /// Record what the `/goal` judge answered: the verdict read from `reply`,
    /// or (`Err`) why the check failed. See [`EntryKind::GoalVerdict`].
    pub fn append_goal_verdict(
        &mut self,
        verdict: Result<(GoalOutcome, &str), &str>,
        reply: &str,
    ) -> std::io::Result<String> {
        let (outcome, reason, error) = match verdict {
            Ok((outcome, reason)) => (Some(outcome), reason, ""),
            Err(error) => (None, "", error),
        };
        self.append(EntryKind::GoalVerdict {
            outcome,
            reason: reason.to_string(),
            error: error.to_string(),
            reply: reply.chars().take(JUDGE_REPLY_CHARS).collect(),
        })
    }

    /// How the branch's goal ended, when the judge's verdict ending it —
    /// `DONE` or `IMPOSSIBLE` — is the last word of the conversation, no
    /// message after it: the outcome and its reason, so a reopened session
    /// shows the notice the panel showed then.
    #[must_use]
    pub fn goal_ending(&self) -> Option<(GoalOutcome, String)> {
        let branch = self.branch();
        let last = branch.into_iter().rev().find(|entry| {
            matches!(
                entry.kind,
                EntryKind::GoalVerdict { .. } | EntryKind::Message { .. }
            )
        })?;
        match &last.kind {
            EntryKind::GoalVerdict {
                outcome: Some(outcome @ (GoalOutcome::Done | GoalOutcome::Impossible)),
                reason,
                ..
            } => Some((*outcome, reason.clone())),
            _ => None,
        }
    }

    /// Record a failed request the panel still holds (`None`: it let it go),
    /// see [`Self::pending_retry`].
    pub fn append_retry(&mut self, failure: Option<Failure>) -> std::io::Result<String> {
        self.append(EntryKind::Retry { failure })
    }

    /// Record the answers held for the next request (empty: they went with
    /// it), see [`Self::held_reports`].
    pub fn append_held_reports(&mut self, reports: &[UserMessage]) -> std::io::Result<String> {
        self.append(EntryKind::HeldReports {
            reports: reports.to_vec(),
        })
    }

    /// The answers of the background tasks stopped with a run that the
    /// branch last left held for its next request: see
    /// [`EntryKind::HeldReports`].
    #[must_use]
    pub fn held_reports(&self) -> Vec<UserMessage> {
        self.branch()
            .into_iter()
            .rev()
            .find_map(|entry| match &entry.kind {
                EntryKind::HeldReports { reports } => Some(reports.clone()),
                _ => None,
            })
            .unwrap_or_default()
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

    /// Bring a fork made in the middle of a run to rest: each call of the
    /// last reply still without a result gets `result(call)`, and the run
    /// left open gets its end. The copy then opens as a conversation between
    /// requests — nothing offered to carry on, no call run a second time —
    /// rather than as a run cut off. A paused run keeps its pause.
    ///
    /// # Errors
    ///
    /// When the log cannot be written.
    pub fn settle(
        &mut self,
        result: impl Fn(&crate::message::ToolCall) -> crate::message::ToolResultMessage,
    ) -> std::io::Result<()> {
        for call in crate::agent::unanswered_calls(&self.context_messages()) {
            self.append_message(&Message::ToolResult(result(&call)))?;
        }
        if self.last_run() == LastRun::CutOff {
            self.append_run_end(false)?;
        }
        Ok(())
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
            | EntryKind::HeldReports { .. }
            | EntryKind::Autorun { .. }
            | EntryKind::CompactionFailed { .. }
            | EntryKind::GoalVerdict { .. }
            | EntryKind::Spent { .. }
            | EntryKind::ContextFill { .. }
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
mod tests;

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

    /// The context's fill reads back from the latest that tells it: an
    /// external agent's logged fill, a compaction, the built-in loop's call.
    #[test]
    fn the_context_fill_reads_back_from_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        assert_eq!(session.context_fill(), None);
        let mut reply = crate::agent::test_support::text_reply("ok");
        reply.usage.input = 900;
        reply.usage.output = 100;
        session
            .append_message(&Message::Assistant(reply.clone()))
            .unwrap();
        assert_eq!(session.context_fill(), Some((1_000, None)));
        session.append_context_fill(5_000, 200_000).unwrap();
        // An external agent's reply is what its turn spent: not the fill.
        reply.provider = crate::ACP_PROVIDER.to_string();
        session.append_message(&Message::Assistant(reply)).unwrap();
        let reopened = Session::open(session.path()).unwrap();
        assert_eq!(reopened.context_fill(), Some((5_000, Some(200_000))));
        session
            .append_compaction_with("summary", 5_000, 0, Some(700), None)
            .unwrap();
        assert_eq!(session.context_fill(), Some((700, None)));
    }
}
