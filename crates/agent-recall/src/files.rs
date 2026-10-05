//! The project's files as search documents: notes, documents and code.
//!
//! A text file — Markdown, plain text and the like — is prose, and prose is
//! searched by section and shown by paragraph: a Markdown file is cut at its
//! headings, so each decision in a long `decisions.md` is a result of its
//! own, labelled with its heading; a matching paragraph is shown whole
//! (within a bound), not as the wrapped line that happened to hold the word;
//! and a recently changed file ranks above a stale one, as a later note tends
//! to supersede an earlier one. Any other file is code: one result per file,
//! shown by its matching lines, with no weight for age.
//!
//! The project root is walked, and every repository nested in it is walked
//! on its own, with its own ignore rules: an outer `.gitignore` often lists
//! the repositories inside, which a single walk would then skip. A walk steps
//! over the roots of the other walks, so nothing is read twice. As ripgrep
//! does, a walk skips hidden entries and binary files (a NUL byte near the
//! start), and like `rg --one-file-system` it stays on the file system it
//! starts on, so a mounted disk or a virtual machine's files under the
//! project are not read. Files of any size are read line by line; the
//! search's deadline is what bounds a large tree.

use std::collections::HashSet;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use regex::{Regex, RegexBuilder};
use termide_agent_core::CancelToken;

use crate::filter::{PathFilter, Scope};
use crate::git::RepoRoot;
use crate::rank::{bm25, recency, Hit, Source, TermBag, Vocabulary};
use crate::text::terms;

/// Characters of a long line kept around its match.
const MAX_LINE_CHARS: usize = 400;
/// Characters of a paragraph kept around its match.
const MAX_PARAGRAPH_CHARS: usize = 500;
/// Matching lines or paragraphs a document keeps for ranking.
const EXCERPTS_PER_DOC: usize = 40;
/// Bytes of one line that are read; the rest of a longer one (minified code,
/// data on one line) is skipped, so a huge file costs no huge buffer.
const MAX_READ_LINE_BYTES: usize = 64 * 1024;
/// Bytes a paragraph gathers before it is cut, for prose with no blank line.
const MAX_READ_PARAGRAPH_BYTES: usize = 64 * 1024;
/// Matching lines a code hit shows.
const SHOWN_LINES: usize = 3;
/// Matching paragraphs a prose hit shows.
const SHOWN_PARAGRAPHS: usize = 2;
/// Headings a prose hit's label shows, innermost last.
const SHOWN_HEADINGS: usize = 2;
/// Extensions of prose; `md`, `markdown` and `mdx` are also cut at headings.
const PROSE_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "text", "rst", "org", "adoc", "asciidoc",
];
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdx"];

/// One walk: a directory and where it sits in the project.
struct WalkRoot {
    dir: PathBuf,
    /// Relative to the project root, empty for the root itself.
    project_dir: String,
}

/// The walks for `project_root` and the repositories in it.
fn walk_roots(project_root: &Path, repos: &[RepoRoot]) -> Vec<WalkRoot> {
    let mut roots = vec![WalkRoot {
        dir: project_root.to_path_buf(),
        project_dir: String::new(),
    }];
    for repo in repos.iter().filter(|r| r.name != ".") {
        roots.push(WalkRoot {
            dir: repo.root.clone(),
            project_dir: repo.name.clone(),
        });
    }
    roots
}

fn looks_binary(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let mut head = [0u8; 8192];
    let read = file.read(&mut head).unwrap_or(0);
    head[..read].contains(&0)
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// What a file search looks for.
#[derive(Debug, Clone, Copy)]
pub struct FileQuery<'a> {
    /// The terms of each phrasing of the query.
    pub terms: &'a [Vec<Vec<String>>],
    /// Substrings a line must hold one of: the stems of the query words.
    pub patterns: &'a [String],
    /// Whole words a line may hold instead: the query's acronyms.
    pub words: &'a [String],
    pub paths: &'a PathFilter,
    /// Directories never walked: the session logs, a source of their own.
    pub excluded: &'a [PathBuf],
    pub now: u64,
}

/// One searchable piece of a file: a whole code file, or a section of prose.
struct Doc {
    path: String,
    /// The line a reference points at: the section's heading, or the first
    /// match.
    line: usize,
    /// The headings above the section, outermost first; empty for code and
    /// for prose before its first heading.
    headings: Vec<String>,
    /// Matching lines (code) or paragraphs (prose), with their first line.
    excerpts: Vec<(usize, String)>,
    /// Prose is shown by paragraph, code by line.
    prose: bool,
    /// When the file last changed, for prose; code carries no date.
    modified: Option<u64>,
    bag: TermBag,
}

/// The best files and sections for `query` in the project, best first, and
/// whether the walk ran out of time before it saw every file.
#[must_use]
pub fn search(
    project_root: &Path,
    repos: &[RepoRoot],
    query: &FileQuery<'_>,
    limit: usize,
    deadline: Instant,
    cancel: &CancelToken,
) -> (Vec<Hit>, bool) {
    if query.patterns.is_empty() && query.words.is_empty() {
        return (Vec::new(), false);
    }
    let alternation = query
        .patterns
        .iter()
        .map(|p| regex::escape(p))
        .chain(
            query
                .words
                .iter()
                .map(|w| format!(r"\b{}\b", regex::escape(w))),
        )
        .collect::<Vec<_>>()
        .join("|");
    let Ok(matcher) = RegexBuilder::new(&alternation)
        .case_insensitive(true)
        .build()
    else {
        return (Vec::new(), false);
    };
    let roots = walk_roots(project_root, repos);
    let skip: HashSet<PathBuf> = roots
        .iter()
        .map(|r| r.dir.clone())
        .chain(query.excluded.iter().cloned())
        .collect();
    let mut vocabulary = Vocabulary::default();
    let mut docs: Vec<Doc> = Vec::new();
    let mut cut_short = false;
    for root in &roots {
        let filter = match query.paths.within(&root.project_dir) {
            Scope::Nothing => continue,
            Scope::All => None,
            Scope::Some(specs) => Some(PathFilter::new(
                &specs
                    .iter()
                    .map(|s| s.as_str().to_string())
                    .collect::<Vec<_>>(),
            )),
        };
        let walk = Walk {
            root,
            skip: &skip,
            filter: filter.as_ref(),
            matcher: &matcher,
            deadline,
            cancel,
        };
        if !walk.run(&mut vocabulary, &mut docs) {
            cut_short = !cancel.is_cancelled();
            break;
        }
    }
    let phrases = vocabulary.phrases(query.terms);
    let bags: Vec<&TermBag> = docs.iter().map(|d| &d.bag).collect();
    let scores = bm25(&bags, &phrases);
    let mut ranked: Vec<(f64, &Doc)> = docs
        .iter()
        .zip(scores)
        .filter(|(_, score)| *score > 0.0)
        .map(|(doc, score)| {
            let weight = doc.modified.map_or(1.0, |at| recency(at, query.now));
            (score * weight, doc)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.truncate(limit);
    let hits = ranked
        .into_iter()
        .map(|(score, doc)| Hit {
            source: Source::File,
            reference: format!("file:{}:{}", doc.path, doc.line),
            timestamp: doc.modified,
            label: label(doc),
            snippet: snippet(doc),
            related: None,
            score,
        })
        .collect();
    (hits, cut_short)
}

/// The path, and for a section the innermost headings: `decisions.md › D-0029 — …`.
fn label(doc: &Doc) -> String {
    let skip = doc.headings.len().saturating_sub(SHOWN_HEADINGS);
    std::iter::once(doc.path.as_str())
        .chain(doc.headings[skip..].iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" › ")
}

fn snippet(doc: &Doc) -> String {
    let shown = if doc.prose {
        SHOWN_PARAGRAPHS
    } else {
        SHOWN_LINES
    };
    doc.excerpts
        .iter()
        .take(shown)
        .map(|(n, text)| format!("{n}: {}", text.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` cut to `max` characters around byte offset `at`.
fn around(text: &str, at: usize, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let centre = chars.iter().position(|(i, _)| *i >= at).unwrap_or(0);
    let start = centre.saturating_sub(max / 4);
    let end = (start + max).min(chars.len());
    let mut out: String = chars[start..end].iter().map(|(_, c)| c).collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// A Markdown ATX heading: its level and text.
fn heading(line: &str) -> Option<(usize, String)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    Some((level, text.to_string()))
}

/// One root's walk and what it needs.
struct Walk<'a> {
    root: &'a WalkRoot,
    skip: &'a HashSet<PathBuf>,
    filter: Option<&'a PathFilter>,
    matcher: &'a Regex,
    deadline: Instant,
    cancel: &'a CancelToken,
}

impl Walk<'_> {
    fn out_of_time(&self) -> bool {
        self.cancel.is_cancelled() || Instant::now() > self.deadline
    }

    /// Walk the root; `false` when the deadline or a cancel stopped it.
    fn run(&self, vocabulary: &mut Vocabulary, docs: &mut Vec<Doc>) -> bool {
        let base = self.root.dir.clone();
        let others: HashSet<PathBuf> = self.skip.iter().filter(|d| **d != base).cloned().collect();
        let walk = ignore::WalkBuilder::new(&self.root.dir)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .same_file_system(true)
            .filter_entry(move |entry| {
                entry.file_name() != ".git" && !others.contains(entry.path())
            })
            .build();
        for entry in walk {
            if self.out_of_time() {
                return false;
            }
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(&base) else {
                continue;
            };
            let relative = relative.to_string_lossy().into_owned();
            if self.filter.is_some_and(|f| !f.matches(&relative)) || looks_binary(path) {
                continue;
            }
            let project_path = if self.root.project_dir.is_empty() {
                relative
            } else {
                format!("{}/{relative}", self.root.project_dir)
            };
            let ext = extension(path);
            let found = if PROSE_EXTENSIONS.contains(&ext.as_str()) {
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                let markdown = MARKDOWN_EXTENSIONS.contains(&ext.as_str());
                self.prose(path, &project_path, markdown, modified)
            } else {
                self.code(path, &project_path)
            };
            let Some(found) = found else {
                return false;
            };
            for mut doc in found {
                let mut indexed = doc.path.clone();
                for heading in &doc.headings {
                    indexed.push('\n');
                    indexed.push_str(heading);
                }
                for (_, text) in &doc.excerpts {
                    indexed.push('\n');
                    indexed.push_str(text);
                }
                doc.bag = TermBag::new(vocabulary, terms(&indexed));
                docs.push(doc);
            }
        }
        true
    }

    /// Read `path` line by line, calling `each` with the line number and
    /// text, a line cut to [`MAX_READ_LINE_BYTES`]; `None` when the deadline
    /// came first. A file that cannot be opened (gone since the walk saw it)
    /// is read as empty.
    fn lines(&self, path: &Path, mut each: impl FnMut(usize, &str)) -> Option<()> {
        let Ok(file) = std::fs::File::open(path) else {
            return Some(());
        };
        let mut reader = std::io::BufReader::new(file);
        let mut buffer = Vec::new();
        let mut number = 0;
        loop {
            buffer.clear();
            let limit = MAX_READ_LINE_BYTES as u64;
            match reader.by_ref().take(limit).read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => return Some(()),
                Ok(_) => {}
            }
            if buffer.len() == MAX_READ_LINE_BYTES
                && buffer.last() != Some(&b'\n')
                && reader.skip_until(b'\n').is_err()
            {
                return Some(());
            }
            number += 1;
            if number % 4096 == 0 && self.out_of_time() {
                return None;
            }
            let line = String::from_utf8_lossy(&buffer);
            each(number, line.trim_end_matches(['\n', '\r']));
        }
    }

    /// A code file: one document with its matching lines. `None` when the
    /// deadline came first; an empty list when nothing matched.
    fn code(&self, path: &Path, project_path: &str) -> Option<Vec<Doc>> {
        let mut excerpts = Vec::new();
        self.lines(path, |number, line| {
            if excerpts.len() < EXCERPTS_PER_DOC {
                if let Some(found) = self.matcher.find(line) {
                    excerpts.push((number, around(line, found.start(), MAX_LINE_CHARS)));
                }
            }
        })?;
        Some(if excerpts.is_empty() {
            Vec::new()
        } else {
            vec![Doc {
                path: project_path.to_string(),
                line: excerpts[0].0,
                headings: Vec::new(),
                excerpts,
                prose: false,
                modified: None,
                bag: TermBag::default(),
            }]
        })
    }

    /// A prose file: a document per section (per file when it has no
    /// headings) that has a matching paragraph or heading, the paragraphs
    /// kept whole within a bound.
    fn prose(
        &self,
        path: &Path,
        project_path: &str,
        markdown: bool,
        modified: Option<u64>,
    ) -> Option<Vec<Doc>> {
        struct Section {
            line: usize,
            headings: Vec<(usize, String)>,
            heading_matched: bool,
            excerpts: Vec<(usize, String)>,
        }
        let mut docs = Vec::new();
        let mut section = Section {
            line: 0,
            headings: Vec::new(),
            heading_matched: false,
            excerpts: Vec::new(),
        };
        let mut paragraph: Vec<String> = Vec::new();
        let mut paragraph_start = 0;
        let mut paragraph_bytes = 0;
        let mut in_fence = false;
        let flush_paragraph = |paragraph: &mut Vec<String>, start: usize, section: &mut Section| {
            if paragraph.is_empty() {
                return;
            }
            let text = paragraph.join(" ");
            paragraph.clear();
            if section.excerpts.len() >= EXCERPTS_PER_DOC {
                return;
            }
            if let Some(found) = self.matcher.find(&text) {
                let collapsed = around(&text, found.start(), MAX_PARAGRAPH_CHARS);
                section.excerpts.push((start, collapsed));
            }
        };
        let finish = |section: &mut Section, docs: &mut Vec<Doc>| {
            if section.excerpts.is_empty() && !section.heading_matched {
                return;
            }
            let line = if section.line > 0 {
                section.line
            } else {
                section.excerpts.first().map_or(1, |(n, _)| *n)
            };
            docs.push(Doc {
                path: project_path.to_string(),
                line,
                headings: section.headings.iter().map(|(_, h)| h.clone()).collect(),
                excerpts: std::mem::take(&mut section.excerpts),
                prose: true,
                modified,
                bag: TermBag::default(),
            });
        };
        self.lines(path, |number, line| {
            let fence =
                line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
            if markdown && fence {
                in_fence = !in_fence;
            }
            let found = (markdown && !in_fence && !fence)
                .then(|| heading(line))
                .flatten();
            if let Some((level, text)) = found {
                flush_paragraph(&mut paragraph, paragraph_start, &mut section);
                finish(&mut section, &mut docs);
                section.headings.retain(|(l, _)| *l < level);
                section.headings.push((level, text.clone()));
                section.line = number;
                section.heading_matched = self.matcher.is_match(&text);
            } else if line.trim().is_empty() {
                flush_paragraph(&mut paragraph, paragraph_start, &mut section);
            } else {
                if paragraph.is_empty() {
                    paragraph_start = number;
                    paragraph_bytes = 0;
                }
                paragraph_bytes += line.len();
                paragraph.push(line.trim().to_string());
                if paragraph_bytes > MAX_READ_PARAGRAPH_BYTES {
                    flush_paragraph(&mut paragraph, paragraph_start, &mut section);
                }
            }
        })?;
        flush_paragraph(&mut paragraph, paragraph_start, &mut section);
        finish(&mut section, &mut docs);
        Some(docs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::repo_with;
    use crate::text::query_words;

    fn query<'a>(
        terms_: &'a [Vec<Vec<String>>],
        patterns: &'a [String],
        paths: &'a PathFilter,
    ) -> FileQuery<'a> {
        FileQuery {
            terms: terms_,
            patterns,
            words: &[],
            paths,
            excluded: &[],
            now: termide_agent_core::message::now_millis(),
        }
    }

    fn later() -> Instant {
        Instant::now() + std::time::Duration::from_secs(60)
    }

    #[test]
    fn an_overlong_line_is_read_up_to_its_bound_and_the_next_one_still_is() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        let line = format!(
            "walrus {} narwhal\nseal\n",
            "x ".repeat(MAX_READ_LINE_BYTES)
        );
        std::fs::write(project.join("bundle.js"), &line).unwrap();
        std::fs::write(project.join("dump.txt"), &line).unwrap();
        let none = PathFilter::default();
        let find = |word: &str| {
            let patterns = vec![word.to_string()];
            let words = [query_words(word)];
            let (hits, cut_short) = search(
                project,
                &[],
                &query(&words, &patterns, &none),
                10,
                later(),
                &CancelToken::new(),
            );
            assert!(!cut_short);
            let mut labels: Vec<String> = hits.into_iter().map(|h| h.label).collect();
            labels.sort_unstable();
            labels
        };
        assert_eq!(find("walrus"), ["bundle.js", "dump.txt"]);
        assert!(find("narwhal").is_empty(), "past the bound is not read");
        assert_eq!(find("seal"), ["bundle.js", "dump.txt"]);
    }

    #[test]
    fn loose_files_are_searched_and_hidden_or_binary_ones_are_not() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::write(project.join("notes.txt"), "walrus loose").unwrap();
        std::fs::write(project.join("blob.bin"), b"walrus\0binary").unwrap();
        let long = format!("{} walrus {}", "x".repeat(5000), "y".repeat(5000));
        std::fs::write(project.join("long.rs"), &long).unwrap();
        std::fs::create_dir_all(project.join(".cache")).unwrap();
        std::fs::write(project.join(".cache/log.json"), "walrus cached").unwrap();
        repo_with(&project.join("app"), &[("main.rs", "// walrus\n", "init")]);
        std::fs::write(project.join("app/.hidden.rs"), "walrus hidden").unwrap();
        let repos = vec![RepoRoot::new(project.join("app"), project)];
        let patterns = vec!["walrus".to_string()];
        let none = PathFilter::default();
        let walrus = [query_words("walrus")];
        let (hits, cut_short) = search(
            project,
            &repos,
            &query(&walrus, &patterns, &none),
            10,
            later(),
            &CancelToken::new(),
        );
        assert!(!cut_short);
        let mut paths: Vec<&str> = hits.iter().map(|h| h.label.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, ["app/main.rs", "long.rs", "notes.txt"]);
        let long_hit = hits.iter().find(|h| h.label == "long.rs").unwrap();
        assert!(long_hit.snippet.contains("walrus"));
        assert!(
            long_hit.snippet.chars().count() < 500,
            "a long line is cut around its match"
        );
        // Prose carries its date, code does not.
        let notes = hits.iter().find(|h| h.label == "notes.txt").unwrap();
        assert!(notes.timestamp.is_some());
        assert!(long_hit.timestamp.is_none());

        // A deadline already past finds nothing and says so.
        let (hits, cut_short) = search(
            project,
            &repos,
            &query(&walrus, &patterns, &none),
            10,
            Instant::now(),
            &CancelToken::new(),
        );
        assert!(hits.is_empty() && cut_short);
    }

    #[test]
    fn markdown_is_searched_by_section_and_shown_by_paragraph() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        std::fs::write(
            project.join("decisions.md"),
            "# Decisions\n\n\
             ## D-0028 — Pricing\n\nWe keep the price; the walrus\nplan is unrelated.\n\n\
             ## D-0029 — Walrus closed\n\nThe walrus branch closes: three\nreasons, see below.\n\n\
             ```\n# not a heading, walrus in a fence\n```\n\n\
             ### Reasons\n\nRisk and channel.\n\n\
             ## D-0030 — Other\n\nNothing here.\n",
        )
        .unwrap();
        let patterns = vec!["walrus".to_string()];
        let none = PathFilter::default();
        let (hits, _) = search(
            project,
            &[],
            &query(&[query_words("walrus")], &patterns, &none),
            10,
            later(),
            &CancelToken::new(),
        );
        let labels: Vec<&str> = hits.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "decisions.md › Decisions › D-0029 — Walrus closed",
                "decisions.md › Decisions › D-0028 — Pricing"
            ],
            "a section each, the one whose heading also matches first"
        );
        assert_eq!(hits[0].reference, "file:decisions.md:8");
        // The wrapped paragraph comes back whole, the fenced line as part of
        // its own paragraph, not as a heading.
        assert!(hits[0]
            .snippet
            .contains("10: The walrus branch closes: three reasons, see below."));
        assert!(hits[0].snippet.contains("not a heading, walrus in a fence"));
    }

    #[test]
    fn nested_repositories_are_searched_even_when_the_outer_one_ignores_them() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        repo_with(
            project,
            &[
                (".gitignore", "inner/\ntarget/\n", "init"),
                ("src/main.rs", "fn walrus_outer() {}\n", "outer"),
            ],
        );
        repo_with(
            &project.join("inner"),
            &[("lib.rs", "// the walrus lives here\n", "inner")],
        );
        std::fs::create_dir_all(project.join("target")).unwrap();
        std::fs::write(project.join("target/gen.rs"), "walrus generated").unwrap();
        let repos = vec![
            RepoRoot::new(project.to_path_buf(), project),
            RepoRoot::new(project.join("inner"), project),
        ];
        let patterns = vec!["walrus".to_string()];
        let none = PathFilter::default();
        let (hits, _) = search(
            project,
            &repos,
            &query(&[query_words("walrus")], &patterns, &none),
            10,
            later(),
            &CancelToken::new(),
        );
        let mut paths: Vec<&str> = hits.iter().map(|h| h.label.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            ["inner/lib.rs", "src/main.rs"],
            "ignored files stay out, nothing twice"
        );
        assert!(hits.iter().any(|h| h.reference == "file:inner/lib.rs:1"));

        let only_inner = PathFilter::new(&["inner".to_string()]);
        let (hits, _) = search(
            project,
            &repos,
            &query(&[query_words("walrus")], &patterns, &only_inner),
            10,
            later(),
            &CancelToken::new(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "inner/lib.rs");
    }
}
