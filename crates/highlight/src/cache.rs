//! Line-based syntax highlighting cache with an optional context-aware
//! whole-document pass, plus the [`LineHighlighter`] integration trait.

use ratatui::style::{Color, Style};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use tree_sitter_highlight::{HighlightEvent, Highlighter};

use crate::keyword::{keyword_line_segments, KeywordSyntax};
use crate::languages::injection_language_alias;
use crate::TreeSitterHighlighter;

/// Maximum highlight cache size (lines)
const MAX_CACHE_SIZE: usize = 1000;

/// Upper bound (in bytes) for whole-document syntax highlighting.
///
/// Whole-document highlighting re-parses the entire buffer on every edit, which
/// is the only way to resolve cross-line context (PHP's HTML/PHP mode switches,
/// multi-line strings and comments). Past this size the cost per keystroke is no
/// longer worth it, so callers fall back to the per-line path.
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

    /// Whether a whole-document highlight pass is pending and applicable.
    ///
    /// When `true`, the caller should hand the full buffer text to
    /// [`LineHighlighter::set_document`] before requesting line segments so
    /// cross-line context resolves correctly. Default: never needed.
    fn needs_document(&self) -> bool {
        false
    }

    /// Provide the full buffer text for a context-aware whole-document highlight.
    ///
    /// Implementations that highlight per line ignore this. Callers must gate
    /// invocation on buffer size (see [`WHOLE_DOCUMENT_BYTE_LIMIT`]) to keep the
    /// per-edit cost bounded. Default: no-op.
    fn set_document(&mut self, _text: &str) {}
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
    /// Per-line segments from the last whole-document highlight pass, indexed by
    /// line number. Populated by [`HighlightCache::set_document`]; empty when the
    /// document path is unused (no syntax, oversized buffer, or stale).
    #[allow(clippy::type_complexity)]
    doc_segments: Vec<Vec<(Cow<'static, str>, Style)>>,
    /// Whether `doc_segments` reflects the current buffer/syntax/theme.
    doc_valid: bool,
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
            doc_segments: Vec::new(),
            doc_valid: false,
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

    /// Drop any cached whole-document highlight. Called whenever the buffer,
    /// syntax or theme changes so the next render rebuilds it.
    fn invalidate_document(&mut self) {
        self.doc_valid = false;
        self.doc_segments.clear();
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
        let doc_hit = self.doc_valid
            && self
                .doc_segments
                .get(line_idx)
                .is_some_and(|segments| Self::segments_match(segments, line_text));
        if doc_hit {
            return &self.doc_segments[line_idx];
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

    /// Whether a whole-document highlight pass should be (re)built before the
    /// next render. True only when a syntax is active and the cached pass is
    /// stale; the size guard lives at the call site (see
    /// [`WHOLE_DOCUMENT_BYTE_LIMIT`]).
    pub fn needs_document(&self) -> bool {
        self.language.is_some() && !self.doc_valid
    }

    /// Highlight the entire buffer in one context-aware pass and cache the
    /// result per line.
    ///
    /// The per-line path parses each line in isolation, which cannot resolve
    /// tokens whose meaning spans lines: PHP files switch between HTML and PHP
    /// at `<?php`/`?>`, and strings/comments routinely run across line breaks.
    /// A single whole-buffer parse keeps that context, so each line is coloured
    /// according to the real parse state at that point.
    ///
    /// No-op when no syntax is set or the grammar/parse fails (the per-line
    /// path then serves plain text). Callers must gate on buffer size.
    pub fn set_document(&mut self, text: &str) {
        self.doc_segments.clear();
        self.doc_valid = false;

        let Some(ref language) = self.language else {
            return;
        };
        let Some(config) = self.syntax_highlighter.get_config(language) else {
            return;
        };

        let default_style = Style::default().fg(self.default_fg);
        let mut highlighter = Highlighter::new();
        let source = text.as_bytes();

        // Resolve embedded languages (e.g. the HTML in a PHP template, or CSS/JS
        // inside HTML) to their loaded configs so injected regions are coloured
        // too. The highlighter is `'static`, so the borrowed configs outlive the
        // pass. Unknown injection languages simply stay unhighlighted.
        let highlighter_ref = self.syntax_highlighter;
        let events = match highlighter.highlight(config, source, None, |name| {
            highlighter_ref.get_config(injection_language_alias(name))
        }) {
            Ok(events) => events,
            Err(_) => return,
        };

        let mut doc: Vec<Vec<(Cow<'static, str>, Style)>> = Vec::new();
        let mut current_line: Vec<(Cow<'static, str>, Style)> = Vec::new();
        let mut current_style = default_style;

        for event in events {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    let Ok(chunk) = std::str::from_utf8(&source[start..end]) else {
                        continue;
                    };
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
                Ok(HighlightEvent::HighlightStart(highlight)) => {
                    current_style = self
                        .syntax_highlighter
                        .style_for_highlight(highlight.0, self.is_light_theme);
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    current_style = default_style;
                }
                Err(_) => {
                    self.doc_segments.clear();
                    return;
                }
            }
        }
        // The text after the final newline (or the whole buffer if it has none).
        doc.push(current_line);

        self.doc_segments = doc;
        self.doc_valid = true;
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

        let highlights = match highlighter.highlight(config, source, None, |_| None) {
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
    pub fn invalidate_line(&mut self, line_idx: usize) {
        self.lines.remove(&line_idx);
        self.invalidate_document();
    }

    /// Invalidate line range.
    pub fn invalidate_range(&mut self, start_line: usize, end_line: usize) {
        for idx in start_line..=end_line {
            self.lines.remove(&idx);
        }
        self.invalidate_document();
    }

    /// Invalidate entire cache.
    pub fn invalidate_all(&mut self) {
        self.lines.clear();
        self.invalidate_document();
    }

    /// Change theme (light/dark).
    pub fn set_light_theme(&mut self, is_light: bool) {
        if self.is_light_theme != is_light {
            self.is_light_theme = is_light;
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
        self.invalidate_document();
    }

    fn invalidate_all(&mut self) {
        HighlightCache::invalidate_all(self);
    }

    fn has_syntax(&self) -> bool {
        HighlightCache::has_syntax(self)
    }

    fn needs_document(&self) -> bool {
        HighlightCache::needs_document(self)
    }

    fn set_document(&mut self, text: &str) {
        HighlightCache::set_document(self, text);
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
        assert!(
            cache.needs_document(),
            "fresh syntax should request a document pass"
        );
        cache.set_document(&text);
        assert!(
            !cache.needs_document(),
            "document pass should satisfy the request"
        );

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
    fn editing_invalidates_document_cache() {
        let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
        cache.set_syntax("php");
        cache.set_document("<?php\n$a = 1;\n");
        assert!(!cache.needs_document());
        cache.invalidate_line(1);
        assert!(
            cache.needs_document(),
            "an edit must trigger a fresh document pass"
        );
    }
}
