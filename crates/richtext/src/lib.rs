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
/// line that opens `url`. A link wrapped over several lines, or split by its
/// styling, is several regions sharing one `id`.
#[derive(Debug, Clone)]
pub struct LinkSpan {
    pub line: usize,
    pub start: u16,
    pub end: u16,
    pub url: String,
    /// Which link of the document the region belongs to.
    pub id: usize,
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

/// One character of a table cell with its style and optional link id.
type Glyph = (char, Style, Option<usize>);

/// One visual line of a laid-out table cell: its styled spans, the display
/// width they occupy, and `[start, end)` column ranges (relative to the cell
/// content) that link to `url_id`.
struct RenderedCell {
    spans: Vec<Span<'static>>,
    used: usize,
    links: Vec<(usize, usize, usize)>,
}

impl RenderedCell {
    fn empty() -> Self {
        Self {
            spans: Vec::new(),
            used: 0,
            links: Vec::new(),
        }
    }
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

    /// Append text whose bare web addresses become links, as GitHub's
    /// autolinks do — outside code and outside a link already open. With
    /// `restyle` an address takes the link style; without, it keeps the
    /// text's own and is only clickable.
    pub fn text_autolinked(&mut self, text: &str, restyle: bool) {
        if self.in_code || self.cur_link.is_some() {
            self.text(text);
            return;
        }
        let mut at = 0;
        for range in termide_core::links::url_ranges(text) {
            if range.start > at {
                self.push_text(&text[at..range.start]);
            }
            let url = &text[range.clone()];
            self.cur_link = self.add_url(url.to_string());
            let style = if restyle {
                self.link_style()
            } else {
                self.cur_style()
            };
            self.push_span(url, style);
            self.cur_link = None;
            at = range.end;
        }
        if at < text.len() {
            self.push_text(&text[at..]);
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
        let mark = if done { "[✓] " } else { "[ ] " };
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

    /// Draw the collected table with box-drawing borders. Columns are sized
    /// to their content (see [`column_widths`]) and cells word-wrap to the
    /// column width, so a row grows taller instead of clipping its text.
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
        // Per column: the longest word (narrowest width that wraps without
        // breaking words) and the whole unwrapped content.
        let mut min = vec![1usize; ncols];
        let mut max = vec![1usize; ncols];
        for row in &t.rows {
            for (c, cell) in row.iter().enumerate() {
                let words = cell_words(cell, Modifier::empty());
                let widths: Vec<usize> = words.iter().map(|w| glyphs_width(w)).collect();
                let longest = widths.iter().copied().max().unwrap_or(0);
                let full = widths.iter().sum::<usize>() + widths.len().saturating_sub(1);
                min[c] = min[c].max(longest);
                max[c] = max[c].max(full);
            }
        }
        let overhead = 3 * ncols + 1;
        let budget = self.width.saturating_sub(overhead).max(ncols);
        let widths = column_widths(&min, &max, budget);

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
            let mut cells: Vec<Vec<RenderedCell>> = widths
                .iter()
                .enumerate()
                .map(|(c, w)| self.wrap_cell(row.get(c).unwrap_or(&empty), *w, header))
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1);
            for cell in &mut cells {
                cell.reverse();
            }
            for _ in 0..height {
                let line_idx = self.lines.len();
                let mut spans: Vec<Span<'static>> = vec![Span::styled("│", dis)];
                // Column offset of the next span; the leading `│` occupies col 0.
                let mut col = 1usize;
                for (cell, w) in cells.iter_mut().zip(&widths) {
                    let rendered = cell.pop().unwrap_or_else(RenderedCell::empty);
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
            }
            if header && t.header_rows > 0 && ri + 1 == t.header_rows {
                self.lines.push(border("├", "┼", "┤", "─"));
            }
        }
        self.lines.push(border("└", "┴", "┘", "─"));
    }

    /// Word-wrap a table cell's styled fragments to `width` columns, one
    /// [`RenderedCell`] per visual line (at least one). Header cells are
    /// bolded; per-fragment styles and link hit-areas are preserved. Runs of
    /// whitespace collapse to one space; a word wider than the column is
    /// broken mid-word.
    fn wrap_cell(&self, cell: &[Word], width: usize, header: bool) -> Vec<RenderedCell> {
        let extra = if header {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };
        let width = width.max(1);
        let mut lines: Vec<Vec<Glyph>> = Vec::new();
        let mut cur: Vec<Glyph> = Vec::new();
        let mut cur_w = 0usize;
        for word in cell_words(cell, extra) {
            let ww = glyphs_width(&word);
            if cur_w > 0 && cur_w + 1 + ww > width {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            if cur_w > 0 {
                cur.push(self.gap(cur.last(), word.first(), extra));
                cur_w += 1;
            }
            for g in word {
                let gw = char_width(g.0);
                if cur_w > 0 && cur_w + gw > width {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push(g);
                cur_w += gw;
            }
        }
        lines.push(cur);
        lines.iter().map(|l| self.render_glyphs(l)).collect()
    }

    /// The space joining two words of a wrapped cell: it keeps the style and
    /// link of its neighbours when both share them, so a multi-word link stays
    /// one continuous underlined hit-area.
    fn gap(&self, prev: Option<&Glyph>, next: Option<&Glyph>, extra: Modifier) -> Glyph {
        match (prev, next) {
            (Some(&(_, ps, pl)), Some(&(_, ns, nl))) if ps == ns && pl == nl => (' ', ps, pl),
            _ => (' ', self.base_style().add_modifier(extra), None),
        }
    }

    /// Coalesce one visual line of styled glyphs into spans, with link ranges
    /// as `[start, end)` column offsets relative to the start of the line.
    fn render_glyphs(&self, glyphs: &[Glyph]) -> RenderedCell {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut buf = String::new();
        let mut buf_style = self.base_style();
        let mut used = 0usize;
        // Open link run: (start_col, url_id). Spans coalesce by style only, so
        // link tracking is kept separate from span buffering.
        let mut links: Vec<(usize, usize, usize)> = Vec::new();
        let mut open: Option<(usize, usize)> = None;
        for &(ch, style, link) in glyphs {
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
            used += char_width(ch);
        }
        if let Some((s, cur)) = open {
            links.push((s, used, cur));
        }
        if !buf.is_empty() {
            spans.push(Span::styled(buf, buf_style));
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

        for word in words {
            let ww: usize = word.iter().map(|(text, _, _)| text.width()).sum();
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
            for (text, style, link) in word {
                let start = cur_prefix_w + cur_text_w;
                let w = text.width();
                if let Some(url_id) = link {
                    self.pending_links.push(PendingLink {
                        line: base + out.len(),
                        start: start as u16,
                        end: (start + w) as u16,
                        url_id,
                    });
                }
                cur.push(Span::styled(text, style));
                cur_text_w += w;
            }
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
                id: p.url_id,
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

/// Split styled runs into space-delimited words, each the fragments it is
/// made of with their style and link: runs with no space between them form
/// one word (`[docs](u).`, `**bold**ly`), so wrapping never inserts one.
fn split_words(runs: &[Word]) -> Vec<Vec<Word>> {
    let mut words = Vec::new();
    let mut cur: Vec<Word> = Vec::new();
    for (text, style, link) in runs {
        for (i, piece) in text.split(' ').enumerate() {
            // A space before this piece ends the word; a run that starts
            // without one carries on the word the previous run left open.
            if i > 0 && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            if !piece.is_empty() {
                cur.push((piece.to_string(), *style, *link));
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn prefix_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Split a table cell's fragments into whitespace-delimited words of styled
/// glyphs. Unlike [`split_words`], a word may span fragments (`**a**b` is one
/// word), so wrapping never inserts a space inside it.
fn cell_words(cell: &[Word], extra: Modifier) -> Vec<Vec<Glyph>> {
    let mut words = Vec::new();
    let mut cur: Vec<Glyph> = Vec::new();
    for (text, style, link) in cell {
        let style = style.add_modifier(extra);
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push((ch, style, *link));
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn glyphs_width(glyphs: &[Glyph]) -> usize {
    glyphs.iter().map(|g| char_width(g.0)).sum()
}

/// Content-driven table column widths within `budget` (all columns together,
/// borders excluded; `budget >= min.len()`). `min[c]` is the column's longest
/// word, `max[c]` its unwrapped content (`min <= max`, both at least 1).
///
/// - Everything fits unwrapped: each column gets its full content.
/// - The longest words fit: each column keeps its longest word and the spare
///   width goes to the columns in proportion to how much they still wrap, so
///   short columns stay narrow and long prose takes the room.
/// - Not even the longest words fit: the budget is shared in proportion to
///   them and long words break mid-word.
fn column_widths(min: &[usize], max: &[usize], budget: usize) -> Vec<usize> {
    let total_max: usize = max.iter().sum();
    if total_max <= budget {
        return max.to_vec();
    }
    let total_min: usize = min.iter().sum();
    if total_min <= budget {
        let slack: Vec<usize> = max.iter().zip(min).map(|(x, n)| x - n).collect();
        return distribute(min, &slack, budget - total_min);
    }
    let ones = vec![1; min.len()];
    let weights: Vec<usize> = min.iter().map(|n| n - 1).collect();
    distribute(&ones, &weights, budget.saturating_sub(min.len()))
}

/// `base[i]` plus a share of `spare` proportional to `weights[i]`, rounding
/// by largest remainder so the shares add up to exactly `spare` (when
/// `spare <= sum(weights)`, no column gets more than its weight).
fn distribute(base: &[usize], weights: &[usize], spare: usize) -> Vec<usize> {
    let total: usize = weights.iter().sum();
    if total == 0 {
        return base.to_vec();
    }
    let mut out: Vec<usize> = base
        .iter()
        .zip(weights)
        .map(|(b, w)| b + w * spare / total)
        .collect();
    let given: usize = weights.iter().map(|w| w * spare / total).sum();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(weights[i] * spare % total));
    for &i in order.iter().take(spare - given) {
        out[i] += 1;
    }
    out
}

/// Display width of a single character (best-effort).
fn char_width(ch: char) -> usize {
    ch.to_string().width()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(r: &Rendered) -> Vec<String> {
        r.lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn runs_without_a_space_between_stay_one_word() {
        let colors = ThemeColors::default();
        let mut b = Builder::new(80, &colors, false);
        b.text("see ");
        b.start_link("https://a.b".into());
        b.text("docs");
        b.end_link();
        b.text(". and ");
        b.start_strong();
        b.text("bold");
        b.pop_style();
        b.text("ly");
        b.end_paragraph();
        let r = b.finish();
        assert_eq!(text_of(&r)[0], "see docs. and boldly");
        assert_eq!((r.links[0].start, r.links[0].end), (4, 8));
    }

    #[test]
    fn a_word_made_of_runs_wraps_whole() {
        let colors = ThemeColors::default();
        let mut b = Builder::new(10, &colors, false);
        b.text("aaaaa ");
        b.start_link("https://a.b".into());
        b.text("bbbb");
        b.end_link();
        b.text("!");
        b.end_paragraph();
        let r = b.finish();
        assert_eq!(text_of(&r)[..2], ["aaaaa", "bbbb!"]);
        assert_eq!(
            (r.links[0].line, r.links[0].start, r.links[0].end),
            (1, 0, 4)
        );
    }

    #[test]
    fn bare_addresses_become_links_outside_code() {
        let colors = ThemeColors::default();
        let mut b = Builder::new(80, &colors, false);
        b.text_autolinked("see https://a.b/c. or ", true);
        b.inline_code("https://not.a/link");
        b.end_paragraph();
        let r = b.finish();
        assert_eq!(text_of(&r)[0], "see https://a.b/c. or https://not.a/link");
        assert_eq!(r.links.len(), 1, "{:?}", r.links);
        assert_eq!(r.links[0].url, "https://a.b/c");
        assert_eq!((r.links[0].start, r.links[0].end), (4, 17));
    }

    #[test]
    fn columns_take_full_content_when_it_fits() {
        assert_eq!(column_widths(&[3, 5], &[10, 20], 40), vec![10, 20]);
    }

    #[test]
    fn spare_width_goes_to_columns_that_wrap() {
        // A short column keeps its content; the long one takes the rest.
        let w = column_widths(&[2, 6], &[2, 100], 30);
        assert_eq!(w, vec![2, 28]);
        // Slack 18 and 54 share 24 spare columns 1:3.
        let w = column_widths(&[2, 6], &[20, 60], 32);
        assert_eq!(w, vec![8, 24]);
    }

    #[test]
    fn widths_fill_budget_exactly_and_respect_bounds() {
        let (min, max) = ([3, 4, 5], [9, 17, 30]);
        for budget in 12..56 {
            let w = column_widths(&min, &max, budget);
            assert_eq!(w.iter().sum::<usize>(), budget, "{budget}: {w:?}");
            for c in 0..3 {
                assert!(min[c] <= w[c] && w[c] <= max[c], "{budget}: {w:?}");
            }
        }
    }

    #[test]
    fn overlong_words_share_budget_proportionally() {
        let w = column_widths(&[10, 30], &[10, 30], 20);
        assert_eq!(w.iter().sum::<usize>(), 20);
        assert!(w[0] >= 1 && w[0] < w[1], "{w:?}");
        // Degenerate budget: every column still gets a cell.
        assert_eq!(column_widths(&[10, 30], &[10, 30], 2), vec![1, 1]);
    }
}
