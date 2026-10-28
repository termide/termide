//! Line-based syntax highlighting cache with an optional context-aware
//! whole-document pass, plus the [`LineHighlighter`] integration trait.

use ratatui::style::{Color, Style};
use ropey::Rope;
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use tree_sitter::InputEdit;
use tree_sitter_highlight::{HighlightEvent, Highlighter};

use crate::document::{DocumentHighlight, DocumentRevision, DocumentSource};
use crate::keyword::{keyword_line_segments, KeywordSyntax};
use crate::languages::{injection_language_alias, HIGHLIGHT_NAMES};
use crate::TreeSitterHighlighter;

/// Maximum highlight cache size (lines)
const MAX_CACHE_SIZE: usize = 1000;

/// Upper bound (in bytes) for whole-document syntax highlighting.
///
/// Whole-document highlighting is the only way to resolve cross-line context
/// (PHP's HTML/PHP mode switches, multi-line strings and comments). Edits are
/// followed incrementally, but opening a file still parses it whole. Past
/// this size callers fall back to the per-line path.
pub const WHOLE_DOCUMENT_BYTE_LIMIT: usize = 1024 * 1024;

/// Trait for line-based syntax highlighting.
/// Allows custom highlighters (e.g., for log files) to integrate with Editor.
pub trait LineHighlighter: Send + Sync {
    /// Get highlighted segments for a line (with caching).
    ///
    /// Segment text is returned as `Cow<str>` so callers that don't need
    /// highlighting (fallback path) can avoid per-frame `String` allocations
    /// by passing a borrowed slice of `line_text` directly.
    fn get_line_segments<'a>(
        &'a mut self,
        line_idx: usize,
        line_text: &'a str,
    ) -> &'a [(Cow<'a, str>, Style)];

    /// Invalidate cache from given line to end (called when text changes).
    fn invalidate_from(&mut self, line: usize);

    /// Invalidate entire cache.
    fn invalidate_all(&mut self);

    /// Check if syntax highlighting is active.
    fn has_syntax(&self) -> bool;

    /// Bring a context-aware whole-document highlight up to date with `doc`
    /// before line segments are requested, making sure lines
    /// `visible_lines` are highlighted.
    ///
    /// Called before every render; cheap when nothing changed. Implementations
    /// that highlight per line ignore it. Callers must gate invocation on
    /// buffer size (see [`WHOLE_DOCUMENT_BYTE_LIMIT`]). Default: no-op.
    fn sync_document(&mut self, _doc: &dyn DocumentSource, _visible_lines: Range<usize>) {}
}

/// Highlighted lines cache for incremental highlighting.
pub struct HighlightCache {
    /// Highlighted lines: line number -> (vector of segments, last access time)
    #[allow(clippy::type_complexity)]
    lines: HashMap<usize, (Vec<(Cow<'static, str>, Style)>, u64)>,
    /// Current language
    language: Option<String>,
    /// Global SyntaxHighlighter (static)
    syntax_highlighter: &'static TreeSitterHighlighter,
    /// Light or dark theme
    is_light_theme: bool,
    /// Access counter for LRU
    access_counter: u64,
    /// Default foreground color for unstyled text (from theme.fg)
    default_fg: Color,
    /// The whole-document highlight: per-line segments, and for languages
    /// highlighted incrementally the syntax tree they come from. Filled by
    /// [`HighlightCache::sync_document`]; its lines are empty or stale when
    /// the document path is unused (no syntax, oversized buffer).
    document: DocumentHighlight,
    /// Style per highlight index for the current theme.
    highlight_styles: Vec<Style>,
    /// Active config-driven keyword highlighter (set for extensions without a
    /// tree-sitter grammar). Mutually exclusive with `language`.
    custom: Option<KeywordSyntax>,
}

impl HighlightCache {
    /// Create a new cache.
    pub fn new(
        syntax_highlighter: &'static TreeSitterHighlighter,
        is_light_theme: bool,
        default_fg: Color,
    ) -> Self {
        Self {
            lines: HashMap::new(),
            language: None,
            syntax_highlighter,
            is_light_theme,
            access_counter: 0,
            default_fg,
            document: DocumentHighlight::new(),
            highlight_styles: highlight_styles(syntax_highlighter, is_light_theme),
            custom: None,
        }
    }

    /// Set (or clear) the config-driven keyword highlighter. Used for file
    /// extensions that have no tree-sitter grammar. Clears the tree-sitter
    /// language so the two never fight.
    pub fn set_custom_syntax(&mut self, syntax: Option<KeywordSyntax>) {
        let changed = match (&self.custom, &syntax) {
            (Some(a), Some(b)) => a.name != b.name,
            (None, None) => false,
            _ => true,
        };
        if !changed {
            return;
        }
        if syntax.is_some() {
            self.language = None;
        }
        self.custom = syntax;
        self.invalidate_all();
        self.invalidate_document();
    }

    /// Drop the whole-document highlight, tree included. Called when the
    /// syntax changes so the next sync parses from scratch.
    fn invalidate_document(&mut self) {
        self.document.reset();
    }

    /// Set syntax (by language name).
    pub fn set_syntax(&mut self, language_name: &str) {
        if self.language.as_deref() == Some(language_name) {
            return;
        }

        if self.syntax_highlighter.get_config(language_name).is_some() {
            self.language = Some(language_name.to_string());
            self.custom = None; // tree-sitter wins over the keyword highlighter
            self.invalidate_all();
            self.invalidate_document();
        }
    }

    /// Set syntax by file extension.
    pub fn set_syntax_from_path(&mut self, path: &Path) {
        if let Some(language) = self.syntax_highlighter.language_for_file(path) {
            self.set_syntax(language);
        }
    }

    /// Get line highlighting (with caching).
    pub fn get_line_segments<'a>(
        &'a mut self,
        line_idx: usize,
        line_text: &'a str,
    ) -> &'a [(Cow<'a, str>, Style)] {
        // Whole-document fast path: when a context-aware pass is current and its
        // cached line still reconstructs this exact text, serve it directly. The
        // text check guards against any line-index drift (CRLF, trailing
        // newline, inline-diff rows) by falling back to the per-line path.
        // The condition is evaluated to a bool first so the immutable borrow is
        // released before the conditional re-borrow on `return` (NLL).
        let doc_hit = self
            .document
            .lines
            .get(line_idx)
            .and_then(Option::as_ref)
            .is_some_and(|segments| Self::segments_match(segments, line_text));
        if doc_hit {
            return self.document.lines[line_idx]
                .as_deref()
                .expect("checked just above");
        }

        self.access_counter += 1;

        if let Some((_, access_time)) = self.lines.get_mut(&line_idx) {
            *access_time = self.access_counter;
        } else {
            let segments = self.compute_line_segments(line_text);

            if self.lines.len() >= MAX_CACHE_SIZE {
                self.evict_lru();
            }

            self.lines.insert(line_idx, (segments, self.access_counter));
        }

        &self
            .lines
            .get(&line_idx)
            .expect("line was just inserted or updated above")
            .0
    }

    /// Bring the whole-document highlight up to date with `doc` and highlight
    /// whatever of `visible_lines` is stale.
    ///
    /// The per-line path parses each line in isolation, which cannot resolve
    /// tokens whose meaning spans lines: PHP files switch between HTML and PHP
    /// at `<?php`/`?>`, and strings/comments routinely run across line breaks.
    /// The document path keeps that context, so each line is coloured
    /// according to the real parse state at that point.
    ///
    /// The syntax tree is kept between syncs: the edits since the last one
    /// are applied to it and the text re-parsed reusing what they left
    /// intact. Embedded languages keep a tree per injected range the same
    /// way. Only visible lines are highlighted, and only those that were
    /// edited or whose syntax may have changed.
    ///
    /// No-op when no tree-sitter syntax is set; when the parse fails the
    /// per-line path serves the lines. Callers must gate on buffer size.
    pub fn sync_document(&mut self, doc: &dyn DocumentSource, visible_lines: Range<usize>) {
        let Some(language) = self.language.as_deref() else {
            return;
        };
        let highlighter = self.syntax_highlighter;
        let Some(highlights) = highlighter.incremental_highlights(language) else {
            return;
        };
        let default_style = Style::default().fg(self.default_fg);
        self.document.sync_tree(doc, &highlights);
        let styles = &self.highlight_styles;
        let style = |highlight: usize| styles.get(highlight).copied().unwrap_or(default_style);
        // Embedded languages resolve to their own queries under the alias
        // the host's injection names.
        let resolve =
            |name: &str| highlighter.incremental_highlights(injection_language_alias(name));
        self.document.fill(
            doc.rope(),
            visible_lines,
            &highlights,
            &resolve,
            &style,
            default_style,
        );
    }

    /// Highlight `text` as a whole document, every line at once. For text
    /// that is not a buffer being edited (a code block in rendered Markdown).
    pub fn set_document(&mut self, text: &str) {
        let doc = WholeText::new(text);
        self.sync_document(&doc, 0..doc.rope.len_lines());
    }

    /// Highlight the whole of `text` through `tree_sitter_highlight`, which
    /// also colours injected languages: the reference the incremental path
    /// is tested against. `None` when no syntax is set or the grammar/parse
    /// fails.
    #[cfg(test)]
    fn highlight_whole_text(&self, text: &str) -> Option<Vec<crate::document::LineSegments>> {
        let language = self.language.as_ref()?;
        let config = self.syntax_highlighter.get_config(language)?;

        let default_style = Style::default().fg(self.default_fg);
        let mut highlighter = Highlighter::new();
        let source = text.as_bytes();

        // Resolve embedded languages (e.g. the HTML in a PHP template, or CSS/JS
        // inside HTML) to their loaded configs so injected regions are coloured
        // too. The highlighter is `'static`, so the borrowed configs outlive the
        // pass. Unknown injection languages simply stay unhighlighted.
        let highlighter_ref = self.syntax_highlighter;
        let events = highlighter
            .highlight(config, source, None, None, |name| {
                highlighter_ref.get_config(injection_language_alias(name))
            })
            .ok()?;

        let mut doc: Vec<crate::document::LineSegments> = Vec::new();
        let mut current_line: crate::document::LineSegments = Vec::new();
        // Highlights nest; when an inner one ends the enclosing one resumes.
        let mut styles: Vec<Style> = Vec::new();

        for event in events {
            match event.ok()? {
                HighlightEvent::Source { start, end } => {
                    let Ok(chunk) = std::str::from_utf8(&source[start..end]) else {
                        continue;
                    };
                    let current_style = styles.last().copied().unwrap_or(default_style);
                    // tree-sitter emits Source for every byte in order, so the
                    // chunks concatenate back to the whole document. Split on
                    // '\n' to distribute each chunk across the lines it spans;
                    // a style that straddles a newline (e.g. a block comment)
                    // is carried onto the continuation line.
                    let mut rest = chunk;
                    while let Some(nl) = rest.find('\n') {
                        let piece = &rest[..nl];
                        if !piece.is_empty() {
                            current_line.push((Cow::Owned(piece.to_string()), current_style));
                        }
                        doc.push(std::mem::take(&mut current_line));
                        rest = &rest[nl + 1..];
                    }
                    if !rest.is_empty() {
                        current_line.push((Cow::Owned(rest.to_string()), current_style));
                    }
                }
                HighlightEvent::HighlightStart(highlight) => {
                    styles.push(
                        self.highlight_styles
                            .get(highlight.0)
                            .copied()
                            .unwrap_or(default_style),
                    );
                }
                HighlightEvent::HighlightEnd => {
                    styles.pop();
                }
            }
        }
        // The text after the final newline (or the whole buffer if it has none).
        doc.push(current_line);
        Some(doc)
    }

    /// True when `segments` concatenate to exactly `line_text`. Used to confirm
    /// a cached whole-document line still matches what the renderer is drawing
    /// before serving it.
    fn segments_match(segments: &[(Cow<'_, str>, Style)], line_text: &str) -> bool {
        let mut rest = line_text;
        for (text, _) in segments {
            match rest.strip_prefix(text.as_ref()) {
                Some(remainder) => rest = remainder,
                None => return false,
            }
        }
        rest.is_empty()
    }

    /// Compute highlighting for line.
    fn compute_line_segments(&self, line_text: &str) -> Vec<(Cow<'static, str>, Style)> {
        let default_style = Style::default().fg(self.default_fg);

        // Config-driven keyword highlighter (extensions without a grammar).
        if let Some(ref syntax) = self.custom {
            let default_fg = self.default_fg;
            let is_light = self.is_light_theme;
            let hl = self.syntax_highlighter;
            let style = |name: &str| {
                if name.is_empty() {
                    Style::default().fg(default_fg)
                } else {
                    hl.style_for_name(name, is_light)
                }
            };
            let segs = keyword_line_segments(line_text, syntax, &style);
            return if segs.is_empty() {
                vec![(Cow::Owned(line_text.to_string()), default_style)]
            } else {
                segs
            };
        }

        let Some(ref language) = self.language else {
            return vec![(Cow::Owned(line_text.to_string()), default_style)];
        };

        let Some(config) = self.syntax_highlighter.get_config(language) else {
            return vec![(Cow::Owned(line_text.to_string()), default_style)];
        };

        let mut highlighter = Highlighter::new();
        let source = line_text.as_bytes();

        let highlights = match highlighter.highlight(config, source, None, None, |_| None) {
            Ok(h) => h,
            Err(_) => return vec![(Cow::Owned(line_text.to_string()), default_style)],
        };

        let mut segments = Vec::new();
        let mut current_style = default_style;
        let mut current_text = String::new();

        for event in highlights {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    if let Ok(text) = std::str::from_utf8(&source[start..end]) {
                        current_text.push_str(text);
                    }
                }
                Ok(HighlightEvent::HighlightStart(highlight)) => {
                    if !current_text.is_empty() {
                        // Use take() instead of clone() + clear() to avoid allocation
                        segments
                            .push((Cow::Owned(std::mem::take(&mut current_text)), current_style));
                    }
                    current_style = self
                        .syntax_highlighter
                        .style_for_highlight(highlight.0, self.is_light_theme);
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    if !current_text.is_empty() {
                        // Use take() instead of clone() + clear() to avoid allocation
                        segments
                            .push((Cow::Owned(std::mem::take(&mut current_text)), current_style));
                    }
                    current_style = default_style;
                }
                Err(_) => {
                    return vec![(Cow::Owned(line_text.to_string()), default_style)];
                }
            }
        }

        if !current_text.is_empty() {
            segments.push((Cow::Owned(current_text), current_style));
        }

        if segments.is_empty() {
            vec![(Cow::Owned(line_text.to_string()), default_style)]
        } else {
            segments
        }
    }

    /// Remove oldest entries from cache (LRU).
    ///
    /// Uses partial sort (select_nth_unstable) for O(n) performance instead of O(n log n).
    fn evict_lru(&mut self) {
        let evict_count = MAX_CACHE_SIZE / 5;

        let mut entries: Vec<(usize, u64)> = self
            .lines
            .iter()
            .map(|(line_idx, (_, access_time))| (*line_idx, *access_time))
            .collect();

        if entries.len() <= evict_count {
            return;
        }

        // Partial sort: elements before evict_count are the smallest (oldest)
        // This is O(n) on average vs O(n log n) for full sort
        entries.select_nth_unstable_by_key(evict_count, |(_, access_time)| *access_time);

        // Remove the oldest entries (those before the partition point)
        for (line_idx, _) in entries.iter().take(evict_count) {
            self.lines.remove(line_idx);
        }
    }

    /// Invalidate line (when editing).
    ///
    /// The whole-document highlight follows the buffer's own edits (see
    /// [`HighlightCache::sync_document`]), so it is left alone here.
    pub fn invalidate_line(&mut self, line_idx: usize) {
        self.lines.remove(&line_idx);
    }

    /// Invalidate line range.
    pub fn invalidate_range(&mut self, start_line: usize, end_line: usize) {
        for idx in start_line..=end_line {
            self.lines.remove(&idx);
        }
    }

    /// Invalidate entire cache. The whole-document highlight keeps its tree
    /// but highlights every line again.
    pub fn invalidate_all(&mut self) {
        self.lines.clear();
        self.document.clear_lines();
    }

    /// Change theme (light/dark).
    pub fn set_light_theme(&mut self, is_light: bool) {
        if self.is_light_theme != is_light {
            self.is_light_theme = is_light;
            self.highlight_styles = highlight_styles(self.syntax_highlighter, is_light);
            self.invalidate_all();
        }
    }

    /// Set default foreground color for unstyled text.
    /// This color is used instead of Style::default() to ensure text is visible
    /// on both light and dark theme backgrounds.
    pub fn set_default_fg(&mut self, fg: Color) {
        if self.default_fg != fg {
            self.default_fg = fg;
            self.invalidate_all();
        }
    }

    /// Check if syntax is set (tree-sitter grammar or keyword highlighter).
    pub fn has_syntax(&self) -> bool {
        self.language.is_some() || self.custom.is_some()
    }

    /// Get current syntax name.
    pub fn current_syntax(&self) -> Option<&str> {
        self.custom
            .as_ref()
            .map(|c| c.name.as_str())
            .or(self.language.as_deref())
    }
}

impl LineHighlighter for HighlightCache {
    fn get_line_segments<'a>(
        &'a mut self,
        line_idx: usize,
        line_text: &'a str,
    ) -> &'a [(Cow<'a, str>, Style)] {
        HighlightCache::get_line_segments(self, line_idx, line_text)
    }

    fn invalidate_from(&mut self, line: usize) {
        let lines_to_remove: Vec<usize> =
            self.lines.keys().filter(|&&l| l >= line).copied().collect();
        for line_idx in lines_to_remove {
            self.lines.remove(&line_idx);
        }
    }

    fn invalidate_all(&mut self) {
        HighlightCache::invalidate_all(self);
    }

    fn has_syntax(&self) -> bool {
        HighlightCache::has_syntax(self)
    }

    fn sync_document(&mut self, doc: &dyn DocumentSource, visible_lines: Range<usize>) {
        HighlightCache::sync_document(self, doc, visible_lines);
    }
}

/// Style per highlight index, for `is_light_theme`.
fn highlight_styles(highlighter: &TreeSitterHighlighter, is_light_theme: bool) -> Vec<Style> {
    (0..HIGHLIGHT_NAMES.len())
        .map(|i| highlighter.style_for_highlight(i, is_light_theme))
        .collect()
}

/// Text highlighted once as a whole, outside any buffer. Every instance is a
/// new revision of a document no buffer uses, so a sync parses it afresh.
struct WholeText {
    rope: Rope,
    revision: DocumentRevision,
}

impl WholeText {
    fn new(text: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            rope: Rope::from_str(text),
            revision: DocumentRevision {
                // Buffers number their documents from 1.
                document: 0,
                edit: NEXT.fetch_add(1, Ordering::Relaxed),
            },
        }
    }
}

impl DocumentSource for WholeText {
    fn rope(&self) -> &Rope {
        &self.rope
    }

    fn revision(&self) -> DocumentRevision {
        self.revision
    }

    fn edits_since(&self, _since: DocumentRevision) -> Option<Vec<InputEdit>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_highlighter;

    /// Highlight a single line and report how many distinct styled segments it
    /// produces. A line that is recognized by the grammar yields more than one
    /// segment; an unhighlighted (plain-text) line yields exactly one.
    fn segment_count(language: &str, line: &str) -> usize {
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax(language);
        assert_eq!(
            cache.current_syntax(),
            Some(language),
            "language {language} should have a loaded config"
        );
        cache.get_line_segments(0, line).len()
    }

    #[test]
    fn php_is_highlighted_in_document() {
        // Regression: PHP was disabled by an ABI-incompatible grammar. PHP uses
        // the template grammar (HTML↔PHP), so a statement only resolves with the
        // surrounding context the whole-document pass provides.
        let text = "<?php\n$count = 1; // comment\n";
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        cache.set_document(text);
        assert!(
            styled_on_line(&mut cache, 1, "$count = 1; // comment") > 0,
            "PHP statement should be highlighted in a document"
        );
    }

    #[test]
    fn kotlin_line_is_highlighted() {
        // Kotlin ships no bundled highlights query; this guards the hand-written
        // KOTLIN_HIGHLIGHTS against a grammar/ABI regression silently disabling it.
        assert!(
            segment_count("kotlin", "fun area(r: Double): Double = 0.0") > 1,
            "Kotlin line should produce multiple highlighted segments"
        );
    }

    #[test]
    fn lua_line_is_highlighted() {
        assert!(
            segment_count("lua", "local function area(r) return r * r end") > 1,
            "Lua line should produce multiple highlighted segments"
        );
    }

    #[test]
    fn jsx_line_is_highlighted() {
        // Regression: jsx was advertised but never loaded.
        assert!(
            segment_count("jsx", "const x = <Foo bar={1} />;") > 1,
            "JSX line should produce multiple highlighted segments"
        );
    }

    /// Count segments whose style differs from the default foreground — i.e.
    /// genuinely highlighted spans on a given line of the cached document.
    fn styled_on_line(cache: &mut HighlightCache, line_idx: usize, line_text: &str) -> usize {
        cache
            .get_line_segments(line_idx, line_text)
            .iter()
            .filter(|(_, style)| style.fg != Some(Color::White))
            .count()
    }

    #[test]
    fn php_document_highlights_both_html_and_php() {
        // Regression: a mixed HTML/PHP template (the common .php file) only
        // highlighted its PHP lines under the per-line path. The whole-document
        // pass must colour the surrounding HTML too.
        let lines = [
            "<!DOCTYPE html>",
            "<html lang=\"ru\">",
            "<body>",
            "    <h1>Title</h1>",
            "    <?php",
            "        $name = \"Ivan\";",
            "        echo \"<p>Hi, $name</p>\";",
            "    ?>",
            "</body>",
            "</html>",
        ];
        let text = lines.join("\n");

        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        cache.set_document(&text);

        // HTML tag line — highlighted only by the whole-document pass.
        assert!(
            styled_on_line(&mut cache, 1, lines[1]) > 0,
            "HTML line should be highlighted in a mixed PHP document"
        );
        // PHP statement inside the <?php block.
        assert!(
            styled_on_line(&mut cache, 5, lines[5]) > 0,
            "PHP line should be highlighted in a mixed PHP document"
        );
    }

    #[test]
    fn document_line_text_mismatch_falls_back() {
        // The whole-document cache must never render stale text: if the line the
        // renderer asks for no longer matches the cached segments, the per-line
        // path serves the correct text instead.
        let text = "<?php\n$a = 1;\n";
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        cache.set_document(text);

        // Ask for a line whose text differs from the cached document line.
        let segs = cache.get_line_segments(1, "$totally = different;");
        let rebuilt: String = segs.iter().map(|(t, _)| t.as_ref()).collect();
        assert_eq!(rebuilt, "$totally = different;");
    }

    #[test]
    fn a_theme_change_rehighlights_a_document_with_injections() {
        // Clearing the lines keeps the trees, host and embedded: the next
        // sync must highlight the lines again rather than leave them empty.
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        let doc = WholeText::new("<?php\n$a = 1;\n");
        cache.sync_document(&doc, 0..3);
        assert!(cache.document.lines[1].is_some());
        cache.set_light_theme(true);
        assert!(cache.document.lines[1].is_none());
        cache.sync_document(&doc, 0..3);
        assert!(cache.document.lines[1].is_some());
    }
}

#[cfg(test)]
#[path = "cache_incremental_tests.rs"]
mod incremental_tests;
