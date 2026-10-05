//! `recall`: search what the project already knows — its earlier agent
//! sessions, its git history and its files — and hand back ranked, cited
//! results, or, with the solver on, an answer drawn from them.
//!
//! Search is deterministic: every source ranks its own documents by BM25
//! over stemmed terms ([`text`]), session and commit hits lose weight with
//! age, and the sources are merged by reciprocal rank fusion. The one
//! optional model call, the solver, only reads what the search found. The
//! tool reads and never writes, so the permission rules let it through in
//! every mode.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::message::now_millis;
use termide_agent_core::{
    civil_date, one_shot, CancelToken, ModelSpec, Provider, RecallPrompt, SessionView, Tool,
    ToolCall, ToolContext, ToolResultMessage, ToolText, ToolUpdate,
};

mod files;
mod filter;
mod git;
mod rank;
mod sessions;
mod text;

pub use git::RepoRoot;
pub use rank::{Hit, Source};

use filter::{parse_date, PathFilter};

/// How long a source may search unless the settings say otherwise.
pub const DEFAULT_TIME_LIMIT: Duration = Duration::from_secs(60);

/// How long each source may search. The sources search side by side, each
/// against its own limit, so a source a project makes slow (files in a huge
/// tree) does not take the others' time; a search lasts as long as its
/// slowest source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeLimits {
    pub sessions: Duration,
    pub git: Duration,
    pub files: Duration,
}

impl Default for TimeLimits {
    fn default() -> Self {
        Self {
            sessions: DEFAULT_TIME_LIMIT,
            git: DEFAULT_TIME_LIMIT,
            files: DEFAULT_TIME_LIMIT,
        }
    }
}
/// Results a search returns unless asked for another number.
const DEFAULT_LIMIT: usize = 10;
/// Results a search returns at most.
const MAX_LIMIT: usize = 30;
/// Candidates each source hands to the fusion, per result asked for.
const CANDIDATES_PER_RESULT: usize = 2;
/// Characters of context a snippet shows around its first match.
const SNIPPET_CHARS: usize = 320;
/// Bytes of output a call returns at most; whole results are dropped past it.
const MAX_OUTPUT_BYTES: usize = 16 * 1024;
/// Entries `open` shows on each side of a session hit.
const OPEN_CONTEXT_ENTRIES: usize = 3;
/// Bytes `open` shows of each block.
const OPEN_BLOCK_BYTES: usize = 2 * 1024;

/// The model call that answers from the results, when it is on.
#[derive(Clone)]
pub struct Solver {
    pub prompt: RecallPrompt,
    /// The model that answers; `None` uses the model the session runs on.
    pub model: Option<(Arc<dyn Provider>, ModelSpec)>,
}

impl std::fmt::Debug for Solver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Solver")
            .field("model", &self.model.as_ref().map(|(_, m)| &m.id))
            .finish_non_exhaustive()
    }
}

/// Finds the roots of the repositories a project holds, as the git panels
/// do, at every search: a repository created after the panel opened, or a
/// first commit made since, is searched as well.
#[derive(Clone)]
pub struct RepoFinder(Arc<FindRepos>);

/// What a [`RepoFinder`] calls: the project root in, the repository roots out.
type FindRepos = dyn Fn(&Path) -> Vec<PathBuf> + Send + Sync;

impl RepoFinder {
    pub fn new(find: impl Fn(&Path) -> Vec<PathBuf> + Send + Sync + 'static) -> Self {
        Self(Arc::new(find))
    }

    /// The repositories of the project at `project_root`.
    fn repos(&self, project_root: &Path) -> Vec<RepoRoot> {
        (self.0)(project_root)
            .into_iter()
            .map(|root| RepoRoot::new(root, project_root))
            .collect()
    }
}

impl std::fmt::Debug for RepoFinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RepoFinder")
    }
}

/// The panel's directory when it lies outside the project (the panel was
/// moved there): searched as well, its files, repositories and session logs.
#[derive(Debug, Clone)]
pub struct PanelDir {
    pub dir: PathBuf,
    /// The panel's session logs there: `<config>/ai/sessions/<dir key>/`.
    pub sessions_dir: Option<PathBuf>,
}

/// Where the tool looks.
#[derive(Debug, Clone)]
pub struct RecallSetup {
    pub project_root: PathBuf,
    /// The project's session logs: `<config>/ai/sessions/<project key>/`.
    pub sessions_dir: Option<PathBuf>,
    /// The repositories the project holds.
    pub find_repos: RepoFinder,
    /// The panel's directory, when it is not inside the project.
    pub panel_dir: Option<PanelDir>,
    pub solver: Option<Solver>,
    /// How long each source may search; what a source found by its limit is
    /// ranked, and the result says which sources it stopped.
    pub time_limits: TimeLimits,
}

/// The sources a search may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sources {
    pub sessions: bool,
    pub git: bool,
    pub files: bool,
}

impl Sources {
    const ALL: Self = Self {
        sessions: true,
        git: true,
        files: true,
    };

    fn parse(value: &Value) -> Result<Self, String> {
        let single;
        let items = match value {
            Value::Array(items) => items.as_slice(),
            Value::String(_) => {
                single = [value.clone()];
                &single[..]
            }
            _ => return Ok(Self::ALL),
        };
        if items.is_empty() {
            return Ok(Self::ALL);
        }
        let mut sources = Self {
            sessions: false,
            git: false,
            files: false,
        };
        for item in items {
            match item.as_str() {
                Some("sessions") => sources.sessions = true,
                Some("git") => sources.git = true,
                Some("files") => sources.files = true,
                other => {
                    return Err(format!(
                        "unknown source {}: use sessions, git or files",
                        other.unwrap_or("(not a string)")
                    ))
                }
            }
        }
        Ok(sources)
    }

    fn names(self) -> Vec<&'static str> {
        [
            (self.sessions, "sessions"),
            (self.git, "git"),
            (self.files, "files"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect()
    }
}

/// A search request, from a tool call or the command line.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    pub queries: Vec<String>,
    pub sources: Sources,
    pub paths: Vec<String>,
    pub since: Option<u64>,
    pub limit: usize,
    /// Return the results as found even with the solver on.
    pub raw: bool,
    /// The directory relative `paths` and the file results are relative to;
    /// the project root when `None`.
    pub cwd: Option<PathBuf>,
}

impl SearchRequest {
    /// A request for `query` over every source, as the command line makes.
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            queries: vec![query.into()],
            sources: Sources::ALL,
            paths: Vec::new(),
            since: None,
            limit: DEFAULT_LIMIT,
            raw: false,
            cwd: None,
        }
    }
}

/// What a search found, and the solver's answer when it ran.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub request: SearchRequest,
    pub hits: Vec<Hit>,
    /// `Some(Ok)` with the answer, `Some(Err)` when the solver failed (the
    /// hits then stand alone), `None` when it did not run.
    pub answer: Option<Result<String, String>>,
    /// Sources their time limit stopped before they were searched through,
    /// with that limit.
    pub incomplete: Vec<(&'static str, Duration)>,
}

/// A window of `text` around the first match of a pattern, whitespace
/// collapsed, `…` where it is cut.
pub(crate) fn snippet(text: &str, patterns: &[String]) -> String {
    let collapsed: Vec<char> = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect();
    if collapsed.len() <= SNIPPET_CHARS {
        return collapsed.into_iter().collect();
    }
    let lower: Vec<char> = collapsed
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let first = patterns
        .iter()
        .filter_map(|pattern| {
            let needle: Vec<char> = pattern.chars().collect();
            (!needle.is_empty())
                .then(|| {
                    lower
                        .windows(needle.len())
                        .position(|w| w == needle.as_slice())
                })
                .flatten()
        })
        .min()
        .unwrap_or(0);
    let start = first.saturating_sub(SNIPPET_CHARS / 4);
    let end = (start + SNIPPET_CHARS).min(collapsed.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&collapsed[start..end]);
    if end < collapsed.len() {
        out.push('…');
    }
    out
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::Session => "session",
        Source::Git => "commit",
        Source::File => "file",
    }
}

/// The hits as the model reads them, numbered, within the output budget.
fn render_hits(hits: &[Hit]) -> String {
    let mut out = String::new();
    for (index, hit) in hits.iter().enumerate() {
        let mut item = format!("{}. {}", index + 1, hit.reference);
        if let Some(at) = hit.timestamp {
            item.push_str(&format!(" · {}", civil_date(at)));
        }
        // The reference names the source, and a file hit's path: a file hit
        // shows only the headings of its section, when it has them.
        let label = match hit.source {
            Source::File => hit
                .label
                .split_once(" › ")
                .map_or("", |(_, headings)| headings),
            _ => hit.label.as_str(),
        };
        if !label.is_empty() {
            item.push_str(&format!(" · {label}"));
        }
        item.push('\n');
        for line in hit.snippet.lines().filter(|l| !l.trim().is_empty()) {
            item.push_str(&format!("   {line}\n"));
        }
        if let Some(related) = &hit.related {
            item.push_str(&format!("   → {related}\n"));
        }
        if out.len() + item.len() > MAX_OUTPUT_BYTES {
            out.push_str(&format!("[{} more results cut]\n", hits.len() - index));
            break;
        }
        out.push_str(&item);
    }
    out
}

/// The outcome as the tool's text: the answer and its sources, or the hits.
#[must_use]
pub fn render(outcome: &Outcome) -> String {
    let mut text = render_found(outcome);
    if !outcome.incomplete.is_empty() {
        let stopped: Vec<String> = outcome
            .incomplete
            .iter()
            .map(|(source, limit)| format!("{source} at its {} s limit", limit.as_secs()))
            .collect();
        text.push_str(&format!(
            "\n(Not searched through: {}. Narrow the search with `paths` to get further.)",
            stopped.join(", ")
        ));
    }
    text
}

fn render_found(outcome: &Outcome) -> String {
    let queries = outcome.request.queries.join(" | ");
    let searched = outcome.request.sources.names().join(", ");
    if outcome.hits.is_empty() {
        return format!(
            "No results for \"{queries}\" in {searched}. Try other words, the request in another \
             language, or the identifiers and file names it would involve."
        );
    }
    let hits = render_hits(&outcome.hits);
    match &outcome.answer {
        Some(Ok(answer)) => format!(
            "{}\n\nFrom {} results for \"{queries}\" in {searched} (`open` a reference to check):\n{hits}",
            answer.trim(),
            outcome.hits.len()
        ),
        Some(Err(error)) => format!(
            "(The solver could not answer: {error}.)\n{} results for \"{queries}\" in {searched}:\n{hits}",
            outcome.hits.len()
        ),
        None => format!(
            "{} results for \"{queries}\" in {searched}:\n{hits}",
            outcome.hits.len()
        ),
    }
}

/// The outcome as JSON, for `--recall --output json`.
#[must_use]
pub fn to_json(outcome: &Outcome) -> Value {
    let hits: Vec<Value> = outcome
        .hits
        .iter()
        .map(|hit| {
            json!({
                "reference": hit.reference,
                "source": source_name(hit.source),
                "date": hit.timestamp.map(civil_date),
                "label": hit.label,
                "snippet": hit.snippet,
                "related": hit.related,
                "score": hit.score,
            })
        })
        .collect();
    json!({
        "queries": outcome.request.queries,
        "sources": outcome.request.sources.names(),
        "answer": outcome.answer.as_ref().and_then(|a| a.as_ref().ok()),
        "solver_error": outcome.answer.as_ref().and_then(|a| a.as_ref().err()),
        "hits": hits,
        "incomplete": outcome.incomplete.iter().map(|(source, _)| *source).collect::<Vec<_>>(),
    })
}

/// The `recall` tool.
#[derive(Debug, Clone)]
pub struct RecallTool {
    setup: RecallSetup,
}

impl RecallTool {
    #[must_use]
    pub fn new(setup: RecallSetup) -> Self {
        Self { setup }
    }

    /// Run `request`; `session` is the calling session, whose log is left
    /// out and whose model answers when the solver names none.
    #[must_use]
    pub fn search(
        &self,
        request: SearchRequest,
        session: Option<&SessionView>,
        cancel: &CancelToken,
    ) -> Outcome {
        let now = now_millis();
        let started = Instant::now();
        let limits = self.setup.time_limits;
        // Each phrasing stays apart: they are alternatives (see `rank::bm25`).
        let term_list: Vec<Vec<Vec<String>>> = request
            .queries
            .iter()
            .map(|q| text::query_words(q))
            .collect();
        let patterns = text::scan_patterns(&request.queries);
        let words = text::acronyms(&request.queries);
        let identifiers = text::identifiers(&request.queries);
        let repos = self.repos();
        let project_root = &self.setup.project_root;
        let cwd = request.cwd.as_deref().unwrap_or(project_root);
        let paths = PathFilter::new(&filter::project_paths(&request.paths, cwd, project_root));
        let candidates = request.limit * CANDIDATES_PER_RESULT;
        // The whole `sessions/` tree, checkpoint copies included, is left out
        // of the file walk: the logs are searched as sessions, and read as
        // files they only repeat it.
        let excluded: Vec<PathBuf> = self
            .setup
            .sessions_dir
            .iter()
            .flat_map(|dir| dir.ancestors())
            .find(|dir| {
                dir.file_name()
                    .is_some_and(|n| n == termide_agent_core::SESSIONS_DIR)
            })
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        let exclude = session.and_then(|s| s.id.as_deref());
        // The sources search side by side, each against its own deadline.
        type Found = Option<(Vec<Hit>, bool)>;
        let (from_sessions, from_git, from_files): (Found, Found, Found) =
            std::thread::scope(|scope| {
                let sessions = scope.spawn(|| {
                    let dirs: Vec<&PathBuf> = self
                        .setup
                        .sessions_dir
                        .iter()
                        .chain(
                            self.setup
                                .panel_dir
                                .iter()
                                .filter_map(|p| p.sessions_dir.as_ref()),
                        )
                        .collect();
                    if !request.sources.sessions || dirs.is_empty() {
                        return None;
                    }
                    let query = sessions::SessionQuery {
                        terms: &term_list,
                        patterns: &patterns,
                        exclude,
                        since: request.since,
                        paths: Some(&paths),
                        project_root: Some(&self.setup.project_root),
                        now,
                        deadline: Some(started + limits.sessions),
                    };
                    // Each directory ranks its own logs; the better of the
                    // two at each place goes first.
                    let mut found: Vec<Hit> = Vec::new();
                    let mut cut_short = false;
                    for dir in dirs {
                        let (hits, cut) = sessions::search(dir, &query, candidates, cancel);
                        found.extend(hits);
                        cut_short |= cut;
                    }
                    found.sort_by(|a, b| b.score.total_cmp(&a.score));
                    found.truncate(candidates);
                    Some((found, cut_short))
                });
                let git = scope.spawn(|| {
                    if !request.sources.git {
                        return None;
                    }
                    let query = git::GitQuery {
                        terms: &term_list,
                        patterns: &patterns,
                        words: &words,
                        identifiers: &identifiers,
                        since: request.since,
                        paths: &paths,
                        now,
                    };
                    Some(git::search(
                        &repos,
                        &query,
                        candidates,
                        started + limits.git,
                        cancel,
                    ))
                });
                let files = scope.spawn(|| {
                    if !request.sources.files {
                        return None;
                    }
                    let query = files::FileQuery {
                        terms: &term_list,
                        patterns: &patterns,
                        words: &words,
                        paths: &paths,
                        excluded: &excluded,
                        now,
                    };
                    Some(files::search(
                        &self.setup.project_root,
                        self.setup.panel_dir.as_ref().map(|p| p.dir.as_path()),
                        &repos,
                        &query,
                        candidates,
                        started + limits.files,
                        cancel,
                    ))
                });
                let join = |handle: std::thread::ScopedJoinHandle<'_, Found>| {
                    handle.join().unwrap_or_else(|_| {
                        log::warn!("recall: a source's search panicked");
                        None
                    })
                };
                (join(sessions), join(git), join(files))
            });
        let mut lists = Vec::new();
        let mut incomplete = Vec::new();
        // A fixed order, so fusion breaks ties the same way every time.
        for (name, limit, found) in [
            ("sessions", limits.sessions, from_sessions),
            ("git", limits.git, from_git),
            ("files", limits.files, from_files),
        ] {
            if let Some((hits, cut_short)) = found {
                if cut_short {
                    incomplete.push((name, limit));
                }
                lists.push(hits);
            }
        }
        let mut hits = rank::fuse(lists, request.limit);
        for hit in hits.iter_mut().filter(|hit| hit.source == Source::File) {
            let Some((path, line)) = hit
                .reference
                .strip_prefix("file:")
                .and_then(|rest| rest.rsplit_once(':'))
            else {
                continue;
            };
            let shown = filter::path_from_cwd(path, cwd, project_root);
            if let Some(rest) = hit.label.strip_prefix(path) {
                hit.label = format!("{shown}{rest}");
            }
            hit.reference = format!("file:{shown}:{line}");
        }
        let answer = self.solve(&request, &hits, session, cancel);
        Outcome {
            request,
            hits,
            answer,
            incomplete,
        }
    }

    /// The project's repositories, and those of the panel's directory when
    /// it lies elsewhere, each once.
    fn repos(&self) -> Vec<RepoRoot> {
        let finder = &self.setup.find_repos;
        let mut repos = finder.repos(&self.setup.project_root);
        if let Some(panel) = &self.setup.panel_dir {
            for root in (finder.0)(&panel.dir) {
                if !repos.iter().any(|repo| repo.root == root) {
                    repos.push(RepoRoot::new(root, &self.setup.project_root));
                }
            }
        }
        repos
    }

    fn solve(
        &self,
        request: &SearchRequest,
        hits: &[Hit],
        session: Option<&SessionView>,
        cancel: &CancelToken,
    ) -> Option<Result<String, String>> {
        let solver = self.setup.solver.as_ref()?;
        if request.raw || hits.is_empty() {
            return None;
        }
        let (provider, model) = match (&solver.model, session) {
            (Some((provider, model)), _) => (Arc::clone(provider), model.clone()),
            (None, Some(session)) => (Arc::clone(&session.provider), session.model.clone()),
            (None, None) => return Some(Err("no model to answer with".into())),
        };
        let question = request.queries.join(" / ");
        let user = solver.prompt.user_turn(&question, &render_hits(hits));
        Some(one_shot(
            provider.as_ref(),
            &model,
            &solver.prompt.instructions,
            &user,
            cancel,
        ))
    }

    /// `open`: the context of one reference.
    ///
    /// # Errors
    ///
    /// When the reference is malformed or names nothing in the project.
    pub fn open(&self, reference: &str, cancel: &CancelToken) -> Result<String, String> {
        if let Some(rest) = reference.strip_prefix("session:") {
            let (id, entry) = rest
                .split_once('#')
                .ok_or("a session reference is session:<id>#<entry>")?;
            let dir = self
                .setup
                .sessions_dir
                .as_ref()
                .ok_or("this project has no session logs")?;
            return sessions::open(
                dir,
                id,
                entry,
                OPEN_CONTEXT_ENTRIES,
                OPEN_CONTEXT_ENTRIES,
                OPEN_BLOCK_BYTES,
            )
            .map(cut_output);
        }
        if let Some(rest) = reference.strip_prefix("commit:") {
            let (name, sha) = rest.rsplit_once('@').unwrap_or(("", rest));
            let repos = self.repos();
            return git::open(&repos, name, sha, cancel).map(cut_output);
        }
        if let Some(rest) = reference.strip_prefix("file:") {
            let (path, line) = rest.rsplit_once(':').unwrap_or((rest, "1"));
            return Err(format!("use the read tool on {path}, from line {line}"));
        }
        Err(format!(
            "`{reference}` is not a reference: session:<id>#<entry>, commit:<repo>@<sha> or file:<path>:<line>"
        ))
    }
}

/// `shown` cut to [`MAX_OUTPUT_BYTES`] on a character boundary, marked.
fn cut_output(shown: String) -> String {
    if shown.len() <= MAX_OUTPUT_BYTES {
        return shown;
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !shown.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[cut]", &shown[..end])
}

fn string_list(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// A search request from a tool call's arguments.
fn parse_request(arguments: &Value) -> Result<SearchRequest, String> {
    let queries = string_list(&arguments["queries"]);
    if queries.is_empty() {
        return Err("give `queries` (one or more search phrases) or `open` (a reference)".into());
    }
    let since = match arguments["since"].as_str().map(str::trim) {
        Some(date) if !date.is_empty() => Some(parse_date(date)?),
        _ => None,
    };
    let limit = arguments["limit"]
        .as_u64()
        .map_or(DEFAULT_LIMIT, |n| usize::try_from(n).unwrap_or(MAX_LIMIT))
        .clamp(1, MAX_LIMIT);
    Ok(SearchRequest {
        queries,
        sources: Sources::parse(&arguments["sources"])?,
        paths: string_list(&arguments["paths"]),
        since,
        limit,
        raw: arguments["raw"].as_bool().unwrap_or(false),
        cwd: None,
    })
}

impl Tool for RecallTool {
    fn name(&self) -> &str {
        "recall"
    }

    fn description(&self) -> &str {
        &ToolText::seed("recall").description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "queries": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Search phrases: the request in English and in the user's language, plus likely identifiers and file names"
                },
                "sources": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["sessions", "git", "files"] },
                    "description": "Where to look; all three when absent"
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Paths or globs to narrow to, relative to the working directory: files under them, commits touching them, session entries naming them"
                },
                "since": {
                    "type": "string",
                    "description": "Only sessions and commits from this date on, YYYY-MM-DD"
                },
                "limit": {
                    "type": "integer",
                    "description": "How many results, 1 to 30; 10 when absent"
                },
                "open": {
                    "type": "string",
                    "description": "A reference from an earlier result, to see its context instead of searching"
                },
                "raw": {
                    "type": "boolean",
                    "description": "Return the results as found, without the summarised answer"
                }
            }
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed("recall").snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed("recall").guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        if let Some(reference) = call.arguments["open"]
            .as_str()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            return match self.open(reference, cancel) {
                Ok(text) => ToolResultMessage::text(call, text),
                Err(error) => ToolResultMessage::error(call, error),
            };
        }
        let request = match parse_request(&call.arguments) {
            Ok(request) => request,
            Err(error) => return ToolResultMessage::error(call, error),
        };
        on_update(ToolUpdate::Output(format!(
            "searching {}…",
            request.sources.names().join(", ")
        )));
        let request = SearchRequest {
            cwd: Some(ctx.cwd.clone()),
            ..request
        };
        let outcome = self.search(request, ctx.session.as_ref(), cancel);
        if cancel.is_cancelled() {
            return ToolResultMessage::error(call, "cancelled");
        }
        let details = json!({
            "results": outcome.hits.len(),
            "answered": matches!(outcome.answer, Some(Ok(_))),
        });
        ToolResultMessage::text(call, render(&outcome)).with_details(details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_agent_core::message::{Message, UserMessage};
    use termide_agent_core::Session;

    fn setup(project: &std::path::Path, sessions: &std::path::Path) -> RecallSetup {
        RecallSetup {
            project_root: project.to_path_buf(),
            sessions_dir: Some(sessions.to_path_buf()),
            find_repos: RepoFinder::new(|root| {
                if root.join(".git").exists() {
                    vec![root.to_path_buf()]
                } else {
                    Vec::new()
                }
            }),
            panel_dir: None,
            solver: None,
            time_limits: TimeLimits::default(),
        }
    }

    #[test]
    fn snippets_centre_on_the_first_match() {
        let long = format!("{} walrus {}", "a ".repeat(400), "b ".repeat(400));
        let cut = snippet(&long, &["walrus".to_string()]);
        assert!(cut.starts_with('…') && cut.ends_with('…'));
        assert!(cut.contains("walrus"));
        assert_eq!(snippet("short   text\nhere", &[]), "short text here");
    }

    #[test]
    fn opened_context_is_cut_to_the_output_bound() {
        assert_eq!(cut_output("short".into()), "short");
        let long = "ж".repeat(MAX_OUTPUT_BYTES);
        let cut = cut_output(long);
        assert!(cut.ends_with("\n[cut]"));
        assert!(cut.len() <= MAX_OUTPUT_BYTES + "\n[cut]".len());
    }

    #[test]
    fn arguments_parse_and_bad_ones_are_explained() {
        let request = parse_request(&json!({
            "queries": ["why no tokio", "почему без tokio"],
            "sources": ["sessions", "git"],
            "paths": "crates/agent-core",
            "since": "2026-09-01",
            "limit": 100
        }))
        .unwrap();
        assert_eq!(request.queries.len(), 2);
        assert!(request.sources.sessions && request.sources.git && !request.sources.files);
        assert_eq!(request.paths, ["crates/agent-core"]);
        assert_eq!(request.limit, MAX_LIMIT);
        assert!(request.since.is_some());
        let single = parse_request(&json!({ "queries": "x", "sources": "git" })).unwrap();
        assert!(single.sources.git && !single.sources.sessions && !single.sources.files);
        assert!(parse_request(&json!({})).is_err());
        assert!(parse_request(&json!({ "queries": ["x"], "sources": ["web"] })).is_err());
        assert!(parse_request(&json!({ "queries": ["x"], "sources": "web" })).is_err());
        assert!(parse_request(&json!({ "queries": ["x"], "since": "soon" })).is_err());
    }

    #[test]
    fn a_search_finds_sessions_commits_and_files_together() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        git::tests::repo_with(
            &project,
            &[
                (
                    "src/walrus.rs",
                    "pub fn walrus() {}\n",
                    "feat: walrus support",
                ),
                ("docs/ci.md", "Run CI on every push.\n", "ci: run on push"),
                (
                    "docs/decision.md",
                    "A decision was made.\n",
                    "docs: record the decision",
                ),
            ],
        );
        let sessions_dir = tmp.path().join("sessions");
        let mut session = Session::create(&sessions_dir, &project).unwrap();
        session
            .append_message(&Message::User(UserMessage::text(
                "we picked the walrus design over the seal one",
            )))
            .unwrap();
        let tool = RecallTool::new(setup(&project, &sessions_dir));
        let outcome = tool.search(SearchRequest::new("walrus"), None, &CancelToken::new());
        let sources: Vec<Source> = outcome.hits.iter().map(|h| h.source).collect();
        assert!(sources.contains(&Source::Session));
        assert!(sources.contains(&Source::Git));
        assert!(sources.contains(&Source::File));
        let text = render(&outcome);
        assert!(text.starts_with("3 results for \"walrus\""), "{text}");
        assert!(to_json(&outcome)["hits"].as_array().unwrap().len() == 3);

        let reference = outcome
            .hits
            .iter()
            .find(|h| h.source == Source::Session)
            .unwrap()
            .reference
            .clone();
        assert!(tool
            .open(&reference, &CancelToken::new())
            .unwrap()
            .contains("seal one"));
        assert!(tool
            .open("file:src/walrus.rs:1", &CancelToken::new())
            .is_err());
        assert!(tool.open("nonsense", &CancelToken::new()).is_err());

        // An acronym is found as a whole word, in commits and files, and not
        // inside a longer one.
        let ci = tool.search(SearchRequest::new("CI"), None, &CancelToken::new());
        let refs: Vec<&str> = ci.hits.iter().map(|h| h.reference.as_str()).collect();
        assert!(ci.hits.iter().any(|h| h.source == Source::Git), "{refs:?}");
        assert!(refs.contains(&"file:docs/ci.md:1"), "{refs:?}");
        assert!(!refs.iter().any(|r| r.contains("decision")), "{refs:?}");

        let none = tool.search(SearchRequest::new("zebra"), None, &CancelToken::new());
        assert!(render(&none).starts_with("No results"));

        // From a panel working in `src`, `paths` and file results are its.
        let from_src = SearchRequest {
            sources: Sources {
                sessions: false,
                git: false,
                files: true,
            },
            paths: vec!["walrus.rs".to_string()],
            cwd: Some(project.join("src")),
            ..SearchRequest::new("walrus")
        };
        let outcome = tool.search(from_src, None, &CancelToken::new());
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].reference, "file:walrus.rs:1");
        assert!(outcome.hits[0].label.starts_with("walrus.rs"));
    }

    #[test]
    fn a_repository_created_after_the_tool_is_searched() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let tool = RecallTool::new(setup(&project, &tmp.path().join("sessions")));
        let git_only = || SearchRequest {
            sources: Sources {
                sessions: false,
                git: true,
                files: false,
            },
            ..SearchRequest::new("walrus")
        };
        assert!(tool
            .search(git_only(), None, &CancelToken::new())
            .hits
            .is_empty());

        git::tests::repo_with(&project, &[("a.txt", "a", "feat: walrus parser")]);
        let outcome = tool.search(git_only(), None, &CancelToken::new());
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].label, "feat: walrus parser");
    }

    #[test]
    fn the_panel_directory_outside_the_project_is_searched_too() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        git::tests::repo_with(
            &elsewhere,
            &[("notes.md", "the walrus plan\n", "feat: walrus parser")],
        );
        let panel_sessions = tmp.path().join("sessions/elsewhere");
        let mut session = Session::create(&panel_sessions, &elsewhere).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("walrus again")))
            .unwrap();
        let tool = RecallTool::new(RecallSetup {
            panel_dir: Some(PanelDir {
                dir: elsewhere.clone(),
                sessions_dir: Some(panel_sessions),
            }),
            ..setup(&project, &tmp.path().join("sessions/proj"))
        });
        let request = SearchRequest {
            cwd: Some(elsewhere.clone()),
            ..SearchRequest::new("walrus")
        };
        let outcome = tool.search(request, None, &CancelToken::new());
        let refs: Vec<&str> = outcome.hits.iter().map(|h| h.reference.as_str()).collect();
        assert!(refs.contains(&"file:notes.md:1"), "{refs:?}");
        assert!(
            outcome.hits.iter().any(|h| h.source == Source::Session),
            "{refs:?}"
        );
        let commit = refs
            .iter()
            .find(|r| r.starts_with("commit:"))
            .expect("the panel's repository is searched");
        assert!(commit.starts_with(&format!("commit:{}@", elsewhere.display())));
        assert!(tool
            .open(commit, &CancelToken::new())
            .unwrap()
            .contains("walrus parser"));
    }

    #[test]
    fn session_logs_under_the_project_are_not_searched_as_files() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let sessions_dir = project.join("config/ai/sessions/proj");
        let mut session = Session::create(&sessions_dir, project).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("the walrus plan")))
            .unwrap();
        std::fs::write(project.join("notes.txt"), "walrus notes").unwrap();
        let outcome = RecallTool::new(setup(project, &sessions_dir)).search(
            SearchRequest::new("walrus"),
            None,
            &CancelToken::new(),
        );
        let found: Vec<&str> = outcome
            .hits
            .iter()
            .filter(|h| h.source == Source::File)
            .map(|h| h.label.as_str())
            .collect();
        assert_eq!(found, ["notes.txt"]);
        assert!(outcome.hits.iter().any(|h| h.source == Source::Session));
    }

    #[test]
    fn a_source_out_of_time_stops_alone_and_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("notes.txt"), "walrus").unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let mut session = Session::create(&sessions_dir, &project).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("the walrus plan")))
            .unwrap();
        let mut setup = setup(&project, &sessions_dir);
        // Files have no time at all; the sessions still have theirs.
        setup.time_limits.files = Duration::ZERO;
        let outcome =
            RecallTool::new(setup).search(SearchRequest::new("walrus"), None, &CancelToken::new());
        assert_eq!(outcome.incomplete, [("files", Duration::ZERO)]);
        assert!(outcome.hits.iter().any(|h| h.source == Source::Session));
        assert!(render(&outcome).contains("Not searched through: files at its 0 s limit"));
        assert_eq!(to_json(&outcome)["incomplete"], json!(["files"]));
    }

    #[test]
    fn the_solver_answers_from_the_hits_and_failure_leaves_them() {
        use termide_agent_core::message::{AssistantContent, AssistantMessage, StopReason, Usage};
        use termide_agent_core::{Request, StreamEvent};

        struct Canned(Option<String>);
        impl Provider for Canned {
            fn name(&self) -> &str {
                "canned"
            }
            fn stream(
                &self,
                request: &Request<'_>,
                _: &mut dyn FnMut(StreamEvent),
                _: &CancelToken,
            ) -> AssistantMessage {
                let Message::User(user) = &request.messages[0] else {
                    unreachable!()
                };
                assert!(
                    user.plain_text().contains("session:"),
                    "the hits are in the question"
                );
                match &self.0 {
                    Some(answer) => AssistantMessage {
                        content: vec![AssistantContent::Text {
                            text: answer.clone(),
                        }],
                        stop_reason: StopReason::Stop,
                        usage: Usage::default(),
                        provider: "canned".into(),
                        model: "m".into(),
                        error_message: None,
                        timestamp: 0,
                    },
                    None => AssistantMessage::failed("canned", "m", StopReason::Error, "offline"),
                }
            }
        }
        let model = ModelSpec {
            provider: "canned".into(),
            id: "m".into(),
            context_window: 8_000,
            max_tokens: None,
            thinking: termide_agent_core::ThinkingLevel::Off,
        };
        let tmp = tempfile::tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        let mut session = Session::create(&sessions_dir, tmp.path()).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("the walrus decision")))
            .unwrap();
        let mut request = SearchRequest::new("walrus");
        request.sources = Sources {
            sessions: true,
            git: false,
            files: false,
        };
        for (reply, expect_answer) in [
            (Some("Walrus, see session:x#y.".to_string()), true),
            (None, false),
        ] {
            let mut setup = setup(tmp.path(), &sessions_dir);
            setup.solver = Some(Solver {
                prompt: RecallPrompt::default(),
                model: Some((Arc::new(Canned(reply)), model.clone())),
            });
            let outcome = RecallTool::new(setup).search(request.clone(), None, &CancelToken::new());
            assert_eq!(matches!(outcome.answer, Some(Ok(_))), expect_answer);
            let text = render(&outcome);
            assert!(
                text.contains("session:"),
                "the references always come back: {text}"
            );
            if !expect_answer {
                assert!(text.starts_with("(The solver could not answer: offline.)"));
            }
        }
        // `raw` skips the solver.
        let mut setup = setup(tmp.path(), &sessions_dir);
        setup.solver = Some(Solver {
            prompt: RecallPrompt::default(),
            model: Some((Arc::new(Canned(None)), model)),
        });
        let mut raw = request;
        raw.raw = true;
        assert!(RecallTool::new(setup)
            .search(raw, None, &CancelToken::new())
            .answer
            .is_none());
    }
}
