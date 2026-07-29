//! Parser-agnostic rich-text layout engine.
//!
//! [`Builder`] turns a stream of semantic block/inline calls into owned
//! `ratatui` [`Line`]s wrapped to a target width. It owns all the layout
//! concerns shared by the Markdown and HTML renderers: inline wrapping,
//! list/quote indentation, box-drawn tables, syntax-highlighted code blocks,
//! and link hit-area recording. Front-ends (a `pulldown-cmark` event adapter,
//! an `html5ever` token adapter) drive it through the semantic methods; they
//! do not touch layout state directly.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use termide_core::ThemeColors;
use termide_highlight::{global_highlighter, HighlightCache};
use unicode_width::UnicodeWidthStr;

/// One styled word (no internal spaces), with an optional link id.
type Word = (String, Style, Option<usize>);

/// A table cell: styled fragments in order of arrival, each carrying an
/// optional link id (same shape as a paragraph's inline runs).
type Cell = Vec<Word>;

/// A clickable region: a half-open `[start, end)` column range on a rendered
/// line that opens `url`.
#[derive(Debug, Clone)]
pub struct LinkSpan {
    pub line: usize,
    pub start: u16,
    pub end: u16,
    pub url: String,
}

/// Rendered document: wrapped lines plus link hit-areas.
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<LinkSpan>,
    /// Named anchors (`id`/`name` / heading slug) → target line index, for
    /// fragment navigation (`#section`).
    pub anchors: Vec<(String, usize)>,
}

/// A pending table being collected between its start and end.
struct Table {
    aligns: usize,
    rows: Vec<Vec<Cell>>,
    header_rows: usize,
    cur_row: Vec<Cell>,
}

/// Output of laying out a single table cell: its styled spans, the display
/// width they occupy, and `[start, end)` column ranges (relative to the cell
/// content) that link to `url_id`.
struct RenderedCell {
    spans: Vec<Span<'static>>,
    used: usize,
    links: Vec<(usize, usize, usize)>,
}

/// A link span recorded during wrapping, before url ids are resolved.
struct PendingLink {
    line: usize,
    start: u16,
    end: u16,
    url_id: usize,
}

/// Layout engine. Construct, drive with the semantic methods, then [`finish`].
///
/// [`finish`]: Builder::finish
pub struct Builder<'c> {
    width: usize,
    colors: &'c ThemeColors,
    is_light: bool,

    lines: Vec<Line<'static>>,
    /// Inline runs accumulated for the current block (paragraph/heading/item).
    runs: Vec<Word>,
    /// Emphasis/code/link styling, innermost last.
    style_stack: Vec<Style>,

    /// List nesting; `None` = bullet, `Some(n)` = next ordinal for ordered list.
    list_stack: Vec<Option<u64>>,
    /// Block-quote nesting depth.
    quote_depth: usize,

    /// When inside a table cell, text is captured here instead of `runs`.
    table: Option<Table>,
    in_cell: bool,

    /// Verbatim text of the code block currently being collected.
    code_buf: String,
    code_lang: String,
    in_code: bool,

    /// Pending list-item marker applied to the first wrapped line.
    item_marker: Option<(Vec<Span<'static>>, String)>,

    /// Link/image URLs; words carry an index into this table.
    urls: Vec<String>,
    /// The link id active for the inline text currently being emitted.
    cur_link: Option<usize>,
    /// Recorded link hit-areas (url ids resolved in `finish`).
    pending_links: Vec<PendingLink>,
    /// Named anchors → target line index.
    anchors: Vec<(String, usize)>,
}

impl<'c> Builder<'c> {
    #[must_use]
    pub fn new(width: u16, colors: &'c ThemeColors, is_light: bool) -> Self {
        Self {
            width: (width as usize).max(1),
            colors,
            is_light,
            lines: Vec::new(),
            runs: Vec::new(),
            style_stack: Vec::new(),
            list_stack: Vec::new(),
            quote_depth: 0,
            table: None,
            in_cell: false,
            code_buf: String::new(),
            code_lang: String::new(),
            in_code: false,
            item_marker: None,
            urls: Vec::new(),
            cur_link: None,
            pending_links: Vec::new(),
            anchors: Vec::new(),
        }
    }

    // --- accessors for front-ends that compute their own styles -------------

    /// Index of the next line that will be emitted (anchor target position).
    #[must_use]
    pub fn current_line(&self) -> usize {
        self.lines.len()
    }

    /// Register a named anchor pointing at line `line`.
    pub fn add_anchor_at(&mut self, id: String, line: usize) {
        if !id.is_empty() {
            self.anchors.push((id, line));
        }
    }

    /// Register a named anchor at the next line to be emitted.
    pub fn add_anchor(&mut self, id: String) {
        let line = self.lines.len();
        self.add_anchor_at(id, line);
    }

    #[must_use]
    pub fn colors(&self) -> &ThemeColors {
        self.colors
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn is_light(&self) -> bool {
        self.is_light
    }

    #[must_use]
    pub fn base_style(&self) -> Style {
        Style::default().fg(self.colors.fg)
    }

    /// Current inline style (innermost emphasis/link), or the base style.
    #[must_use]
    pub fn cur_style(&self) -> Style {
        self.style_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.base_style())
    }

    pub fn push_style(&mut self, style: Style) {
        self.style_stack.push(style);
    }

    pub fn pop_style(&mut self) {
        self.style_stack.pop();
    }

    // --- inline content -----------------------------------------------------

    /// Append text. Routed to the open code block, table cell, or inline runs.
    pub fn text(&mut self, text: &str) {
        if self.in_code {
            self.code_buf.push_str(text);
        } else {
            self.push_text(text);
        }
    }

    /// Inline `code` span (monospace-styled run).
    pub fn inline_code(&mut self, text: &str) {
        let style = Style::default().fg(self.colors.success);
        self.push_span(text, style);
    }

    /// Append a literal styled run without word-splitting (markers, glyphs).
    pub fn styled(&mut self, text: impl Into<String>, style: Style) {
        self.push_span(text, style);
    }

    /// Soft line break: collapses to a space.
    pub fn soft_break(&mut self) {
        self.push_text(" ");
    }

    /// Hard line break: flush the current inline run onto its own line.
    pub fn hard_break(&mut self) {
        let prefix = self.context_prefix();
        self.flush_inline(prefix.clone(), prefix);
    }

    /// Task-list checkbox marker.
    pub fn task_marker(&mut self, done: bool) {
        let mark = if done { "[x] " } else { "[ ] " };
        self.push_span(mark, Style::default().fg(self.colors.info));
    }

    /// Horizontal rule across the full width.
    pub fn rule(&mut self) {
        self.push_blank();
        self.lines.push(Line::styled(
            "─".repeat(self.width),
            Style::default().fg(self.colors.disabled),
        ));
        self.push_blank();
    }

    /// Blank separator line (collapses consecutive blanks).
    pub fn blank(&mut self) {
        self.push_blank();
    }

    // --- blocks -------------------------------------------------------------

    pub fn start_heading(&mut self, depth: usize) {
        self.push_blank();
        let weight = Style::default()
            .fg(self.colors.info)
            .add_modifier(Modifier::BOLD);
        let hashes = "#".repeat(depth.clamp(1, 6));
        self.push_span(
            format!("{hashes} "),
            Style::default().fg(self.colors.disabled),
        );
        self.style_stack.push(weight);
    }

    pub fn end_heading(&mut self) {
        self.style_stack.pop();
        let prefix = self.context_prefix();
        self.flush_inline(prefix.clone(), prefix);
        self.push_blank();
    }

    pub fn end_paragraph(&mut self) {
        let prefix = self.context_prefix();
        self.flush_inline(prefix.clone(), prefix);
        if self.quote_depth == 0 && self.list_stack.is_empty() {
            self.push_blank();
        }
    }

    /// Flush the current inline run and separate it with a blank line, without
    /// the paragraph's list/quote suppression. Used for raw HTML blocks.
    pub fn flush_block(&mut self) {
        let prefix = self.context_prefix();
        self.flush_inline(prefix.clone(), prefix);
        self.push_blank();
    }

    pub fn start_quote(&mut self) {
        self.push_blank();
        self.quote_depth += 1;
    }

    pub fn end_quote(&mut self) {
        self.quote_depth = self.quote_depth.saturating_sub(1);
        if self.quote_depth == 0 {
            self.push_blank();
        }
    }

    pub fn start_code_block(&mut self, lang: &str) {
        self.push_blank();
        self.in_code = true;
        self.code_lang = lang.to_string();
        self.code_buf.clear();
    }

    pub fn end_code_block(&mut self) {
        self.in_code = false;
        self.flush_code_block();
        self.push_blank();
    }

    /// Start a list. `ordered_start` is `Some(first_ordinal)` for ordered
    /// lists, `None` for bullets.
    pub fn start_list(&mut self, ordered_start: Option<u64>) {
        if !self.runs.is_empty() || self.item_marker.is_some() {
            let prefix = self.context_prefix();
            self.flush_inline(prefix.clone(), prefix);
        }
        self.list_stack.push(ordered_start);
    }

    pub fn end_list(&mut self) {
        self.list_stack.pop();
        if self.list_stack.is_empty() {
            self.push_blank();
        }
    }

    pub fn start_item(&mut self) {
        let prefix = self.context_prefix();
        let marker = match self.list_stack.last_mut() {
            Some(Some(n)) => {
                let m = format!("{n}. ");
                *n += 1;
                m
            }
            _ => "• ".to_string(),
        };
        self.item_marker = Some((prefix, marker));
    }

    pub fn end_item(&mut self) {
        if !self.runs.is_empty() || self.item_marker.is_some() {
            let prefix = self.context_prefix();
            self.flush_inline(prefix.clone(), prefix);
        }
    }

    pub fn start_emphasis(&mut self) {
        let s = self.cur_style().add_modifier(Modifier::ITALIC);
        self.style_stack.push(s);
    }

    pub fn start_strong(&mut self) {
        let s = self.cur_style().add_modifier(Modifier::BOLD);
        self.style_stack.push(s);
    }

    pub fn start_strike(&mut self) {
        let s = self.cur_style().add_modifier(Modifier::CROSSED_OUT);
        self.style_stack.push(s);
    }

    pub fn start_link(&mut self, url: String) {
        self.cur_link = self.add_url(url);
        let s = self.link_style();
        self.style_stack.push(s);
    }

    pub fn end_link(&mut self) {
        self.style_stack.pop();
        self.cur_link = None;
    }

    /// Clickable image pictogram; the following text is the alt label, ended
    /// with [`end_link`](Builder::end_link).
    pub fn start_image(&mut self, url: String) {
        self.cur_link = self.add_url(url);
        let s = self.link_style();
        self.push_span("🖼 ", s);
        self.style_stack.push(s);
    }

    // --- tables -------------------------------------------------------------

    pub fn start_table(&mut self, ncols: usize) {
        self.table = Some(Table {
            aligns: ncols,
            rows: Vec::new(),
            header_rows: 0,
            cur_row: Vec::new(),
        });
        self.push_blank();
    }

    pub fn start_table_head(&mut self) {
        if let Some(t) = self.table.as_mut() {
            t.cur_row = Vec::new();
        }
    }

    pub fn start_table_row(&mut self) {
        if let Some(t) = self.table.as_mut() {
            t.cur_row = Vec::new();
        }
    }

    pub fn start_table_cell(&mut self) {
        if let Some(t) = self.table.as_mut() {
            t.cur_row.push(Cell::new());
        }
        self.in_cell = true;
    }

    pub fn end_table_cell(&mut self) {
        self.in_cell = false;
    }

    pub fn end_table_head(&mut self) {
        if let Some(t) = self.table.as_mut() {
            let row = std::mem::take(&mut t.cur_row);
            t.rows.push(row);
            t.header_rows = 1;
        }
    }

    pub fn end_table_row(&mut self) {
        if let Some(t) = self.table.as_mut() {
            let row = std::mem::take(&mut t.cur_row);
            t.rows.push(row);
        }
    }

    pub fn end_table(&mut self) {
        self.flush_table();
        self.push_blank();
    }

    // --- internals ----------------------------------------------------------

    fn push_text(&mut self, text: &str) {
        let style = self.cur_style();
        if self.push_cell(text, style) {
            return;
        }
        let link = self.cur_link;
        self.runs.push((text.to_string(), style, link));
    }

    fn push_span(&mut self, text: impl Into<String>, style: Style) {
        let text = text.into();
        if self.push_cell(&text, style) {
            return;
        }
        let link = self.cur_link;
        self.runs.push((text, style, link));
    }

    /// Route inline text into the open table cell, if any, preserving its
    /// style and link id so cell formatting and hit-areas survive to
    /// `flush_table`; returns true when consumed by a cell.
    fn push_cell(&mut self, text: &str, style: Style) -> bool {
        if !self.in_cell {
            return false;
        }
        let link = self.cur_link;
        if let Some(t) = self.table.as_mut() {
            if let Some(cell) = t.cur_row.last_mut() {
                cell.push((text.to_string(), style, link));
            }
        }
        true
    }

    fn push_blank(&mut self) {
        if self.lines.last().is_some_and(|l| l.spans.is_empty()) {
            return;
        }
        self.lines.push(Line::default());
    }

    /// Indentation prefix for the current list/quote context.
    fn context_prefix(&self) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for _ in 0..self.quote_depth {
            spans.push(Span::styled(
                "│ ",
                Style::default().fg(self.colors.disabled),
            ));
        }
        let indent = self.list_stack.len().saturating_sub(1) * 2;
        if indent > 0 {
            spans.push(Span::raw(" ".repeat(indent)));
        }
        spans
    }

    fn link_style(&self) -> Style {
        Style::default()
            .fg(self.colors.info)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn add_url(&mut self, url: String) -> Option<usize> {
        if url.is_empty() {
            return None;
        }
        self.urls.push(url);
        Some(self.urls.len() - 1)
    }

    /// Render the captured code block with per-line syntax highlighting.
    fn flush_code_block(&mut self) {
        let code = std::mem::take(&mut self.code_buf);
        let lang = std::mem::take(&mut self.code_lang);

        // A ```mermaid block renders as the diagram itself (reusing the shared
        // mermaid crate); unsupported diagram kinds fall through to code.
        if lang.eq_ignore_ascii_case("mermaid") {
            if let Some(diagram) = termide_mermaid::render_to_lines(&code) {
                let bar = Style::default().fg(self.colors.disabled);
                for line in diagram {
                    let spans = vec![
                        Span::styled("┊ ", bar),
                        Span::styled(line, self.base_style()),
                    ];
                    self.lines.push(Line::from(spans));
                }
                return;
            }
        }

        let mut cache = HighlightCache::new(global_highlighter(), self.is_light, self.colors.fg);
        if !lang.is_empty() {
            cache.set_syntax(&lang);
            if !cache.has_syntax() {
                cache.set_syntax_from_path(std::path::Path::new(&format!("code.{lang}")));
            }
        }
        if cache.has_syntax() {
            cache.set_document(&code);
        }
        let bar = Style::default().fg(self.colors.disabled);
        for (i, line) in code.lines().enumerate() {
            let mut spans: Vec<Span<'static>> = vec![Span::styled("┊ ", bar)];
            if cache.has_syntax() {
                for (text, style) in cache.get_line_segments(i, line) {
                    spans.push(Span::styled(text.to_string(), *style));
                }
            } else {
                spans.push(Span::styled(line.to_string(), self.base_style()));
            }
            self.lines.push(Line::from(spans));
        }
    }

    /// Draw the collected table with box-drawing borders.
    fn flush_table(&mut self) {
        let Some(t) = self.table.take() else { return };
        if t.rows.is_empty() {
            return;
        }
        let ncols = t
            .aligns
            .max(t.rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if ncols == 0 {
            return;
        }
        let mut widths = vec![0usize; ncols];
        for row in &t.rows {
            for (c, cell) in row.iter().enumerate() {
                widths[c] = widths[c].max(cell_width(cell));
            }
        }
        for w in &mut widths {
            *w = (*w).max(1);
        }
        let overhead = 3 * ncols + 1;
        let budget = self.width.saturating_sub(overhead).max(ncols);
        let total: usize = widths.iter().sum();
        if total > budget {
            for w in &mut widths {
                let scaled = (*w * budget) / total.max(1);
                *w = scaled.max(1);
            }
        }

        let dis = Style::default().fg(self.colors.disabled);
        let border = |left: &str, mid: &str, right: &str, fill: &str| -> Line<'static> {
            let mut s = String::from(left);
            for (i, w) in widths.iter().enumerate() {
                s.push_str(&fill.repeat(w + 2));
                s.push_str(if i + 1 == widths.len() { right } else { mid });
            }
            Line::styled(s, dis)
        };

        self.lines.push(border("┌", "┬", "┐", "─"));
        let empty: Cell = Vec::new();
        for (ri, row) in t.rows.iter().enumerate() {
            let header = ri < t.header_rows;
            let line_idx = self.lines.len();
            let mut spans: Vec<Span<'static>> = vec![Span::styled("│", dis)];
            // Column offset of the next span; the leading `│` occupies col 0.
            let mut col = 1usize;
            for (c, w) in widths.iter().enumerate() {
                let cell = row.get(c).unwrap_or(&empty);
                let rendered = self.render_cell(cell, *w, header);
                spans.push(Span::raw(" "));
                col += 1;
                let cell_base = col;
                spans.extend(rendered.spans);
                col += rendered.used;
                for (s, e, url_id) in rendered.links {
                    self.pending_links.push(PendingLink {
                        line: line_idx,
                        start: (cell_base + s) as u16,
                        end: (cell_base + e) as u16,
                        url_id,
                    });
                }
                let pad = w.saturating_sub(rendered.used);
                spans.push(Span::raw(" ".repeat(pad + 1)));
                col += pad + 1;
                spans.push(Span::styled("│", dis));
                col += 1;
            }
            self.lines.push(Line::from(spans));
            if header && t.header_rows > 0 && ri + 1 == t.header_rows {
                self.lines.push(border("├", "┼", "┤", "─"));
            }
        }
        self.lines.push(border("└", "┴", "┘", "─"));
    }

    /// Render a table cell's styled fragments clipped to `width` columns,
    /// bolding header cells and preserving per-fragment styles (inline code,
    /// emphasis) and link hit-areas. Outer whitespace is trimmed; an ellipsis
    /// marks truncation. Link ranges are `[start, end)` column offsets relative
    /// to the start of the cell content, clamped to what actually rendered.
    fn render_cell(&self, cell: &[Word], width: usize, header: bool) -> RenderedCell {
        let extra = if header {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };
        // Flatten to styled chars so trimming and clipping can span fragments.
        let mut chars: Vec<(char, Style, Option<usize>)> = Vec::new();
        for (text, style, link) in cell {
            let style = style.add_modifier(extra);
            for ch in text.chars() {
                chars.push((ch, style, *link));
            }
        }
        let start = chars
            .iter()
            .position(|(c, _, _)| !c.is_whitespace())
            .unwrap_or(chars.len());
        let end = chars
            .iter()
            .rposition(|(c, _, _)| !c.is_whitespace())
            .map_or(start, |i| i + 1);
        let slice = &chars[start..end];

        let total: usize = slice.iter().map(|(c, _, _)| char_width(*c)).sum();
        let (limit, ellipsis) = if total > width {
            (width.saturating_sub(1), true)
        } else {
            (width, false)
        };

        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut buf = String::new();
        let mut buf_style = self.base_style();
        let mut used = 0usize;
        // Open link run: (start_col, url_id). Spans coalesce by style only, so
        // link tracking is kept separate from span buffering.
        let mut links: Vec<(usize, usize, usize)> = Vec::new();
        let mut open: Option<(usize, usize)> = None;
        for (ch, style, link) in slice.iter().copied() {
            let cw = char_width(ch);
            if used + cw > limit {
                break;
            }
            if !buf.is_empty() && style != buf_style {
                spans.push(Span::styled(std::mem::take(&mut buf), buf_style));
            }
            if buf.is_empty() {
                buf_style = style;
            }
            match (link, open) {
                (Some(id), Some((_, cur))) if cur == id => {}
                (Some(id), _) => {
                    if let Some((s, cur)) = open {
                        links.push((s, used, cur));
                    }
                    open = Some((used, id));
                }
                (None, Some((s, cur))) => {
                    links.push((s, used, cur));
                    open = None;
                }
                (None, None) => {}
            }
            buf.push(ch);
            used += cw;
        }
        if let Some((s, cur)) = open {
            links.push((s, used, cur));
        }
        if !buf.is_empty() {
            spans.push(Span::styled(buf, buf_style));
        }
        if ellipsis {
            spans.push(Span::styled("…", self.base_style().add_modifier(extra)));
            used += 1;
        }
        RenderedCell { spans, used, links }
    }

    /// Wrap accumulated inline runs to width and append as lines, applying the
    /// prefixes (first line vs continuation) and recording link hit-areas.
    fn flush_inline(&mut self, first_prefix: Vec<Span<'static>>, cont_prefix: Vec<Span<'static>>) {
        let (first_prefix, cont_prefix) = if let Some((pfx, marker)) = self.item_marker.take() {
            let mut first = pfx.clone();
            first.push(Span::styled(
                marker.clone(),
                Style::default().fg(self.colors.info),
            ));
            let mut cont = pfx;
            cont.push(Span::raw(" ".repeat(marker.width())));
            (first, cont)
        } else {
            (first_prefix, cont_prefix)
        };

        let runs = std::mem::take(&mut self.runs);
        if runs.is_empty() {
            return;
        }
        let words = split_words(&runs);
        if words.is_empty() {
            return;
        }

        let first_w = prefix_width(&first_prefix);
        let cont_w = prefix_width(&cont_prefix);

        let base = self.lines.len();
        let mut out: Vec<Line<'static>> = Vec::new();
        let mut cur: Vec<Span<'static>> = first_prefix;
        let mut cur_text_w = 0usize;
        let mut cur_prefix_w = first_w;
        let mut avail = self.width.saturating_sub(first_w).max(1);

        for (text, style, link) in words {
            let ww = text.width();
            if cur_text_w > 0 && cur_text_w + 1 + ww > avail {
                out.push(Line::from(std::mem::take(&mut cur)));
                cur = cont_prefix.clone();
                cur_text_w = 0;
                cur_prefix_w = cont_w;
                avail = self.width.saturating_sub(cont_w).max(1);
            }
            if cur_text_w > 0 {
                cur.push(Span::raw(" "));
                cur_text_w += 1;
            }
            let start = cur_prefix_w + cur_text_w;
            if let Some(url_id) = link {
                self.pending_links.push(PendingLink {
                    line: base + out.len(),
                    start: start as u16,
                    end: (start + ww) as u16,
                    url_id,
                });
            }
            cur.push(Span::styled(text, style));
            cur_text_w += ww;
        }
        out.push(Line::from(cur));
        self.lines.extend(out);
    }

    #[must_use]
    pub fn finish(mut self) -> Rendered {
        while self.lines.last().is_some_and(|l| l.spans.is_empty()) {
            self.lines.pop();
        }
        let line_count = self.lines.len();
        let links = self
            .pending_links
            .into_iter()
            .filter(|p| p.line < line_count)
            .map(|p| LinkSpan {
                line: p.line,
                start: p.start,
                end: p.end,
                url: self.urls[p.url_id].clone(),
            })
            .collect();
        // Clamp anchors that fell past the trimmed end to the last line.
        let last = line_count.saturating_sub(1);
        let anchors = self
            .anchors
            .into_iter()
            .map(|(id, line)| (id, line.min(last)))
            .collect();
        Rendered {
            lines: self.lines,
            links,
            anchors,
        }
    }
}

/// Split styled runs into whitespace-delimited words, preserving style + link.
fn split_words(runs: &[Word]) -> Vec<Word> {
    let mut words = Vec::new();
    for (text, style, link) in runs {
        for piece in text.split(' ') {
            if piece.is_empty() {
                continue;
            }
            words.push((piece.to_string(), *style, *link));
        }
    }
    words
}

fn prefix_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Trimmed display width of a table cell's concatenated text.
fn cell_width(cell: &[Word]) -> usize {
    let text: String = cell.iter().map(|(t, _, _)| t.as_str()).collect();
    text.trim().width()
}

/// Display width of a single character (best-effort).
fn char_width(ch: char) -> usize {
    ch.to_string().width()
}
