//! The project's git history as search documents: commits whose message
//! has a query word (`log --grep`) and commits that added or removed a
//! queried identifier (`log -S`, the pickaxe), in every repository the
//! project holds.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use termide_agent_core::CancelToken;

use crate::filter::{PathFilter, Scope};
use crate::rank::{bm25, recency, Hit, Source, TermBag, Vocabulary};
use crate::text::terms;

/// How long one git command may run; a pickaxe over a long history is slow.
const GIT_TIMEOUT: Duration = Duration::from_secs(8);
/// Commits one message search takes, newest first.
const GREP_COMMITS: usize = 300;
/// Commits one pickaxe search takes.
const PICKAXE_COMMITS: usize = 60;
/// Identifiers the pickaxe looks for, at most: each is a full history walk.
const PICKAXE_IDENTIFIERS: usize = 3;
/// What a pickaxe hit adds to a commit's score, so a commit that changed a
/// queried identifier ranks even when its message does not name it.
const PICKAXE_BONUS: f64 = 2.0;
/// Changed files a hit lists.
const LISTED_FILES: usize = 6;

/// A repository the project holds, as the git panel finds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRoot {
    /// The working tree's root.
    pub root: PathBuf,
    /// The project's directory inside this repository, when the project is
    /// only part of it (a project in a dotfiles repository, say): its
    /// history is narrowed to that directory.
    pub pathspec: Option<PathBuf>,
    /// The repository's directory relative to the project root, `.` for the
    /// repository the project root is in; what references name.
    pub name: String,
}

impl RepoRoot {
    /// The repository at `root` as seen from `project_root`.
    #[must_use]
    pub fn new(root: PathBuf, project_root: &Path) -> Self {
        if let Ok(inside) = root.strip_prefix(project_root) {
            let name = inside.to_string_lossy().into_owned();
            return Self {
                root,
                pathspec: None,
                name: if name.is_empty() { ".".into() } else { name },
            };
        }
        let pathspec = project_root
            .strip_prefix(&root)
            .ok()
            .map(Path::to_path_buf)
            .filter(|p| !p.as_os_str().is_empty());
        Self {
            root,
            pathspec,
            name: ".".into(),
        }
    }

    /// The repository's directory inside the project, for a path filter:
    /// empty for the one the project is in.
    fn project_dir(&self) -> &str {
        if self.name == "." {
            ""
        } else {
            &self.name
        }
    }
}

/// Run `git -C dir args…` with no terminal prompt and no stdin, for at most
/// [`GIT_TIMEOUT`] and never past `deadline`; standard output on success.
///
/// # Errors
///
/// When git cannot start, fails, runs out of time or the run is cancelled.
pub fn run_git(
    dir: &Path,
    args: &[String],
    deadline: Instant,
    cancel: &CancelToken,
) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("git has no output")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = deadline.min(Instant::now() + GIT_TIMEOUT);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if cancel.is_cancelled() || Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(if cancel.is_cancelled() {
                    "cancelled".into()
                } else {
                    format!(
                        "git {} took too long",
                        args.first().map_or("", String::as_str)
                    )
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        return Err(err.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// One commit found.
#[derive(Debug, Clone)]
struct Commit {
    sha: String,
    time: u64,
    subject: String,
    body: String,
    files: Vec<String>,
    pickaxe: usize,
}

/// The `--format` the parser reads: a record separator, then the fields
/// separated by unit separators; `--name-only` follows the last one.
const FORMAT: &str = "--format=%x1e%H%x1f%ct%x1f%s%x1f%b%x1f";

fn parse_log(output: &str) -> Vec<Commit> {
    output
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.splitn(5, '\x1f');
            let sha = fields.next()?.trim().to_string();
            if sha.is_empty() {
                return None;
            }
            let time = fields.next()?.trim().parse::<u64>().ok()? * 1000;
            let subject = fields.next()?.to_string();
            let body = fields.next()?.trim().to_string();
            let files = fields
                .next()
                .unwrap_or("")
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            Some(Commit {
                sha,
                time,
                subject,
                body,
                files,
                pickaxe: 0,
            })
        })
        .collect()
}

/// What a git search looks for and is narrowed to.
#[derive(Debug, Clone, Copy)]
pub struct GitQuery<'a> {
    /// The terms of each phrasing of the query.
    pub terms: &'a [Vec<Vec<String>>],
    /// Substrings for `--grep`: the stems of the query words.
    pub patterns: &'a [String],
    /// Identifiers for the pickaxe.
    pub identifiers: &'a [String],
    pub since: Option<u64>,
    pub paths: &'a PathFilter,
    pub now: u64,
}

/// The pathspecs of `repo` for `paths`: `None` when the filter leaves the
/// repository out.
fn pathspecs(repo: &RepoRoot, paths: &PathFilter) -> Option<Vec<String>> {
    let base = repo.pathspec.clone().unwrap_or_default();
    match paths.within(repo.project_dir()) {
        Scope::Nothing => None,
        Scope::All => Some(if base.as_os_str().is_empty() {
            Vec::new()
        } else {
            vec![base.to_string_lossy().into_owned()]
        }),
        Scope::Some(specs) => Some(specs.iter().map(|s| s.pathspec(&base)).collect()),
    }
}

/// The commits of one repository the query finds.
fn find_commits(
    repo: &RepoRoot,
    query: &GitQuery<'_>,
    deadline: Instant,
    cancel: &CancelToken,
) -> Vec<Commit> {
    let Some(specs) = pathspecs(repo, query.paths) else {
        return Vec::new();
    };
    let common = |count: usize| {
        let mut args = vec![
            "log".to_string(),
            "--no-color".to_string(),
            "--name-only".to_string(),
            format!("-n{count}"),
            FORMAT.to_string(),
        ];
        if let Some(since) = query.since {
            args.push(format!("--since=@{}", since / 1000));
        }
        args
    };
    let finish = |mut args: Vec<String>| {
        args.push("--".into());
        args.extend(specs.iter().cloned());
        args
    };
    let mut commits: Vec<Commit> = Vec::new();
    if !query.patterns.is_empty() {
        let mut args = common(GREP_COMMITS);
        args.extend(["-i".to_string(), "-F".to_string()]);
        args.extend(query.patterns.iter().map(|p| format!("--grep={p}")));
        match run_git(&repo.root, &finish(args), deadline, cancel) {
            Ok(out) => commits.extend(parse_log(&out)),
            Err(e) => log::warn!("recall: git log in {}: {e}", repo.root.display()),
        }
    }
    for identifier in query.identifiers.iter().take(PICKAXE_IDENTIFIERS) {
        if cancel.is_cancelled() {
            break;
        }
        let mut args = common(PICKAXE_COMMITS);
        args.push(format!("-S{identifier}"));
        match run_git(&repo.root, &finish(args), deadline, cancel) {
            Ok(out) => {
                for mut found in parse_log(&out) {
                    match commits.iter_mut().find(|c| c.sha == found.sha) {
                        Some(known) => known.pickaxe += 1,
                        None => {
                            found.pickaxe = 1;
                            commits.push(found);
                        }
                    }
                }
            }
            Err(e) => log::warn!("recall: git pickaxe in {}: {e}", repo.root.display()),
        }
    }
    commits
}

/// The best commit hits across `repos`, best first, and whether the deadline
/// came before every repository was searched.
#[must_use]
pub fn search(
    repos: &[RepoRoot],
    query: &GitQuery<'_>,
    limit: usize,
    deadline: Instant,
    cancel: &CancelToken,
) -> (Vec<Hit>, bool) {
    let mut vocabulary = Vocabulary::default();
    let mut found: Vec<(&RepoRoot, Commit, TermBag)> = Vec::new();
    let mut cut_short = false;
    for repo in repos {
        if Instant::now() > deadline {
            cut_short = true;
            break;
        }
        for commit in find_commits(repo, query, deadline, cancel) {
            let text = format!(
                "{}\n{}\n{}",
                commit.subject,
                commit.body,
                commit.files.join("\n")
            );
            let bag = TermBag::new(&mut vocabulary, terms(&text));
            found.push((repo, commit, bag));
        }
    }
    let ids = vocabulary.phrases(query.terms);
    let bags: Vec<&TermBag> = found.iter().map(|(_, _, bag)| bag).collect();
    let scores = bm25(&bags, &ids);
    let mut ranked: Vec<(f64, &RepoRoot, &Commit)> = found
        .iter()
        .zip(scores)
        .map(|((repo, commit, _), score)| {
            let score =
                (score + PICKAXE_BONUS * commit.pickaxe as f64) * recency(commit.time, query.now);
            (score, *repo, commit)
        })
        .filter(|(score, _, _)| *score > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.truncate(limit);
    let hits = ranked
        .into_iter()
        .map(|(score, repo, commit)| {
            let mut snippet = crate::snippet(&commit.body, query.patterns);
            if !commit.files.is_empty() {
                let shown: Vec<&str> = commit
                    .files
                    .iter()
                    .take(LISTED_FILES)
                    .map(String::as_str)
                    .collect();
                let more = commit.files.len().saturating_sub(LISTED_FILES);
                if !snippet.is_empty() {
                    snippet.push('\n');
                }
                snippet.push_str(&format!("files: {}", shown.join(", ")));
                if more > 0 {
                    snippet.push_str(&format!(" (+{more})"));
                }
            }
            Hit {
                source: Source::Git,
                reference: format!(
                    "commit:{}@{}",
                    repo.name,
                    &commit.sha[..commit.sha.len().min(12)]
                ),
                timestamp: Some(commit.time),
                label: commit.subject.clone(),
                snippet,
                related: None,
                score,
            }
        })
        .collect();
    (hits, cut_short)
}

/// `git show --stat` of `sha` in the repository `name` (any repository the
/// project holds when `name` is empty or unknown).
///
/// # Errors
///
/// When no repository has the commit.
pub fn open(
    repos: &[RepoRoot],
    name: &str,
    sha: &str,
    cancel: &CancelToken,
) -> Result<String, String> {
    if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("`{sha}` is not a commit hash"));
    }
    let named: Vec<&RepoRoot> = repos.iter().filter(|r| r.name == name).collect();
    let candidates = if named.is_empty() {
        repos.iter().collect()
    } else {
        named
    };
    let args = [
        "show".to_string(),
        "--no-color".to_string(),
        "--stat".to_string(),
        "--format=fuller".to_string(),
        sha.to_string(),
        "--".to_string(),
    ];
    let deadline = Instant::now() + GIT_TIMEOUT;
    for repo in candidates {
        if let Ok(out) = run_git(&repo.root, &args, deadline, cancel) {
            return Ok(format!(
                "repository {} ({})\n{out}",
                repo.name,
                repo.root.display()
            ));
        }
    }
    Err(format!("no repository of the project has commit {sha}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::text::query_words;

    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    pub(crate) fn repo_with(dir: &Path, commits: &[(&str, &str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        for (file, content, message) in commits {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
            git(dir, &["add", "-A"]);
            git(dir, &["commit", "-q", "-m", message]);
        }
    }

    fn query<'a>(
        terms_: &'a [Vec<Vec<String>>],
        patterns: &'a [String],
        identifiers: &'a [String],
        paths: &'a PathFilter,
    ) -> GitQuery<'a> {
        GitQuery {
            terms: terms_,
            patterns,
            identifiers,
            since: None,
            paths,
            now: termide_agent_core::message::now_millis(),
        }
    }

    #[test]
    fn messages_and_the_pickaxe_find_commits_in_every_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        repo_with(
            &project.join("a"),
            &[
                ("lib.rs", "fn main() {}", "feat: add the walrus parser"),
                ("lib.rs", "fn split_command_line() {}", "refactor: tidy"),
            ],
        );
        repo_with(
            &project.join("b"),
            &[("x.md", "notes", "docs: walrus notes")],
        );
        let repos = vec![
            RepoRoot::new(project.join("a"), project),
            RepoRoot::new(project.join("b"), project),
        ];
        let words = vec!["walrus".to_string()];
        let none = PathFilter::default();
        let (hits, _) = search(
            &repos,
            &query(&[query_words("walrus")], &words, &[], &none),
            10,
            Instant::now() + GIT_TIMEOUT,
            &CancelToken::new(),
        );
        let refs: Vec<&str> = hits.iter().map(|h| h.reference.as_str()).collect();
        assert_eq!(hits.len(), 2, "{refs:?}");
        assert!(refs.iter().any(|r| r.starts_with("commit:a@")));
        assert!(refs.iter().any(|r| r.starts_with("commit:b@")));

        let ids = vec!["split_command_line".to_string()];
        let (hits, _) = search(
            &repos,
            &query(&[query_words("split_command_line")], &[], &ids, &none),
            10,
            Instant::now() + GIT_TIMEOUT,
            &CancelToken::new(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "refactor: tidy");

        // A path filter keeps the other repository out.
        let only_b = PathFilter::new(&["b".to_string()]);
        let (hits, _) = search(
            &repos,
            &query(&[query_words("walrus")], &words, &[], &only_b),
            10,
            Instant::now() + GIT_TIMEOUT,
            &CancelToken::new(),
        );
        assert_eq!(hits.len(), 1);
        assert!(hits[0].reference.starts_with("commit:b@"));

        let sha = hits[0].reference.rsplit('@').next().unwrap().to_string();
        let shown = open(&repos, "b", &sha, &CancelToken::new()).unwrap();
        assert!(shown.contains("docs: walrus notes"));
        assert!(open(&repos, "b", "zz", &CancelToken::new()).is_err());
    }

    #[test]
    fn a_project_inside_a_repository_sees_only_its_own_history() {
        let tmp = tempfile::tempdir().unwrap();
        repo_with(
            tmp.path(),
            &[
                ("proj/a.rs", "x", "feat: walrus inside"),
                ("other/b.rs", "y", "feat: walrus outside"),
            ],
        );
        let project = tmp.path().join("proj");
        let repo = RepoRoot::new(tmp.path().to_path_buf(), &project);
        assert_eq!(repo.pathspec.as_deref(), Some(Path::new("proj")));
        assert_eq!(repo.name, ".");
        let words = vec!["walrus".to_string()];
        let none = PathFilter::default();
        let (hits, _) = search(
            &[repo],
            &query(&[query_words("walrus")], &words, &[], &none),
            10,
            Instant::now() + GIT_TIMEOUT,
            &CancelToken::new(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "feat: walrus inside");
    }
}
