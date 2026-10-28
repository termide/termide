//! Incremental whole-document highlighting.
//!
//! A [`DocumentHighlight`] keeps the syntax tree of the document between
//! renders. Each sync feeds the buffer's edits to the old tree, re-parses
//! reading straight from the rope, and marks stale only the lines the edits
//! touched plus those whose syntax changed (`Tree::changed_ranges`). Stale
//! lines are re-highlighted when they are on screen, by running the
//! highlights query over just their byte range.
//!
//! The capture walk mirrors `tree_sitter_highlight`'s, so a language
//! highlights the same here as through `Highlighter::highlight`. That crate
//! cannot take a tree that was parsed elsewhere — it parses the whole source
//! itself on every call — which is why the walk lives here. A grammar
//! without injections is walked here line by line; one with injections goes
//! through [`crate::layers`], which keeps a tree per injected range and
//! replays `tree_sitter_highlight`'s layer interleaving over the visible
//! byte range.

use ratatui::style::Style;
use ropey::Rope;
use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;
use tree_sitter::{
    InputEdit, Node, Parser, Query, QueryCursor, StreamingIterator, TextProvider, Tree,
};

use crate::layers::{edit_bytes, HostCombined, HostLayer, LayerCache, Resolver};

/// Styled pieces of one line, its line break excluded.
pub(crate) type LineSegments = Vec<(Cow<'static, str>, Style)>;

/// Where a document's text stands: which document, and how many edits into
/// it. A different `document` means the text was replaced wholesale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentRevision {
    pub document: u64,
    pub edit: u64,
}

/// A document the highlighter can follow edit by edit.
pub trait DocumentSource {
    /// The current text.
    fn rope(&self) -> &Rope;
    /// Where the current text stands.
    fn revision(&self) -> DocumentRevision;
    /// The edits that lead from `since` to the current text, oldest first,
    /// each in the coordinates of the text right before it. `None` when they
    /// are not all known, which makes the highlighter parse from scratch.
    fn edits_since(&self, since: DocumentRevision) -> Option<Vec<InputEdit>>;
}

/// The highlights and injections of one language, as its
/// `HighlightConfiguration` holds them.
#[derive(Clone, Copy)]
pub(crate) struct QueryHighlights<'a> {
    /// The name the language is loaded under (its `language_name`).
    pub name: &'a str,
    pub language: &'a tree_sitter::Language,
    /// Injection patterns first, then highlight patterns.
    pub query: &'a Query,
    /// Highlight index (into `HIGHLIGHT_NAMES`) per capture index.
    pub capture_highlights: &'a [Option<usize>],
    /// How many patterns at the start of `query` are injections.
    pub injection_patterns: usize,
    /// The combined injections, matched over a whole tree at once.
    pub combined_injections: Option<&'a Query>,
    pub content_capture: Option<u32>,
    pub language_capture: Option<u32>,
}

impl QueryHighlights<'_> {
    /// Whether other languages can be embedded in this one.
    pub fn has_injections(&self) -> bool {
        self.injection_patterns > 0 || self.combined_injections.is_some()
    }
}

/// A kept syntax tree plus the highlighted lines derived from it.
pub(crate) struct DocumentHighlight {
    parser: Parser,
    cursor: QueryCursor,
    tree: Option<Tree>,
    /// The trees of languages embedded in `tree`, kept across edits.
    layers: LayerCache,
    /// The combined injections of `tree`; `None` until worked out for its
    /// current text.
    combined: Option<HostCombined>,
    /// The revision `tree` and `lines` describe.
    revision: Option<DocumentRevision>,
    /// One entry per rope line; `None` when stale or not highlighted yet.
    pub lines: Vec<Option<LineSegments>>,
}

impl DocumentHighlight {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            cursor: QueryCursor::new(),
            tree: None,
            layers: LayerCache::new(),
            combined: None,
            revision: None,
            lines: Vec::new(),
        }
    }

    /// Forget the tree and every line, e.g. when the language changes.
    pub fn reset(&mut self) {
        self.tree = None;
        self.layers.clear();
        self.combined = None;
        self.revision = None;
        self.lines.clear();
    }

    /// Mark every line stale but keep the tree (the theme changed, the text
    /// did not). Without a tree (the parse failed) the next sync parses
    /// again.
    pub fn clear_lines(&mut self) {
        self.lines.iter_mut().for_each(|line| *line = None);
        if self.tree.is_none() {
            self.revision = None;
        }
    }

    #[cfg(test)]
    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// The injected trees in use: language, included ranges, tree.
    #[cfg(test)]
    pub fn layer_trees(&self) -> impl Iterator<Item = (&str, &[tree_sitter::Range], &Tree)> {
        self.layers.trees()
    }

    /// Bring the tree up to `doc`'s revision and mark stale the lines whose
    /// highlighting may have changed.
    ///
    /// With embedded languages every line goes stale: an edit can change how
    /// text far from it is highlighted through a tree that is only brought
    /// up to date when that text is on screen (a `/*` typed in an HTML
    /// `<script>`, a code fence's language renamed). Only the visible lines
    /// are highlighted again, so this costs no more than an unchanged line.
    pub fn sync_tree(&mut self, doc: &dyn DocumentSource, highlights: &QueryHighlights) {
        let revision = doc.revision();
        if self.revision == Some(revision) {
            return;
        }
        let rope = doc.rope();
        let edits = self.revision.and_then(|since| doc.edits_since(since));
        self.revision = Some(revision);

        if let (Some(mut old_tree), Some(edits)) = (self.tree.take(), edits) {
            // What the edits wrote, in the coordinates of the new text.
            let mut written: Vec<Range<usize>> = Vec::with_capacity(edits.len());
            for edit in &edits {
                old_tree.edit(edit);
                self.layers.edit(edit);
                if let Some(combined) = &mut self.combined {
                    combined.edit(edit);
                }
                written.iter_mut().for_each(|w| edit_bytes(w, edit));
                written.push(edit.start_byte..edit.new_end_byte);
                self.splice_lines(edit);
            }
            if let Some(tree) = parse(&mut self.parser, highlights.language, rope, Some(&old_tree))
            {
                let changed = old_tree
                    .changed_ranges(&tree)
                    .map(|range| range.start_byte..range.end_byte);
                if highlights.has_injections() {
                    let mut regions = written;
                    regions.extend(changed);
                    let layers = &mut self.layers;
                    if self.combined.as_ref().is_some_and(|combined| {
                        layers.host_combined_changed(combined, &tree, highlights, &regions, rope)
                    }) {
                        self.combined = None;
                    }
                    self.lines.iter_mut().for_each(|line| *line = None);
                } else {
                    for range in changed {
                        self.mark_stale(rope, range);
                    }
                }
                self.tree = Some(tree);
                if self.lines.len() == rope.len_lines() {
                    return;
                }
            } else {
                self.combined = None;
            }
        } else {
            self.layers.clear();
            self.combined = None;
            self.tree = parse(&mut self.parser, highlights.language, rope, None);
        }
        // A fresh parse, or line bookkeeping that no longer lines up.
        self.lines = vec![None; rope.len_lines()];
    }

    /// Mirror an edit on the per-line entries: the lines it replaced become
    /// the (stale) lines it wrote, and everything after shifts along.
    fn splice_lines(&mut self, edit: &InputEdit) {
        let len = self.lines.len();
        let start = edit.start_position.row.min(len);
        let old_end = (edit.old_end_position.row + 1).clamp(start, len);
        let written = edit.new_end_position.row - edit.start_position.row + 1;
        self.lines
            .splice(start..old_end, std::iter::repeat_n(None, written));
    }

    fn mark_stale(&mut self, rope: &Rope, bytes: Range<usize>) {
        let len = rope.len_bytes();
        let first = rope.byte_to_line(bytes.start.min(len));
        let last = rope.byte_to_line(bytes.end.min(len).max(bytes.start.min(len)));
        for line in first..=last.min(self.lines.len().saturating_sub(1)) {
            self.lines[line] = None;
        }
    }

    /// Highlight every stale line in `visible`. A no-op without a tree (the
    /// parse failed), leaving those lines to the per-line path. `resolve`
    /// finds the queries of an embedded language by the name its host gives.
    pub fn fill(
        &mut self,
        rope: &Rope,
        visible: Range<usize>,
        highlights: &QueryHighlights<'static>,
        resolve: &Resolver,
        styles: &dyn Fn(usize) -> Style,
        default_style: Style,
    ) {
        let Some(tree) = &self.tree else {
            return;
        };
        let injections = highlights.has_injections();
        let end = visible.end.min(self.lines.len());
        let mut line = visible.start;
        while line < end {
            if self.lines[line].is_some() {
                line += 1;
                continue;
            }
            let run_start = line;
            while line < end && self.lines[line].is_none() {
                line += 1;
            }
            let highlighted = if injections {
                let combined = self.combined.get_or_insert_with(|| {
                    self.layers
                        .host_combined_injections(tree, highlights, rope, resolve)
                });
                let host = HostLayer {
                    tree,
                    highlights: *highlights,
                    combined: combined.injections(),
                };
                let window = line_byte(rope, run_start)..line_byte(rope, line);
                let runs =
                    self.layers
                        .highlight(&host, rope, window, resolve, styles, default_style);
                split_into_lines(rope, run_start..line, &runs)
            } else {
                highlight_lines(
                    &mut self.cursor,
                    tree,
                    rope,
                    run_start..line,
                    highlights,
                    styles,
                    default_style,
                )
            };
            for (slot, segments) in self.lines[run_start..line].iter_mut().zip(highlighted) {
                *slot = Some(segments);
            }
        }
        self.layers.evict();
    }
}

/// Parse `rope` chunk by chunk, reusing `old_tree` where its edits allow.
pub(crate) fn parse(
    parser: &mut Parser,
    language: &tree_sitter::Language,
    rope: &Rope,
    old_tree: Option<&Tree>,
) -> Option<Tree> {
    parser.set_language(language).ok()?;
    let len = rope.len_bytes();
    parser.parse_with_options(
        &mut |byte, _| {
            if byte >= len {
                return &[][..];
            }
            let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - chunk_start..]
        },
        old_tree,
        None,
    )
}

/// Node text for query predicates, read from the rope without copying.
pub(crate) struct RopeText<'a>(pub &'a Rope);

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = std::iter::Map<ropey::iter::Chunks<'a>, fn(&'a str) -> &'a [u8]>;

    fn text(&mut self, node: Node) -> Self::I {
        let rope = self.0;
        // Char-based slicing rounds a byte offset inside a character down, so
        // a malformed range cannot panic.
        let start = rope.byte_to_char(node.start_byte().min(rope.len_bytes()));
        let end = rope.byte_to_char(node.end_byte().min(rope.len_bytes()));
        rope.slice(start..end.max(start))
            .chunks()
            .map(str::as_bytes)
    }
}

#[derive(Clone, Copy)]
struct Capture {
    start: usize,
    end: usize,
    node: usize,
    match_id: u32,
    index: u32,
}

/// Where rope line `line` starts; the end of the text past the last line.
fn line_byte(rope: &Rope, line: usize) -> usize {
    if line >= rope.len_lines() {
        rope.len_bytes()
    } else {
        rope.line_to_byte(line)
    }
}

/// Highlight rope lines `lines`, returning one segment list per line.
///
/// Only captures intersecting those lines are visited. Highlights nest: when
/// an inner one ends, the enclosing one resumes, so what a byte looks like
/// depends only on the captured nodes that contain it — which is what makes
/// highlighting a window of lines agree with highlighting the whole file.
fn highlight_lines(
    cursor: &mut QueryCursor,
    tree: &Tree,
    rope: &Rope,
    lines: Range<usize>,
    highlights: &QueryHighlights,
    styles: &dyn Fn(usize) -> Style,
    default_style: Style,
) -> Vec<LineSegments> {
    let window = line_byte(rope, lines.start)..line_byte(rope, lines.end);

    cursor.set_byte_range(window.clone());
    let mut captures = Vec::new();
    let mut stream = cursor.captures(highlights.query, tree.root_node(), RopeText(rope));
    while let Some((query_match, capture_index)) = stream.next() {
        let capture = query_match.captures()[*capture_index];
        captures.push(Capture {
            start: capture.node.start_byte(),
            end: capture.node.end_byte(),
            node: capture.node.id(),
            match_id: query_match.id(),
            index: capture.index,
        });
    }

    let runs = styled_runs(&captures, window.clone(), highlights, styles, default_style);
    split_into_lines(rope, lines, &runs)
}

/// Turn the ordered capture stream into styled byte runs covering `window`.
///
/// Follows `tree_sitter_highlight::HighlightIter` for one layer without
/// locals: of the captures on one node the last pattern wins and the
/// overruled matches are dropped (`QueryMatch::remove`); a highlight closes
/// once the next capture starts at or after its end.
fn styled_runs(
    captures: &[Capture],
    window: Range<usize>,
    highlights: &QueryHighlights,
    styles: &dyn Fn(usize) -> Style,
    default_style: Style,
) -> Vec<(Range<usize>, Style)> {
    let mut runs: Vec<(Range<usize>, Style)> = Vec::new();
    let mut pos = window.start;
    let mut advance = |runs: &mut Vec<(Range<usize>, Style)>, to: usize, style: Style| {
        let to = to.min(window.end);
        if to > pos {
            runs.push((pos..to, style));
            pos = to;
        }
    };
    let top = |stack: &[(usize, Style)]| stack.last().map_or(default_style, |(_, s)| *s);

    let mut stack: Vec<(usize, Style)> = Vec::new();
    let mut removed: HashSet<u32> = HashSet::new();
    let mut i = 0;
    while i < captures.len() {
        let capture = captures[i];
        if removed.contains(&capture.match_id) {
            i += 1;
            continue;
        }
        if let Some(&(end, style)) = stack.last() {
            if end <= capture.start {
                advance(&mut runs, end, style);
                stack.pop();
                continue;
            }
        }
        i += 1;
        let mut current = capture;
        loop {
            while i < captures.len() && removed.contains(&captures[i].match_id) {
                i += 1;
            }
            match captures.get(i) {
                Some(next) if next.node == current.node => {
                    removed.insert(current.match_id);
                    current = *next;
                    i += 1;
                }
                _ => break,
            }
        }
        if let Some(Some(highlight)) = highlights.capture_highlights.get(current.index as usize) {
            advance(&mut runs, current.start, top(&stack));
            stack.push((current.end, styles(*highlight)));
        }
    }
    while let Some((end, style)) = stack.pop() {
        advance(&mut runs, end, style);
    }
    advance(&mut runs, window.end, default_style);
    runs
}

/// Cut styled runs at rope line boundaries, dropping each line's `\n`.
pub(crate) fn split_into_lines(
    rope: &Rope,
    lines: Range<usize>,
    runs: &[(Range<usize>, Style)],
) -> Vec<LineSegments> {
    let mut result = Vec::with_capacity(lines.len());
    let mut run = 0;
    for line in lines {
        let slice = rope.line(line);
        let start = rope.line_to_byte(line);
        let text: Cow<str> = slice.into();
        let text = text.strip_suffix('\n').unwrap_or(&text);
        let end = start + text.len();
        let mut segments: LineSegments = Vec::new();
        while run < runs.len() && runs[run].0.end <= start {
            run += 1;
        }
        let mut r = run;
        while r < runs.len() && runs[r].0.start < end {
            let (range, style) = &runs[r];
            let from = range.start.max(start) - start;
            let to = range.end.min(end) - start;
            if let Some(piece) = text.get(from..to).filter(|p| !p.is_empty()) {
                segments.push((Cow::Owned(piece.to_string()), *style));
            }
            r += 1;
        }
        result.push(segments);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_highlights_resume_the_enclosing_style() {
        let outer = Style::default().fg(ratatui::style::Color::Red);
        let inner = Style::default().fg(ratatui::style::Color::Blue);
        let caps = [
            Capture {
                start: 0,
                end: 10,
                node: 1,
                match_id: 1,
                index: 0,
            },
            Capture {
                start: 3,
                end: 5,
                node: 2,
                match_id: 2,
                index: 1,
            },
        ];
        let query = Query::new(&tree_sitter_rust::LANGUAGE.into(), "").unwrap();
        let highlights = QueryHighlights {
            name: "rust",
            language: &tree_sitter_rust::LANGUAGE.into(),
            query: &query,
            capture_highlights: &[Some(0), Some(1)],
            injection_patterns: 0,
            combined_injections: None,
            content_capture: None,
            language_capture: None,
        };
        let styles = |h: usize| if h == 0 { outer } else { inner };
        let runs = styled_runs(&caps, 0..12, &highlights, &styles, Style::default());
        assert_eq!(
            runs,
            [
                (0..3, outer),
                (3..5, inner),
                (5..10, outer),
                (10..12, Style::default())
            ]
        );
    }
}
