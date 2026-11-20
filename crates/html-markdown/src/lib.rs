//! HTML to markdown: the text of a page with its structure
//! (headings, paragraphs, lists, code, tables, links) and without what only a
//! browser needs (scripts, styles, forms) or what repeats on every page of a
//! site (navigation, sidebars, footers).
//!
//! Driven by the `html5ever` tokenizer, no DOM: a page's `<main>` or single
//! `<article>` is kept when there is one, else the whole `<body>`.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use url::Url;

/// A converted page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub title: String,
    pub markdown: String,
}

/// Convert `html`, resolving relative links against `base` (a `<base href>`
/// in the page wins).
#[must_use]
pub fn html_to_markdown(html: &str, base: &str) -> Converted {
    let tokens = tokenize(html);
    let mut base = Url::parse(base).ok();
    let mut title = String::new();
    let mut in_title = false;
    for token in &tokens {
        match token {
            Tok::Open(tag) if tag.name == "base" => {
                if let Some(href) = tag.attr("href") {
                    base = match &base {
                        Some(current) => current.join(href).ok(),
                        None => Url::parse(href).ok(),
                    }
                    .or(base);
                }
            }
            Tok::Open(tag) if tag.name == "title" => in_title = true,
            Tok::Close(name) if name == "title" => in_title = false,
            Tok::Text(text) if in_title => title.push_str(text),
            _ => {}
        }
    }

    let scope = content_scope(&tokens);
    let mut writer = Writer::new(base);
    for token in &tokens[scope.0..scope.1] {
        writer.token(token);
    }
    Converted {
        title: collapse_spaces(&title).trim().to_string(),
        markdown: writer.finish(),
    }
}

/// The token range worth converting: the first `<main>`, else the only
/// `<article>`, else everything.
fn content_scope(tokens: &[Tok]) -> (usize, usize) {
    let range_of = |name: &str, start: usize| -> Option<(usize, usize)> {
        let mut depth = 0usize;
        for (index, token) in tokens.iter().enumerate().skip(start) {
            match token {
                Tok::Open(tag) if tag.name == name && !tag.self_closing => depth += 1,
                Tok::Close(close) if close == name => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((start, index + 1));
                    }
                }
                _ => {}
            }
        }
        Some((start, tokens.len()))
    };
    let opens = |name: &str| -> Vec<usize> {
        tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| matches!(token, Tok::Open(tag) if tag.name == name))
            .map(|(index, _)| index)
            .collect()
    };
    if let Some(&start) = opens("main").first() {
        if let Some(range) = range_of("main", start) {
            return range;
        }
    }
    if let [start] = opens("article")[..] {
        if let Some(range) = range_of("article", start) {
            return range;
        }
    }
    (0, tokens.len())
}

#[derive(Debug, Clone)]
struct OpenTag {
    name: String,
    attrs: Vec<(String, String)>,
    self_closing: bool,
}

impl OpenTag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone)]
enum Tok {
    Open(OpenTag),
    Close(String),
    Text(String),
}

struct Sink(RefCell<Vec<Tok>>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut out = self.0.borrow_mut();
        match token {
            Token::TagToken(Tag {
                kind,
                name,
                self_closing,
                attrs,
                ..
            }) => {
                let name = name.to_string();
                match kind {
                    TagKind::StartTag => out.push(Tok::Open(OpenTag {
                        name,
                        attrs: attrs
                            .into_iter()
                            .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                            .collect(),
                        self_closing,
                    })),
                    TagKind::EndTag => out.push(Tok::Close(name)),
                }
            }
            Token::CharacterTokens(text) => match out.last_mut() {
                Some(Tok::Text(existing)) => existing.push_str(&text),
                _ => out.push(Tok::Text(text.to_string())),
            },
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn tokenize(html: &str) -> Vec<Tok> {
    let tokenizer = Tokenizer::new(Sink(RefCell::new(Vec::new())), TokenizerOpts::default());
    let queue = BufferQueue::default();
    queue.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    tokenizer.sink.0.take()
}

/// Elements whose whole content is dropped.
const SKIPPED: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "canvas", "iframe", "object", "head", "nav",
    "aside", "footer", "form", "button", "select", "textarea", "dialog",
];

/// Elements that are never closed.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Elements that start a block of their own.
const BLOCKS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "figure",
    "figcaption",
    "address",
    "details",
    "summary",
    "dl",
    "dt",
    "dd",
    "center",
    "hgroup",
];

struct ListLevel {
    ordered: bool,
    next: usize,
}

struct Writer {
    base: Option<Url>,
    out: String,
    /// Pending inline text of the current block, whitespace already collapsed.
    line: String,
    skip_depth: usize,
    skip_name: Option<String>,
    pre_depth: usize,
    lists: Vec<ListLevel>,
    /// Text of the prefix to put before the next line (list bullet, quote).
    item_prefix: Option<String>,
    quote_depth: usize,
    links: Vec<(Option<String>, usize)>,
    table: Option<Table>,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    header_rows: usize,
    cell: Option<String>,
    /// Inside `<thead>`.
    in_thead: bool,
    /// The current row is a header row (inside `<thead>`, or a first row
    /// made of `<th>`).
    head_row: bool,
}

impl Writer {
    fn new(base: Option<Url>) -> Self {
        Self {
            base,
            out: String::new(),
            line: String::new(),
            skip_depth: 0,
            skip_name: None,
            pre_depth: 0,
            lists: Vec::new(),
            item_prefix: None,
            quote_depth: 0,
            links: Vec::new(),
            table: None,
        }
    }

    fn token(&mut self, token: &Tok) {
        if self.skip_depth > 0 {
            match token {
                Tok::Open(tag)
                    if Some(&tag.name) == self.skip_name.as_ref() && !tag.self_closing =>
                {
                    self.skip_depth += 1;
                }
                Tok::Close(name) if Some(name) == self.skip_name.as_ref() => {
                    self.skip_depth -= 1;
                    if self.skip_depth == 0 {
                        self.skip_name = None;
                    }
                }
                _ => {}
            }
            return;
        }
        match token {
            Tok::Text(text) => self.text(text),
            Tok::Open(tag) => self.open(tag),
            Tok::Close(name) => self.close(name),
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(table) = &mut self.table {
            if let Some(cell) = &mut table.cell {
                push_collapsed(cell, text);
            }
            return;
        }
        if self.pre_depth > 0 {
            self.line.push_str(text);
            return;
        }
        push_collapsed(&mut self.line, text);
    }

    fn open(&mut self, tag: &OpenTag) {
        let name = tag.name.as_str();
        if SKIPPED.contains(&name) && !tag.self_closing {
            self.skip_depth = 1;
            self.skip_name = Some(tag.name.clone());
            return;
        }
        if tag.attr("hidden").is_some() || tag.attr("aria-hidden") == Some("true") {
            if !VOID.contains(&name) && !tag.self_closing {
                self.skip_depth = 1;
                self.skip_name = Some(tag.name.clone());
            }
            return;
        }
        if self.table.is_some() {
            self.table_open(name);
            return;
        }
        // Inside preformatted text only line structure matters: highlighters
        // wrap each line of code in a block element of its own.
        if self.pre_depth > 0 && name != "pre" && name != "code" {
            if name == "br" || BLOCKS.contains(&name) {
                self.pre_newline();
            }
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush_block();
                let level = name[1..].parse::<usize>().unwrap_or(1);
                self.line.push_str(&"#".repeat(level));
                self.line.push(' ');
            }
            "br" => {
                if self.pre_depth > 0 {
                    self.line.push('\n');
                } else {
                    self.flush_line();
                }
            }
            "hr" => {
                self.flush_block();
                self.out.push_str("---\n\n");
            }
            "pre" => {
                self.flush_block();
                self.pre_depth += 1;
                let language = tag.attr("class").and_then(language_of).unwrap_or_default();
                self.out.push_str(&format!("```{language}\n"));
            }
            "code" if self.pre_depth == 0 => self.line.push('`'),
            // `<pre><code class="language-x">`: the language sits on the code.
            "code" if self.line.is_empty() && self.out.ends_with("```\n") => {
                if let Some(language) = tag.attr("class").and_then(language_of) {
                    self.out.pop();
                    self.out.push_str(&language);
                    self.out.push('\n');
                }
            }
            "ul" | "ol" => {
                self.flush_line();
                let start = tag.attr("start").and_then(|s| s.parse().ok()).unwrap_or(1);
                self.lists.push(ListLevel {
                    ordered: name == "ol",
                    next: start,
                });
            }
            "li" => {
                self.flush_line();
                let depth = self.lists.len().saturating_sub(1);
                let bullet = match self.lists.last_mut() {
                    Some(level) if level.ordered => {
                        let bullet = format!("{}. ", level.next);
                        level.next += 1;
                        bullet
                    }
                    _ => "- ".to_string(),
                };
                self.item_prefix = Some(format!("{}{bullet}", "  ".repeat(depth)));
            }
            "blockquote" => {
                self.flush_block();
                self.quote_depth += 1;
            }
            "a" => {
                let href = tag.attr("href").and_then(|href| self.absolute(href));
                self.links.push((href, self.line.len()));
            }
            "img" => {
                // An image only matters when it is all a link has to say.
                if let (Some(alt), Some(_)) = (tag.attr("alt"), self.links.last()) {
                    push_collapsed(&mut self.line, alt);
                }
            }
            "table" => {
                self.flush_block();
                self.table = Some(Table::default());
            }
            _ if BLOCKS.contains(&name) => self.flush_line(),
            _ => {}
        }
    }

    fn close(&mut self, name: &str) {
        if self.table.is_some() {
            self.table_close(name);
            return;
        }
        if self.pre_depth > 0 && name != "pre" && name != "code" {
            if BLOCKS.contains(&name) {
                self.pre_newline();
            }
            return;
        }
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" => self.flush_block(),
            "pre" if self.pre_depth > 0 => {
                self.pre_depth -= 1;
                let code = std::mem::take(&mut self.line);
                self.out.push_str(code.trim_matches('\n'));
                self.out.push_str("\n```\n\n");
            }
            "code" if self.pre_depth == 0 => {
                if self.line.ends_with('`') {
                    self.line.pop();
                } else {
                    self.line.push('`');
                }
            }
            "ul" | "ol" => {
                self.flush_line();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.out.push('\n');
                }
            }
            "li" => self.flush_line(),
            "blockquote" => {
                self.flush_block();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            "a" => {
                let Some((href, start)) = self.links.pop() else {
                    return;
                };
                let start = start.min(self.line.len());
                let text = self.line[start..].trim().to_string();
                let Some(href) = href else { return };
                if text.is_empty() {
                    return;
                }
                let leading = if self.line[start..].starts_with(' ') {
                    " "
                } else {
                    ""
                };
                self.line.truncate(start);
                self.line.push_str(&format!("{leading}[{text}]({href})"));
            }
            _ if BLOCKS.contains(&name) => self.flush_line(),
            _ => {}
        }
    }

    fn table_open(&mut self, name: &str) {
        let Some(table) = &mut self.table else { return };
        match name {
            "thead" => table.in_thead = true,
            "tr" => {
                table.rows.push(Vec::new());
                table.head_row = table.in_thead;
            }
            "td" | "th" => {
                if table.rows.is_empty() {
                    table.rows.push(Vec::new());
                }
                if name == "th" && table.rows.len() == 1 {
                    table.head_row = true;
                }
                table.cell = Some(String::new());
            }
            "br" => {
                if let Some(cell) = &mut table.cell {
                    cell.push(' ');
                }
            }
            _ => {}
        }
    }

    fn table_close(&mut self, name: &str) {
        let Some(table) = &mut self.table else { return };
        match name {
            "td" | "th" => {
                if let Some(cell) = table.cell.take() {
                    let cell = cell.trim().replace('|', "\\|");
                    if let Some(row) = table.rows.last_mut() {
                        row.push(cell);
                    }
                }
            }
            "tr" if table.head_row => {
                table.header_rows = table.rows.len();
                table.head_row = false;
            }
            "thead" => {
                table.in_thead = false;
                table.header_rows = table.rows.len();
            }
            "table" => {
                let table = self.table.take().unwrap_or_default();
                self.out.push_str(&render_table(&table));
            }
            _ => {}
        }
    }

    fn absolute(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
            return None;
        }
        match &self.base {
            Some(base) => base.join(href).ok().map(String::from),
            None => Url::parse(href).ok().map(String::from),
        }
    }

    /// Start a new line of preformatted text, unless one was just started.
    fn pre_newline(&mut self) {
        if !self.line.is_empty() && !self.line.ends_with('\n') {
            self.line.push('\n');
        }
    }

    /// End the current line (a list item, a `<br>`).
    fn flush_line(&mut self) {
        let text = std::mem::take(&mut self.line);
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let quote = "> ".repeat(self.quote_depth);
        let prefix = self.item_prefix.take().unwrap_or_else(|| {
            // Continuation text inside a list item lines up under it.
            "  ".repeat(self.lists.len())
        });
        self.out.push_str(&quote);
        self.out.push_str(&prefix);
        self.out.push_str(text);
        self.out.push('\n');
        if self.lists.is_empty() {
            self.out.push('\n');
        }
    }

    /// End a block: its line plus a blank line after.
    fn flush_block(&mut self) {
        self.flush_line();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn finish(mut self) -> String {
        self.flush_block();
        let mut result = String::new();
        let mut blank = 0;
        for line in self.out.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                blank += 1;
                if blank > 1 {
                    continue;
                }
            } else {
                blank = 0;
            }
            result.push_str(line);
            result.push('\n');
        }
        result.trim().to_string()
    }
}

fn render_table(table: &Table) -> String {
    let rows: Vec<&Vec<String>> = table.rows.iter().filter(|row| !row.is_empty()).collect();
    if rows.is_empty() {
        return String::new();
    }
    let columns = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let line = |row: &Vec<String>| {
        let mut cells: Vec<&str> = row.iter().map(String::as_str).collect();
        cells.resize(columns, "");
        format!("| {} |\n", cells.join(" | "))
    };
    let header_rows = table.header_rows.clamp(1, rows.len());
    let mut out = String::new();
    for row in &rows[..header_rows] {
        out.push_str(&line(row));
    }
    out.push_str(&format!("|{}\n", " --- |".repeat(columns)));
    for row in &rows[header_rows..] {
        out.push_str(&line(row));
    }
    out.push('\n');
    out
}

/// `language-rust` / `lang-rust` in a class list names a code block.
fn language_of(class: &str) -> Option<String> {
    class.split_whitespace().find_map(|name| {
        name.strip_prefix("language-")
            .or_else(|| name.strip_prefix("lang-"))
            .map(str::to_string)
    })
}

/// Append `text` with runs of whitespace collapsed to one space, never
/// doubling a space already at the end of `out`.
fn push_collapsed(out: &mut String, text: &str) {
    for (index, word) in text.split_whitespace().enumerate() {
        let needs_space = if index == 0 {
            text.starts_with(char::is_whitespace)
        } else {
            true
        };
        if needs_space && !out.is_empty() && !out.ends_with(' ') && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(word);
    }
    if text.ends_with(char::is_whitespace) && !text.trim().is_empty() && !out.ends_with(' ') {
        out.push(' ');
    }
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        html_to_markdown(html, "https://example.com/docs/page.html").markdown
    }

    #[test]
    fn headings_paragraphs_and_inline_code() {
        let out = md("<h1>Title</h1><p>Some   <b>bold</b>\n text with <code>x = 1</code>.</p><h2>Next</h2><p>More.</p>");
        assert_eq!(
            out,
            "# Title\n\nSome bold text with `x = 1`.\n\n## Next\n\nMore."
        );
    }

    #[test]
    fn links_are_made_absolute_and_empty_ones_dropped() {
        let out = md(
            r##"<p>See <a href="../api/">the API</a>, <a href="#top">top</a> and <a href="javascript:void(0)">js</a>.</p>"##,
        );
        assert_eq!(out, "See [the API](https://example.com/api/), top and js.");
    }

    #[test]
    fn a_base_element_moves_the_base() {
        let converted = html_to_markdown(
            r#"<head><base href="https://other.org/x/"><title> A  page </title></head><body><a href="y">y</a></body>"#,
            "https://example.com/",
        );
        assert_eq!(converted.title, "A page");
        assert_eq!(converted.markdown, "[y](https://other.org/x/y)");
    }

    #[test]
    fn scripts_navigation_and_hidden_content_are_dropped() {
        let out = md(
            r#"<nav><a href="/">Home</a></nav><script>var x = "<p>no</p>";</script>
            <style>p{}</style><p>Kept</p><div hidden>secret</div><footer>(c)</footer>"#,
        );
        assert_eq!(out, "Kept");
    }

    #[test]
    fn main_wins_over_the_rest_of_the_page() {
        let out = md("<header><h1>Site</h1></header><div class=sidebar>links</div><main><h1>Article</h1><p>Body</p></main><div>after</div>");
        assert_eq!(out, "# Article\n\nBody");
    }

    #[test]
    fn a_single_article_is_the_content_but_several_are_not() {
        assert_eq!(md("<p>menu</p><article><p>one</p></article>"), "one");
        assert_eq!(
            md("<article><p>one</p></article><article><p>two</p></article>"),
            "one\n\ntwo"
        );
    }

    #[test]
    fn nested_lists_are_indented_and_numbered() {
        let out = md("<ul><li>a<ul><li>a1</li><li>a2</li></ul></li><li>b</li></ul><ol start=3><li>c</li><li>d</li></ol><p>end</p>");
        assert_eq!(out, "- a\n  - a1\n  - a2\n- b\n\n3. c\n4. d\n\nend");
    }

    #[test]
    fn preformatted_code_keeps_its_whitespace() {
        let out = md("<pre><code class=\"language-rust\">fn main() {\n    println!(\"hi\");\n}\n</code></pre><p>after</p>");
        assert_eq!(
            out,
            "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nafter"
        );
    }

    #[test]
    fn highlighted_code_with_a_block_per_line_stays_one_code_block() {
        let out = md(
            r#"<pre data-language="rust"><code><div class="ec-line"><div class="code"><span>use a;</span></div></div><div class="ec-line"><div class="code"><span>  use b;</span></div></div></code></pre><p>after</p>"#,
        );
        assert_eq!(out, "```\nuse a;\n  use b;\n```\n\nafter");
    }

    #[test]
    fn tables_become_pipe_tables() {
        let out = md("<table><tr><th>Name</th><th>Size</th></tr><tr><td>a|b</td><td>1</td></tr><tr><td>c</td></tr></table>");
        assert_eq!(
            out,
            "| Name | Size |\n| --- | --- |\n| a\\|b | 1 |\n| c |  |"
        );
    }

    #[test]
    fn quotes_and_rules() {
        let out = md("<blockquote><p>quoted</p></blockquote><hr><p>x</p>");
        assert_eq!(out, "> quoted\n\n---\n\nx");
    }

    #[test]
    fn an_image_speaks_only_for_an_empty_link() {
        let out = md(
            r#"<p><img src="a.png" alt="decor"> <a href="/home"><img src="logo.png" alt="Home"></a></p>"#,
        );
        assert_eq!(out, "[Home](https://example.com/home)");
    }
}
