//! A commit log read on demand from one running `git log`.
//!
//! The panel shows the first screen at once and asks for more as the user
//! scrolls toward the end of what was read. One `git log` process serves the
//! whole view: a reader thread takes as many commits from its output as were
//! asked for and then stops reading, so git blocks on the full pipe until the
//! next request instead of walking history nobody looks at. Unlike paging with
//! `--skip`, history is walked once, and git's own ASCII `--graph` continues
//! across pieces; the box-drawing graph carries its [`GraphLayout`] from one
//! piece to the next. Dropping the stream kills the process.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use crate::command::{git_command_stdout, hardened_git};
use crate::commits::CommitInfo;
use crate::graph::{GraphCommit, GraphLayout};

/// What the reader thread sends back.
#[derive(Debug)]
pub enum LogChunk {
    /// The next rows, in order, holding `commits` commits (a graph's
    /// connector rows come on top); `done` when the history ended with them.
    Rows {
        rows: Vec<CommitInfo>,
        commits: usize,
        done: bool,
    },
    /// How many commits the history holds in all, counted apart so the first
    /// rows need not wait for it.
    Total(usize),
    /// `git log` failed (a branch that is gone, say): git's own words.
    /// The last `Rows`, with `done`, follows it.
    Failed(String),
}

/// A running `git log` and the thread reading it.
pub struct LogStream {
    requests: Sender<usize>,
    chunks: Receiver<LogChunk>,
    child: Arc<Mutex<Option<Child>>>,
}

impl LogStream {
    /// Start reading the history of `branch` (`None` = HEAD) in `repo`, drawn
    /// with the box-drawing graph when `unicode_graph`, else with git's ASCII
    /// `--graph`. Nothing is read until [`LogStream::request`].
    #[must_use]
    pub fn start(repo: &Path, branch: Option<&str>, unicode_graph: bool) -> Self {
        let (requests, wanted) = mpsc::channel::<usize>();
        let (sender, chunks) = mpsc::channel();
        let child = Arc::new(Mutex::new(None));

        let format = if unicode_graph {
            // Trailing %p adds the space-separated parent hashes for the layout engine.
            "--format=%h\t%an\t%ar\t%d\t%s\t%p"
        } else {
            "--format=%h\t%an\t%ar\t%d\t%s"
        };
        let mut args = vec!["log", format];
        if !unicode_graph {
            args.push("--graph");
        }
        if let Some(branch) = branch {
            args.push(branch);
        }
        // A branch named like a file or folder (`docs`) is a revision, not
        // a path to filter by.
        args.push("--");
        let spawned = hardened_git(repo, &args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let (stdout, stderr) = match spawned {
            Ok(mut process) => {
                let stdout = process.stdout.take();
                let stderr = process.stderr.take();
                *child.lock().unwrap_or_else(PoisonError::into_inner) = Some(process);
                (stdout, stderr)
            }
            Err(error) => {
                log::warn!("cannot run git log: {error}");
                (None, None)
            }
        };
        // Read apart so a chatty stderr cannot fill its pipe and stall git.
        let stderr = std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(mut stderr) = stderr {
                let _ = std::io::Read::read_to_end(&mut stderr, &mut text);
            }
            String::from_utf8_lossy(&text).trim().to_string()
        });

        let revision = branch.unwrap_or("HEAD").to_string();
        let count_repo = repo.to_path_buf();
        let count_sender = sender.clone();
        std::thread::spawn(move || {
            let total = git_command_stdout(&count_repo, &["rev-list", "--count", &revision, "--"])
                .and_then(|out| out.trim().parse().ok());
            if let Some(total) = total {
                let _ = count_sender.send(LogChunk::Total(total));
            }
        });

        let reaper = Arc::clone(&child);
        let head_repo = (branch.is_none()).then(|| repo.to_path_buf());
        std::thread::spawn(move || {
            let mut reader = stdout.map(BufReader::new);
            let mut parser = Parser::new(unicode_graph);
            // A request ends when the stream is dropped: the sender goes away.
            while let Ok(wanted) = wanted.recv() {
                let (rows, commits, done) = match reader.as_mut() {
                    Some(reader) => parser.read(reader, wanted),
                    None => (Vec::new(), 0, true),
                };
                if done {
                    if let Some(error) = failure(&reaper, stderr, head_repo.as_deref()) {
                        let _ = sender.send(LogChunk::Failed(error));
                    }
                    let _ = sender.send(LogChunk::Rows {
                        rows,
                        commits,
                        done,
                    });
                    break;
                }
                if sender
                    .send(LogChunk::Rows {
                        rows,
                        commits,
                        done,
                    })
                    .is_err()
                {
                    break;
                }
            }
            kill(&reaper);
        });

        Self {
            requests,
            chunks,
            child,
        }
    }

    /// Ask for the next `commits` commits; they arrive as one
    /// [`LogChunk::Rows`]. Asking again before it came queues another piece.
    pub fn request(&self, commits: usize) {
        let _ = self.requests.send(commits.max(1));
    }

    /// What the reader sent since the last call, without waiting.
    #[must_use]
    pub fn try_recv(&self) -> Option<LogChunk> {
        self.chunks.try_recv().ok()
    }
}

impl Drop for LogStream {
    fn drop(&mut self) {
        // The reader may be blocked on a slow git; killing the process ends
        // its read with EOF, and the closed request channel ends its loop.
        kill(&self.child);
    }
}

/// Why `git log`, whose output just ended, failed; `None` when it did not,
/// when the stream killed it, or when the HEAD of `head_repo` (the log of
/// HEAD) has no commit yet, which is an empty history rather than an error.
fn failure(
    child: &Mutex<Option<Child>>,
    stderr: std::thread::JoinHandle<String>,
    head_repo: Option<&Path>,
) -> Option<String> {
    let mut process = child
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()?;
    let status = process.wait().ok()?;
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        return None;
    }
    if let Some(repo) = head_repo {
        // No commit on HEAD yet: nothing to show, nothing went wrong.
        git_command_stdout(repo, &["rev-parse", "--verify", "-q", "HEAD"])?;
    }
    Some(match stderr.lines().next() {
        Some(line) => line.to_string(),
        None => format!("git log: {status}"),
    })
}

fn kill(child: &Mutex<Option<Child>>) {
    if let Some(mut process) = child.lock().unwrap_or_else(PoisonError::into_inner).take() {
        let _ = process.kill();
        let _ = process.wait();
    }
}

/// Turns `git log` lines into rows, keeping the graph's layout across calls.
struct Parser {
    unicode_graph: bool,
    layout: GraphLayout,
}

impl Parser {
    fn new(unicode_graph: bool) -> Self {
        Self {
            unicode_graph,
            layout: GraphLayout::default(),
        }
    }

    /// Read lines until `wanted` commits are in, or the output ends; the rows
    /// read, how many commits they hold, and whether the output ended.
    fn read(&mut self, reader: &mut impl BufRead, wanted: usize) -> (Vec<CommitInfo>, usize, bool) {
        let mut rows = Vec::new();
        let mut commits = 0;
        let mut line = Vec::new();
        while commits < wanted {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return (rows, commits, true),
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\n', '\r']);
            commits += if self.unicode_graph {
                self.push_unicode(text, &mut rows)
            } else {
                push_ascii(text, &mut rows)
            };
        }
        (rows, commits, false)
    }

    /// Lay out one `%h %an %ar %d %s %p` line; how many commits it added.
    fn push_unicode(&mut self, line: &str, rows: &mut Vec<CommitInfo>) -> usize {
        let parts: Vec<&str> = line.splitn(6, '\t').collect();
        if parts.len() != 6 {
            return 0;
        }
        let commit = commit_info(&parts, None);
        let graph_commit = GraphCommit {
            hash: parts[0].to_string(),
            parents: parts[5].split_whitespace().map(str::to_string).collect(),
        };
        let mut laid = Vec::new();
        self.layout.push(&graph_commit, 0, &mut laid);
        for row in laid {
            rows.push(match row.commit {
                Some(_) => CommitInfo {
                    graph: Some(row.graph),
                    ..commit.clone()
                },
                None => connector(row.graph),
            });
        }
        1
    }
}

/// Parse one line of `git log --graph` in ASCII; how many commits it added.
/// A line with no commit on it is a graph-only row (a merge's diagonals).
fn push_ascii(line: &str, rows: &mut Vec<CommitInfo>) -> usize {
    // Graph lines start with *, |, /, \ or space; the commit info starts at
    // the first hex digit, the hash.
    let graph_end = line.find(|c: char| c.is_ascii_hexdigit()).unwrap_or(0);
    let graph = (graph_end > 0).then(|| line[..graph_end].to_string());
    let info = &line[graph_end..];
    let parts: Vec<&str> = info.splitn(5, '\t').collect();
    if parts.len() == 5 {
        rows.push(commit_info(&parts, graph));
        1
    } else {
        if !info.trim().is_empty() || graph.is_some() {
            rows.push(connector(graph.unwrap_or_else(|| info.to_string())));
        }
        0
    }
}

/// A commit from the `%h %an %ar %d %s` fields of a line.
fn commit_info(parts: &[&str], graph: Option<String>) -> CommitInfo {
    let refs = parts[3].trim();
    CommitInfo {
        hash: parts[0].to_string(),
        author: parts[1].to_string(),
        date: parts[2].to_string(),
        message: parts[4].to_string(),
        graph,
        refs: (!refs.is_empty()).then(|| refs.to_string()),
    }
}

/// A row carrying only a piece of graph.
fn connector(graph: String) -> CommitInfo {
    CommitInfo {
        hash: String::new(),
        author: String::new(),
        date: String::new(),
        message: String::new(),
        graph: Some(graph),
        refs: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A repository with `n` commits on `main`, a side branch merged back in
    /// the middle so the graph has connector rows.
    fn repo(n: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "-q", "-b", "main"]);
        for i in 0..n {
            git(
                path,
                &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")],
            );
            if i == n / 2 {
                git(path, &["checkout", "-q", "-b", "side"]);
                git(path, &["commit", "-q", "--allow-empty", "-m", "side"]);
                git(path, &["checkout", "-q", "main"]);
                git(path, &["merge", "-q", "--no-ff", "-m", "merge", "side"]);
            }
        }
        dir
    }

    fn next(stream: &LogStream) -> LogChunk {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(chunk) = stream.try_recv() {
                return chunk;
            }
            assert!(Instant::now() < deadline, "no chunk arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn next_rows(stream: &LogStream) -> (Vec<CommitInfo>, usize, bool) {
        loop {
            if let LogChunk::Rows {
                rows,
                commits,
                done,
            } = next(stream)
            {
                return (rows, commits, done);
            }
        }
    }

    fn graphs(rows: &[CommitInfo]) -> Vec<String> {
        rows.iter()
            .map(|row| format!("{} {}", row.graph.as_deref().unwrap_or(""), row.message))
            .collect()
    }

    #[test]
    fn pieces_join_into_the_history_read_at_once() {
        let dir = repo(12);
        for unicode in [true, false] {
            let whole = LogStream::start(dir.path(), None, unicode);
            whole.request(1000);
            let (all, total, done) = next_rows(&whole);
            // Twelve commits, the side branch and its merge.
            assert_eq!(total, 14);
            assert!(done);

            let pieces = LogStream::start(dir.path(), None, unicode);
            let mut joined = Vec::new();
            let mut read = 0;
            loop {
                pieces.request(3);
                let (rows, commits, done) = next_rows(&pieces);
                assert!(commits <= 3);
                read += commits;
                joined.extend(rows);
                if done {
                    break;
                }
            }
            assert_eq!(read, 14);
            assert_eq!(graphs(&joined), graphs(&all), "unicode: {unicode}");
            assert!(
                all.iter().any(|row| row.hash.is_empty()),
                "the merge draws a connector row"
            );
        }
    }

    #[test]
    fn the_total_is_counted_apart() {
        let dir = repo(4);
        let stream = LogStream::start(dir.path(), Some("main"), true);
        let total = loop {
            if let LogChunk::Total(total) = next(&stream) {
                break total;
            }
        };
        assert_eq!(total, 6);
    }

    #[test]
    fn a_repository_with_no_history_ends_at_once() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let stream = LogStream::start(dir.path(), None, true);
        stream.request(10);
        // An empty history, not a failure.
        let chunk = next(&stream);
        assert!(
            matches!(&chunk, LogChunk::Rows { rows, commits: 0, done: true } if rows.is_empty()),
            "{chunk:?}"
        );
    }

    #[test]
    fn a_branch_named_like_a_folder_shows_its_history() {
        let dir = repo(2);
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/a.md"), "a").unwrap();
        git(dir.path(), &["branch", "docs"]);
        for unicode in [true, false] {
            let stream = LogStream::start(dir.path(), Some("docs"), unicode);
            stream.request(100);
            let (mut rows, mut total) = (None, None);
            while rows.is_none() || total.is_none() {
                match next(&stream) {
                    LogChunk::Rows { commits, done, .. } => rows = Some((commits, done)),
                    LogChunk::Total(count) => total = Some(count),
                    LogChunk::Failed(error) => panic!("{error}"),
                }
            }
            assert_eq!(rows, Some((4, true)), "unicode: {unicode}");
            assert_eq!(total, Some(4));
        }
    }

    #[test]
    fn a_failing_log_says_why() {
        let dir = repo(2);
        let stream = LogStream::start(dir.path(), Some("no-such-branch"), true);
        stream.request(10);
        let error = loop {
            match next(&stream) {
                LogChunk::Failed(error) => break error,
                LogChunk::Rows { .. } => panic!("rows came without the failure"),
                LogChunk::Total(_) => {}
            }
        };
        assert!(!error.is_empty());
        let (rows, _, done) = next_rows(&stream);
        assert!(rows.is_empty() && done);
    }
}
