//! The project's agent sessions as search documents.
//!
//! Every `*.jsonl` log under the project's session directory is read along
//! its live branch ([`Session::read_branch`]): what a rewind undid is left
//! out, what a compaction replaced is kept (the originals are still in the
//! file). Each message block becomes a document, weighted by kind. Parsed
//! files stay in a process-wide cache keyed by size and modification time,
//! so a second search does not read 20 MB of logs again; the few session
//! directories searched last are kept, each behind its own lock.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::SystemTime;

use regex::Regex;
use serde_json::Value;
use termide_agent_core::handoff::CONTINUATION_LEAD;
use termide_agent_core::message::{AssistantContent, Message};
use termide_agent_core::{prune_by, CancelToken, Entry, EntryKind, Session};

use crate::filter::PathFilter;
use crate::rank::{bm25, recency, Hit, Source, TermBag, Vocabulary};
use crate::text::terms;

/// Tool output is indexed and shown from its head only: what matters (an
/// error, a commit line) is near the top, and the rest is mostly noise.
pub const OUTPUT_INDEX_BYTES: usize = 4 * 1024;
/// A tool call's arguments are indexed up to this many bytes.
const CALL_INDEX_BYTES: usize = 2 * 1024;

/// What a session document is, which sets its weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    User,
    Assistant,
    Thinking,
    ToolCall,
    ToolOutput,
    Compaction,
    Handoff,
}

impl DocKind {
    /// How much a match here counts: summaries and handoff briefs are
    /// distilled and count most, reasoning and tool output are long and
    /// noisy and count least.
    #[must_use]
    pub fn weight(self) -> f64 {
        match self {
            Self::Compaction | Self::Handoff => 1.5,
            Self::User | Self::Assistant => 1.0,
            Self::ToolCall => 0.8,
            Self::Thinking => 0.5,
            Self::ToolOutput => 0.3,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Thinking => "thinking",
            Self::ToolCall => "tool call",
            Self::ToolOutput => "tool output",
            Self::Compaction => "compaction summary",
            Self::Handoff => "handoff brief",
        }
    }
}

/// One searchable block of a session.
#[derive(Debug, Clone)]
pub struct SessionDoc {
    pub entry: String,
    /// Which block of the entry this is, so a fork's copy of the entry
    /// dedups block by block.
    pub block: usize,
    pub timestamp: u64,
    pub kind: DocKind,
    pub text: String,
    /// Paths the entry's tool call names, as written.
    pub paths: Vec<String>,
    /// The commit the entry's `git commit` call made.
    pub commit: Option<String>,
    /// Whether a compaction took the entry out of its session's context.
    pub forgotten: bool,
    bag: TermBag,
}

/// A parsed log.
#[derive(Debug)]
struct ParsedFile {
    modified: Option<SystemTime>,
    len: u64,
    id: String,
    cwd: PathBuf,
    docs: Vec<SessionDoc>,
}

/// Session directories whose parsed logs stay cached, most recently
/// searched first; another one pushes the oldest out.
const CACHED_DIRS: usize = 4;

/// The parsed logs of one session directory.
#[derive(Debug, Default)]
struct Cache {
    vocabulary: Vocabulary,
    files: HashMap<PathBuf, ParsedFile>,
}

/// The cached directories, most recent first.
type Caches = Vec<(PathBuf, Arc<Mutex<Cache>>)>;

/// The cache of `dir`, made the most recent.
fn cache(dir: &Path) -> Arc<Mutex<Cache>> {
    static CACHES: OnceLock<Mutex<Caches>> = OnceLock::new();
    let mut caches = CACHES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = match caches.iter().position(|(cached, _)| cached == dir) {
        Some(at) => caches.remove(at),
        None => (dir.to_path_buf(), Arc::default()),
    };
    let shared = Arc::clone(&entry.1);
    caches.insert(0, entry);
    caches.truncate(CACHED_DIRS);
    shared
}

/// `cache` locked, waiting for another search of the same directory no
/// longer than `deadline`; `None` when that came first or the run was
/// cancelled.
fn lock_until<'a>(
    cache: &'a Mutex<Cache>,
    deadline: Option<std::time::Instant>,
    cancel: &CancelToken,
) -> Option<MutexGuard<'a, Cache>> {
    loop {
        match cache.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if cancel.is_cancelled() || deadline.is_some_and(|d| std::time::Instant::now() > d)
                {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

/// Every `*.jsonl` under `dir`, recursively; checkpoint copies are skipped.
fn log_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if entry.file_name() != "checkpoints" {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                files.push(path);
            }
        }
    }
    files
}

/// The first `max` bytes of `text`, cut at a character boundary.
fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A tool call's arguments as one line of text: the string values of its
/// top-level fields, the ones a search would name (a command, a path, a
/// pattern), in order.
fn call_text(name: &str, arguments: &Value) -> String {
    let mut text = name.to_string();
    if let Value::Object(fields) = arguments {
        for value in fields.values() {
            match value {
                Value::String(s) => {
                    text.push(' ');
                    text.push_str(s);
                }
                Value::Array(items) => {
                    for item in items.iter().filter_map(Value::as_str) {
                        text.push(' ');
                        text.push_str(item);
                    }
                }
                _ => {}
            }
        }
    }
    head(&text, CALL_INDEX_BYTES).to_string()
}

/// The commit `git commit` reported: `[main 08c2e3b1] subject`.
fn committed_sha(output: &str) -> Option<String> {
    static LINE: OnceLock<Regex> = OnceLock::new();
    let line = LINE.get_or_init(|| {
        Regex::new(r"(?m)^\[[^\]\n]* ([0-9a-f]{7,40})\]").expect("valid commit-line pattern")
    });
    line.captures(output).map(|c| c[1].to_string())
}

/// Where the entries of `branch` still in the session's context begin: the
/// messages the last compaction kept, then its summary; the start without
/// one. Mirrors how the session rebuilds its context.
fn context_start(branch: &[Entry]) -> usize {
    let Some((at, keep_last)) = branch
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, entry)| match entry.kind {
            EntryKind::Compaction { keep_last, .. } => Some((at, keep_last)),
            _ => None,
        })
    else {
        return 0;
    };
    let messages: Vec<usize> = branch[..at]
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry.kind, EntryKind::Message { .. }))
        .map(|(index, _)| index)
        .collect();
    match messages.len().checked_sub(keep_last) {
        Some(first) => messages.get(first).copied().unwrap_or(at),
        None => 0,
    }
}

/// The message entries of `branch` that a plan carried out from a clean
/// context cut out of the context, or cut down: those before its last
/// [`EntryKind::Pruned`] that [`prune_by`] does not keep as they were.
/// Mirrors how the session rebuilds its context.
fn pruned_away(branch: &[Entry]) -> Vec<&str> {
    let Some(at) = branch
        .iter()
        .rposition(|entry| matches!(entry.kind, EntryKind::Pruned))
    else {
        return Vec::new();
    };
    let messages: Vec<(&str, &Message)> = branch[..at]
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message { message, .. } => Some((entry.id.as_str(), message)),
            _ => None,
        })
        .collect();
    let intact: HashSet<&str> = prune_by(&messages, |(_, message)| message)
        .into_iter()
        .filter(|((_, original), pruned)| same_content(original, pruned))
        .map(|((id, _), _)| *id)
        .collect();
    messages
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !intact.contains(id))
        .collect()
}

/// Whether pruning left `pruned` saying what `original` said; the usage it
/// clears is not part of what the model reads.
fn same_content(original: &Message, pruned: &Message) -> bool {
    match (original, pruned) {
        (Message::Assistant(original), Message::Assistant(pruned)) => {
            original.content == pruned.content
        }
        _ => original == pruned,
    }
}

/// The documents of one log's live branch.
fn parse(
    path: &Path,
    vocabulary: &mut Vocabulary,
) -> std::io::Result<(String, PathBuf, Vec<SessionDoc>)> {
    let (header, branch) = Session::read_branch(path)?;
    let mut docs: Vec<SessionDoc> = Vec::new();
    // A call's path and whether it commits, for its result; the index of its
    // own document, which learns the commit once the result shows it.
    let mut calls: HashMap<String, (Vec<String>, bool, usize)> = HashMap::new();
    let mut push = |docs: &mut Vec<SessionDoc>,
                    entry: &str,
                    block: usize,
                    timestamp: u64,
                    kind: DocKind,
                    text: String,
                    paths: Vec<String>| {
        if text.trim().is_empty() {
            return;
        }
        let bag = TermBag::new(vocabulary, terms(&text));
        docs.push(SessionDoc {
            entry: entry.to_string(),
            block,
            timestamp,
            kind,
            text,
            paths,
            commit: None,
            forgotten: false,
            bag,
        });
    };
    for entry in &branch {
        let (id, at) = (entry.id.as_str(), entry.timestamp);
        match &entry.kind {
            EntryKind::Compaction { summary, .. } => {
                push(
                    &mut docs,
                    id,
                    0,
                    at,
                    DocKind::Compaction,
                    summary.clone(),
                    Vec::new(),
                );
            }
            EntryKind::Message { message, .. } => match message {
                Message::User(user) => {
                    let text = user.plain_text();
                    if let Some(command) = &user.ran {
                        push(
                            &mut docs,
                            id,
                            0,
                            at,
                            DocKind::ToolCall,
                            command.clone(),
                            Vec::new(),
                        );
                        let output = head(&text, OUTPUT_INDEX_BYTES).to_string();
                        push(
                            &mut docs,
                            id,
                            1,
                            at,
                            DocKind::ToolOutput,
                            output,
                            Vec::new(),
                        );
                    } else {
                        let kind = if text.starts_with(CONTINUATION_LEAD) {
                            DocKind::Handoff
                        } else {
                            DocKind::User
                        };
                        push(&mut docs, id, 0, at, kind, text, Vec::new());
                    }
                }
                Message::Assistant(reply) => {
                    for (block, content) in reply.content.iter().enumerate() {
                        match content {
                            AssistantContent::Text { text } => {
                                push(
                                    &mut docs,
                                    id,
                                    block,
                                    at,
                                    DocKind::Assistant,
                                    text.clone(),
                                    Vec::new(),
                                );
                            }
                            AssistantContent::Thinking { text, .. } => {
                                push(
                                    &mut docs,
                                    id,
                                    block,
                                    at,
                                    DocKind::Thinking,
                                    text.clone(),
                                    Vec::new(),
                                );
                            }
                            AssistantContent::ToolCall(call) => {
                                let paths: Vec<String> = ["path", "file_path"]
                                    .iter()
                                    .filter_map(|key| call.arguments[*key].as_str())
                                    .map(str::to_string)
                                    .collect();
                                let commits = call.name == "bash"
                                    && call.arguments["command"]
                                        .as_str()
                                        .is_some_and(|c| c.contains("git commit"));
                                let index = docs.len();
                                push(
                                    &mut docs,
                                    id,
                                    block,
                                    at,
                                    DocKind::ToolCall,
                                    call_text(&call.name, &call.arguments),
                                    paths.clone(),
                                );
                                calls.insert(call.id.clone(), (paths, commits, index));
                            }
                        }
                    }
                }
                Message::ToolResult(result) => {
                    let text = result.plain_text();
                    let (paths, commits, call_doc) = calls
                        .get(&result.tool_call_id)
                        .cloned()
                        .unwrap_or((Vec::new(), false, usize::MAX));
                    let commit = commits.then(|| committed_sha(&text)).flatten();
                    let before = docs.len();
                    push(
                        &mut docs,
                        id,
                        0,
                        at,
                        DocKind::ToolOutput,
                        head(&text, OUTPUT_INDEX_BYTES).to_string(),
                        paths,
                    );
                    if let Some(sha) = commit {
                        if docs.len() > before {
                            docs[before].commit = Some(sha.clone());
                        }
                        if let Some(doc) = docs.get_mut(call_doc) {
                            doc.commit = Some(sha);
                        }
                    }
                }
            },
            _ => {}
        }
    }
    let start = context_start(&branch);
    let mut forgotten: HashSet<&str> = branch[..start]
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    forgotten.extend(pruned_away(&branch[start..]));
    for doc in &mut docs {
        doc.forgotten = forgotten.contains(doc.entry.as_str());
    }
    Ok((header.id, header.cwd, docs))
}

/// What a session search is narrowed to and leaves out.
#[derive(Debug, Clone, Default)]
pub struct SessionQuery<'a> {
    /// The terms of each phrasing of the query.
    pub terms: &'a [Vec<Vec<String>>],
    /// Patterns for the snippet window.
    pub patterns: &'a [String],
    /// The calling session, whose content is in the context already: only
    /// what a compaction took out of it is searched.
    pub exclude: Option<&'a str>,
    pub since: Option<u64>,
    pub paths: Option<&'a PathFilter>,
    /// Where relative and absolute paths in the logs are made relative to.
    pub project_root: Option<&'a Path>,
    pub now: u64,
    /// When to stop reading logs not parsed yet; the ones read by then are
    /// ranked.
    pub deadline: Option<std::time::Instant>,
}

/// `path` as the logs wrote it, relative to `root` when it is under it,
/// absolute ones resolved against nothing else.
fn project_relative(path: &str, cwd: &Path, root: Option<&Path>) -> String {
    let absolute = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };
    root.and_then(|root| absolute.strip_prefix(root).ok())
        .map_or_else(
            || path.to_string(),
            |rel| rel.to_string_lossy().into_owned(),
        )
}

/// The best session hits for `query` under `dir`, best first, and whether
/// the deadline came before every log was read.
#[must_use]
pub fn search(
    dir: &Path,
    query: &SessionQuery<'_>,
    limit: usize,
    cancel: &CancelToken,
) -> (Vec<Hit>, bool) {
    let shared = cache(dir);
    let Some(mut cache) = lock_until(&shared, query.deadline, cancel) else {
        return (Vec::new(), !cancel.is_cancelled());
    };
    let mut files: Vec<(PathBuf, Option<SystemTime>, u64)> = log_files(dir)
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            Some((path, meta.modified().ok(), meta.len()))
        })
        .collect();
    // Newest first, so a fork's copy of an entry yields to the newer log.
    files.sort_by(|a, b| b.1.cmp(&a.1));
    let live: HashSet<&PathBuf> = files.iter().map(|(path, _, _)| path).collect();
    let stale: Vec<PathBuf> = cache
        .files
        .keys()
        .filter(|path| !live.contains(path))
        .cloned()
        .collect();
    for path in stale {
        cache.files.remove(&path);
    }
    let mut cut_short = false;
    for (path, modified, len) in &files {
        if cancel.is_cancelled() {
            return (Vec::new(), false);
        }
        let fresh = cache
            .files
            .get(path)
            .is_some_and(|parsed| parsed.modified == *modified && parsed.len == *len);
        if fresh {
            continue;
        }
        // Only parsing takes time; a log already cached is ranked anyway.
        if query
            .deadline
            .is_some_and(|d| std::time::Instant::now() > d)
        {
            cut_short = true;
            break;
        }
        let Cache {
            vocabulary,
            files: parsed,
        } = &mut *cache;
        match parse(path, vocabulary) {
            Ok((id, cwd, docs)) => {
                parsed.insert(
                    path.clone(),
                    ParsedFile {
                        modified: *modified,
                        len: *len,
                        id,
                        cwd,
                        docs,
                    },
                );
            }
            Err(error) => {
                log::warn!("recall: skipping {}: {error}", path.display());
                parsed.remove(path);
            }
        }
    }

    let mut seen: HashSet<(&str, usize)> = HashSet::new();
    let mut candidates: Vec<(&ParsedFile, &SessionDoc)> = Vec::new();
    for (path, _, _) in &files {
        let Some(parsed) = cache.files.get(path) else {
            continue;
        };
        // The calling session's context holds its log, but for what a
        // compaction took out of it.
        let calling = query.exclude == Some(parsed.id.as_str());
        for doc in parsed.docs.iter().filter(|doc| !calling || doc.forgotten) {
            if query.since.is_some_and(|since| doc.timestamp < since) {
                continue;
            }
            if let Some(filter) = query.paths.filter(|f| !f.is_empty()) {
                let touches = doc
                    .paths
                    .iter()
                    .any(|p| filter.matches(&project_relative(p, &parsed.cwd, query.project_root)));
                if !touches && !filter.mentioned_in(&doc.text) {
                    continue;
                }
            }
            if seen.insert((doc.entry.as_str(), doc.block)) {
                candidates.push((parsed, doc));
            }
        }
    }
    let query_ids = cache.vocabulary.phrases(query.terms);
    let bags: Vec<&TermBag> = candidates.iter().map(|(_, doc)| &doc.bag).collect();
    let scores = bm25(&bags, &query_ids);
    let mut ranked: Vec<(f64, &ParsedFile, &SessionDoc)> = candidates
        .into_iter()
        .zip(scores)
        .filter(|(_, score)| *score > 0.0)
        .map(|((parsed, doc), score)| {
            (
                score * doc.kind.weight() * recency(doc.timestamp, query.now),
                parsed,
                doc,
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.truncate(limit);
    let hits = ranked
        .into_iter()
        .map(|(score, parsed, doc)| Hit {
            source: Source::Session,
            reference: format!("session:{}#{}", parsed.id, doc.entry),
            timestamp: Some(doc.timestamp),
            label: doc.kind.label().to_string(),
            snippet: crate::snippet(&doc.text, query.patterns),
            related: doc.commit.as_ref().map(|sha| format!("commit {sha}")),
            score,
        })
        .collect();
    (hits, cut_short)
}

/// The log whose header id is `id` under `dir`: file names end in
/// `_<id>.jsonl`, so no file has to be opened to find it.
#[must_use]
pub fn find_log(dir: &Path, id: &str) -> Option<PathBuf> {
    let suffix = format!("_{id}.jsonl");
    log_files(dir).into_iter().find(|path| {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(&suffix))
    })
}

/// The entries around `entry` on the live branch of the log `id`, rendered
/// for the model: `before` and `after` entries on each side, each block cut
/// to `per_block` bytes.
///
/// # Errors
///
/// When no log has the id or the entry is not on its live branch.
pub fn open(
    dir: &Path,
    id: &str,
    entry: &str,
    before: usize,
    after: usize,
    per_block: usize,
) -> Result<String, String> {
    let path = find_log(dir, id).ok_or_else(|| format!("no session {id} in this project"))?;
    let (header, branch) = Session::read_branch(&path).map_err(|e| e.to_string())?;
    let at = branch
        .iter()
        .position(|e| e.id == entry)
        .ok_or_else(|| format!("entry {entry} is not on the live branch of session {id}"))?;
    let start = at.saturating_sub(before);
    let end = (at + after + 1).min(branch.len());
    let mut out = format!(
        "session {id} ({}), entries {}–{} of {}\n",
        header.cwd.display(),
        start + 1,
        end,
        branch.len()
    );
    for (index, e) in branch[start..end].iter().enumerate() {
        let marker = if start + index == at { "▶" } else { " " };
        let date = termide_agent_core::civil_date(e.timestamp);
        let mut blocks: Vec<(String, String)> = Vec::new();
        match &e.kind {
            EntryKind::Compaction { summary, .. } => {
                blocks.push(("compaction summary".into(), summary.clone()));
            }
            EntryKind::Message { message, .. } => match message {
                Message::User(user) => {
                    if let Some(command) = &user.ran {
                        blocks.push((
                            "user ran".into(),
                            format!("$ {command}\n{}", user.plain_text()),
                        ));
                    } else {
                        blocks.push(("user".into(), user.plain_text()));
                    }
                }
                Message::Assistant(reply) => {
                    for content in &reply.content {
                        match content {
                            AssistantContent::Text { text } => {
                                blocks.push(("assistant".into(), text.clone()))
                            }
                            AssistantContent::Thinking { text, .. } if !text.is_empty() => {
                                blocks.push(("thinking".into(), text.clone()));
                            }
                            AssistantContent::Thinking { .. } => {}
                            AssistantContent::ToolCall(call) => {
                                blocks.push((
                                    format!("call {}", call.name),
                                    call_text("", &call.arguments).trim().to_string(),
                                ));
                            }
                        }
                    }
                }
                Message::ToolResult(result) => {
                    let label = if result.is_error { "error" } else { "result" };
                    blocks.push((format!("{label} {}", result.tool_name), result.plain_text()));
                }
            },
            _ => continue,
        }
        for (label, text) in blocks {
            let cut = head(&text, per_block);
            let more = if cut.len() < text.len() {
                format!(" […{} more bytes]", text.len() - cut.len())
            } else {
                String::new()
            };
            out.push_str(&format!(
                "\n{marker} #{} · {date} · {label}\n{cut}{more}\n",
                e.id
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::text::query_words;
    use termide_agent_core::message::{
        AssistantMessage, StopReason, ToolCall, ToolResultMessage, Usage, UserMessage,
    };

    pub(crate) fn reply(content: Vec<AssistantContent>) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            provider: "p".into(),
            model: "m".into(),
            error_message: None,
            failure: None,
            timestamp: 0,
        })
    }

    pub(crate) fn text(text: &str) -> AssistantContent {
        AssistantContent::Text { text: text.into() }
    }

    fn query<'a>(terms_: &'a [Vec<Vec<String>>], exclude: Option<&'a str>) -> SessionQuery<'a> {
        SessionQuery {
            terms: terms_,
            patterns: &[],
            exclude,
            since: None,
            paths: None,
            project_root: None,
            now: termide_agent_core::message::now_millis(),
            deadline: None,
        }
    }

    #[test]
    fn the_cache_keeps_the_last_directories_and_a_busy_one_waits_no_longer_than_the_deadline() {
        let first = cache(Path::new("/cache-test/0"));
        assert!(Arc::ptr_eq(&first, &cache(Path::new("/cache-test/0"))));
        for n in 1..=CACHED_DIRS {
            cache(Path::new(&format!("/cache-test/{n}")));
        }
        assert!(!Arc::ptr_eq(&first, &cache(Path::new("/cache-test/0"))));

        let busy = Mutex::new(Cache::default());
        let _held = busy.lock().unwrap();
        let now = std::time::Instant::now();
        assert!(lock_until(&busy, Some(now), &CancelToken::new()).is_none());
    }

    #[test]
    fn the_live_branch_is_searched_and_dead_branches_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        let first = session
            .append_message(&Message::User(UserMessage::text("why drop tokio")))
            .unwrap();
        session
            .append_message(&reply(vec![text("undone zebra answer")]))
            .unwrap();
        session.rewind_to(Some(&first)).unwrap();
        session
            .append_message(&reply(vec![text("threads with mpsc instead of tokio")]))
            .unwrap();
        session
            .append_compaction("we chose std threads over tokio", 10, 1)
            .unwrap();

        let (found, _) = search(
            dir.path(),
            &query(&[query_words("tokio threads")], None),
            10,
            &CancelToken::new(),
        );
        assert!(!found.is_empty());
        assert_eq!(
            found[0].label, "compaction summary",
            "a summary counts most"
        );
        assert!(found.iter().all(|h| h
            .reference
            .starts_with(&format!("session:{}#", session.id()))));
        assert!(search(
            dir.path(),
            &query(&[query_words("zebra")], None),
            10,
            &CancelToken::new()
        )
        .0
        .is_empty());
        // Of the session in the context only what the compaction took out
        // is searched: not the reply it kept, nor its summary.
        let id = session.id().to_string();
        let (found, _) = search(
            dir.path(),
            &query(&[query_words("tokio")], Some(&id)),
            10,
            &CancelToken::new(),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].reference, format!("session:{id}#{first}"));
        // Without a compaction all of it is in the context.
        let mut fresh = Session::create(dir.path(), dir.path()).unwrap();
        fresh
            .append_message(&Message::User(UserMessage::text("tokio again")))
            .unwrap();
        let fresh_id = fresh.id().to_string();
        let (found, _) = search(
            dir.path(),
            &query(&[query_words("tokio again")], Some(&fresh_id)),
            10,
            &CancelToken::new(),
        );
        assert!(found.iter().all(|h| !h.reference.contains(&fresh_id)));
    }

    #[test]
    fn what_a_clean_plan_start_cleared_is_searched_and_what_it_kept_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        let read = ToolCall {
            id: "r1".into(),
            name: "read".into(),
            arguments: serde_json::json!({ "path": "src/cache.rs" }),
            extra_content: None,
        };
        session
            .append_message(&Message::User(UserMessage::text("plan the cache")))
            .unwrap();
        session
            .append_message(&reply(vec![
                text("looking"),
                AssistantContent::ToolCall(read.clone()),
            ]))
            .unwrap();
        let output = session
            .append_message(&Message::ToolResult(ToolResultMessage::text(
                &read,
                "fn evict_okapi() {}",
            )))
            .unwrap();
        session
            .append_message(&reply(vec![text("the plan: memoize the walrus")]))
            .unwrap();
        session.append_pruned().unwrap();
        session
            .append_message(&Message::User(UserMessage::text("do it")))
            .unwrap();

        let id = session.id().to_string();
        let search_own = |words: &str| {
            search(
                dir.path(),
                &query(&[query_words(words)], Some(&id)),
                10,
                &CancelToken::new(),
            )
            .0
        };
        let found = search_own("okapi");
        assert!(
            found
                .iter()
                .any(|hit| hit.reference == format!("session:{id}#{output}")),
            "{found:?}"
        );
        assert!(search_own("walrus").is_empty(), "the plan stays in context");
    }

    #[test]
    fn a_commit_call_links_its_sha_and_paths_narrow() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        let commit = ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": "git commit -m 'feat: walrus'" }),
            extra_content: None,
        };
        let edit = ToolCall {
            id: "c2".into(),
            name: "edit".into(),
            arguments: serde_json::json!({ "path": "src/walrus.rs", "old_string": "a", "new_string": "b" }),
            extra_content: None,
        };
        session
            .append_message(&reply(vec![
                AssistantContent::ToolCall(commit.clone()),
                AssistantContent::ToolCall(edit),
            ]))
            .unwrap();
        session
            .append_message(&Message::ToolResult(ToolResultMessage::text(
                &commit,
                "[main 08c2e3b1] feat: walrus\n 1 file changed",
            )))
            .unwrap();
        session
            .append_message(&reply(vec![text("the walrus module is in src/other.rs")]))
            .unwrap();

        let (found, _) = search(
            dir.path(),
            &query(&[query_words("walrus")], None),
            10,
            &CancelToken::new(),
        );
        assert!(found
            .iter()
            .any(|h| h.related.as_deref() == Some("commit 08c2e3b1")));

        let filter = PathFilter::new(&["src/walrus.rs".to_string()]);
        let walrus = [query_words("walrus")];
        let narrowed = SessionQuery {
            paths: Some(&filter),
            project_root: Some(dir.path()),
            ..query(&walrus, None)
        };
        let (found, _) = search(dir.path(), &narrowed, 10, &CancelToken::new());
        assert_eq!(found.len(), 1, "only the edit names the path: {found:?}");
        assert_eq!(found[0].label, "tool call");
    }

    #[test]
    fn open_shows_the_entries_around_a_hit() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("question one")))
            .unwrap();
        let middle = session
            .append_message(&reply(vec![text("answer one")]))
            .unwrap();
        session
            .append_message(&Message::User(UserMessage::text("question two")))
            .unwrap();
        let shown = open(dir.path(), session.id(), &middle, 1, 0, 100).unwrap();
        assert!(shown.contains("question one"));
        assert!(shown.contains("▶ #"));
        assert!(!shown.contains("question two"));
        assert!(open(dir.path(), "nope", &middle, 1, 1, 100).is_err());
        assert!(open(dir.path(), session.id(), "nope", 1, 1, 100).is_err());
    }
}
