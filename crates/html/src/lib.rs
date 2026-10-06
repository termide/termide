//! HTML → terminal pseudographics renderer.
//!
//! Tokenizes HTML with `html5ever` (the tokenizer only — no DOM, no CSS) into a
//! small owned token stream, then drives the shared
//! [`termide_richtext::Builder`] layout engine. A supported subset of tags maps
//! to styled blocks/inline runs; unknown tags are transparent (their content
//! still renders). This is *not* a browser: author CSS is ignored and layout is
//! a fixed tag→style mapping.
//!
//! The driver keeps its open-element stack in [`HtmlState`], so HTML that
//! arrives in fragments renders coherently. That matters for HTML embedded in
//! Markdown, where CommonMark splits a single element (e.g. `<details>` wrapping
//! Markdown) across several events with Markdown in between. The Markdown
//! renderer feeds those fragments through the same [`drive`] entry point with a
//! persistent [`HtmlState`].

use html5ever::tendril::Tendril;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use ratatui::style::{Modifier, Style};
use std::cell::RefCell;
use termide_core::ThemeColors;
use termide_richtext::{Builder, Rendered};

/// Render a complete HTML document/fragment for the given inner `width`.
#[must_use]
pub fn render_html(src: &str, width: u16, colors: &ThemeColors, is_light: bool) -> Rendered {
    let toks = tokenize(src);
    let mut b = Builder::new(width, colors, is_light);
    let mut st = HtmlState::default();
    drive(&mut b, &mut st, &toks);
    st.close_all(&mut b);
    // Flush any inline run not wrapped in a block element (bare top-level text).
    b.flush_block();
    b.finish()
}

/// An owned, simplified HTML token. The `html5ever` token types borrow tendrils
/// and are not `'static`, so the sink converts them to this before layout.
#[derive(Debug, Clone)]
pub enum Tok {
    /// A start tag with lowercased name and `(name, value)` attributes. `void`
    /// is true for self-closing tags and HTML void elements (no end tag).
    Open {
        name: String,
        attrs: Vec<(String, String)>,
        void: bool,
    },
    /// An end tag with lowercased name.
    Close(String),
    /// Character data (entities already decoded by the tokenizer).
    Text(String),
}

/// Tokenize HTML into the owned [`Tok`] stream.
#[must_use]
pub fn tokenize(src: &str) -> Vec<Tok> {
    let sink = Sink(RefCell::new(Vec::new()));
    let tok = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(Tendril::from(src));
    let _ = tok.feed(&input);
    tok.end();
    tok.sink.0.into_inner()
}

struct Sink(RefCell<Vec<Tok>>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        match token {
            Token::CharacterTokens(s) => {
                // The tokenizer splits entities (`&amp;`, `&#39;`) into their own
                // character tokens; coalesce adjacent text so word-splitting
                // does not insert spurious spaces between the pieces.
                let mut v = self.0.borrow_mut();
                if let Some(Tok::Text(last)) = v.last_mut() {
                    last.push_str(&s);
                } else {
                    v.push(Tok::Text(s.to_string()));
                }
            }
            Token::TagToken(t) => {
                let name = t.name.as_ref().to_ascii_lowercase();
                if t.kind == TagKind::StartTag {
                    let attrs = t
                        .attrs
                        .iter()
                        .map(|a| {
                            (
                                a.name.local.as_ref().to_ascii_lowercase(),
                                a.value.to_string(),
                            )
                        })
                        .collect();
                    let void = t.self_closing || is_void(&name);
                    self.0.borrow_mut().push(Tok::Open { name, attrs, void });
                } else {
                    self.0.borrow_mut().push(Tok::Close(name));
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// HTML void elements: they never have an end tag.
fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// What to do when an open element's end tag (or auto-close) is reached.
#[derive(Debug, Clone, Copy)]
enum Close {
    /// Transparent wrapper: nothing to undo.
    None,
    PopStyle,
    Heading,
    /// Paragraph-like block: flush the inline run and separate with a blank.
    Block,
    Quote,
    List,
    Item,
    Link,
    CodeBlock,
    /// `<summary>`: pop the bold style and break the line.
    Summary,
    /// `<head>`/`<script>`/`<style>`/`<title>`: re-enable text output.
    Unsuppress,
    Table,
    TableRow,
    TableCell,
    Thead,
}

struct Frame {
    name: String,
    close: Close,
}

/// Persistent driver state: the open-element stack plus a few mode counters.
/// Reused across fragment feeds so split elements render coherently.
#[derive(Default)]
pub struct HtmlState {
    stack: Vec<Frame>,
    /// Inside `<head>`/`<script>`/`<style>`/`<title>`: text is dropped.
    suppress: usize,
    /// Inside `<pre>`: text is kept verbatim (no whitespace collapsing).
    pre: usize,
    /// Inside `<thead>`: a row close styles the row as a table header.
    in_thead: bool,
}

impl HtmlState {
    /// Flush any still-open elements (unclosed `<p>`, `<table>`, …) at the end
    /// of input, running their close actions in stack order.
    pub fn close_all(&mut self, b: &mut Builder) {
        while let Some(frame) = self.stack.pop() {
            do_close(
                b,
                frame.close,
                &mut self.in_thead,
                &mut self.pre,
                &mut self.suppress,
            );
        }
    }
}

/// Drive `b` from a slice of tokens, threading `st` so calls compose across
/// fragments.
pub fn drive(b: &mut Builder, st: &mut HtmlState, toks: &[Tok]) {
    for t in toks {
        match t {
            Tok::Text(s) => emit_text(b, st, s),
            Tok::Open { name, attrs, void } => open(b, st, name, attrs, *void),
            Tok::Close(name) => close(b, st, name),
        }
    }
}

fn emit_text(b: &mut Builder, st: &HtmlState, s: &str) {
    if st.suppress > 0 {
        return;
    }
    if st.pre > 0 {
        b.text(s);
        return;
    }
    let norm = collapse_ws(s);
    if !norm.is_empty() {
        b.text(&norm);
    }
}

fn open(b: &mut Builder, st: &mut HtmlState, name: &str, attrs: &[(String, String)], void: bool) {
    // While suppressed, ignore everything except the matching close, which is
    // handled by `close` finding the frame we push here.
    if st.suppress > 0 && !matches!(name, "script" | "style" | "head" | "title" | "noscript") {
        return;
    }

    let close: Close = match name {
        "script" | "style" | "head" | "title" | "noscript" => {
            st.suppress += 1;
            Close::Unsuppress
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let depth = (name.as_bytes()[1] - b'0') as usize;
            b.start_heading(depth);
            Close::Heading
        }
        "p" | "div" | "section" | "article" | "header" | "footer" | "nav" | "main" | "aside"
        | "figure" | "figcaption" | "details" | "dl" | "dd" | "dt" | "address" | "fieldset" => {
            Close::Block
        }
        "summary" => {
            b.push_style(b.cur_style().add_modifier(Modifier::BOLD));
            Close::Summary
        }
        "blockquote" => {
            b.start_quote();
            Close::Quote
        }
        "ul" | "menu" => {
            b.start_list(None);
            Close::List
        }
        "ol" => {
            b.start_list(Some(1));
            Close::List
        }
        "li" => {
            b.start_item();
            Close::Item
        }
        "pre" => {
            b.start_code_block("");
            st.pre += 1;
            Close::CodeBlock
        }
        "code" if st.pre == 0 => {
            b.push_style(Style::default().fg(b.colors().success));
            Close::PopStyle
        }
        "b" | "strong" => {
            b.push_style(b.cur_style().add_modifier(Modifier::BOLD));
            Close::PopStyle
        }
        "i" | "em" | "cite" | "var" | "dfn" => {
            b.push_style(b.cur_style().add_modifier(Modifier::ITALIC));
            Close::PopStyle
        }
        "u" | "ins" => {
            b.push_style(b.cur_style().add_modifier(Modifier::UNDERLINED));
            Close::PopStyle
        }
        "s" | "del" | "strike" => {
            b.push_style(b.cur_style().add_modifier(Modifier::CROSSED_OUT));
            Close::PopStyle
        }
        "kbd" | "mark" | "samp" => {
            b.push_style(b.cur_style().add_modifier(Modifier::REVERSED));
            Close::PopStyle
        }
        "a" => {
            b.start_link(attr(attrs, "href"));
            Close::Link
        }
        "br" => {
            b.hard_break();
            Close::None
        }
        "hr" => {
            b.rule();
            Close::None
        }
        "img" => {
            b.start_image(attr(attrs, "src"));
            let alt = attr(attrs, "alt");
            if !alt.is_empty() {
                b.text(&alt);
            }
            b.end_link();
            Close::None
        }
        "table" => {
            b.start_table(0);
            Close::Table
        }
        "thead" => {
            st.in_thead = true;
            Close::Thead
        }
        "tr" => {
            b.start_table_row();
            Close::TableRow
        }
        "td" | "th" => {
            b.start_table_cell();
            Close::TableCell
        }
        _ => Close::None,
    };

    // Register a fragment anchor for this element's `id` (or `<a name>`),
    // pointing at the line where its content begins.
    let mut anchor = attr(attrs, "id");
    if anchor.is_empty() && name == "a" {
        anchor = attr(attrs, "name");
    }
    if !anchor.is_empty() {
        b.add_anchor(anchor);
    }

    if !void {
        st.stack.push(Frame {
            name: name.to_string(),
            close,
        });
    } else if let Close::Unsuppress = close {
        // A self-closed suppressing tag (rare) leaves no frame to undo it.
        st.suppress = st.suppress.saturating_sub(1);
    }
}

fn close(b: &mut Builder, st: &mut HtmlState, name: &str) {
    // Find the nearest matching open element; auto-close anything above it.
    let Some(idx) = st.stack.iter().rposition(|f| f.name == name) else {
        return; // stray end tag
    };
    while st.stack.len() > idx {
        let frame = st.stack.pop().expect("len checked");
        do_close(
            b,
            frame.close,
            &mut st.in_thead,
            &mut st.pre,
            &mut st.suppress,
        );
    }
}

fn do_close(
    b: &mut Builder,
    close: Close,
    in_thead: &mut bool,
    pre: &mut usize,
    suppress: &mut usize,
) {
    match close {
        Close::None => {}
        Close::PopStyle => b.pop_style(),
        Close::Heading => b.end_heading(),
        Close::Block => b.flush_block(),
        Close::Quote => b.end_quote(),
        Close::List => b.end_list(),
        Close::Item => b.end_item(),
        Close::Link => b.end_link(),
        Close::CodeBlock => {
            b.end_code_block();
            *pre = pre.saturating_sub(1);
        }
        Close::Summary => {
            b.pop_style();
            b.hard_break();
        }
        Close::Unsuppress => *suppress = suppress.saturating_sub(1),
        Close::Table => b.end_table(),
        Close::TableRow => {
            if *in_thead {
                b.end_table_head();
            } else {
                b.end_table_row();
            }
        }
        Close::TableCell => b.end_table_cell(),
        Close::Thead => *in_thead = false,
    }
}

/// Look up an attribute value (empty string if absent).
fn attr(attrs: &[(String, String)], key: &str) -> String {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// Collapse runs of ASCII whitespace to single spaces (HTML inline behavior),
/// preserving a single leading/trailing space so adjacent text tokens stay
/// word-separated.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_ascii_whitespace() {
            in_ws = true;
        } else {
            if in_ws {
                out.push(' ');
            }
            in_ws = false;
            out.push(ch);
        }
    }
    if in_ws && !out.is_empty() {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> ThemeColors {
        ThemeColors::default()
    }

    fn render(src: &str) -> Vec<String> {
        render_html(src, 80, &colors(), false)
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn heading_and_paragraph_separated() {
        let out = render("<h2>Title</h2><p>Body text.</p>");
        assert!(out.iter().any(|l| l.contains("## Title")), "{out:?}");
        assert!(
            out.iter()
                .any(|l| l.contains("Body text.") && !l.contains("Title")),
            "{out:?}"
        );
    }

    #[test]
    fn unordered_list_bullets() {
        let out = render("<ul><li>one</li><li>two</li></ul>");
        let joined = out.join("\n");
        assert!(joined.contains("• one"), "{out:?}");
        assert!(joined.contains("• two"), "{out:?}");
    }

    #[test]
    fn link_hit_area_recorded() {
        let r = render_html(
            "<p>see <a href=\"https://ex.com\">docs</a></p>",
            80,
            &colors(),
            false,
        );
        assert_eq!(r.links.len(), 1, "{:?}", r.links);
        assert_eq!(r.links[0].url, "https://ex.com");
    }

    #[test]
    fn image_alt_and_icon() {
        let out = render("<p><img src=\"a.png\" alt=\"a pic\"></p>");
        let joined = out.join("\n");
        assert!(joined.contains("🖼"), "{out:?}");
        assert!(joined.contains("a pic"), "{out:?}");
    }

    #[test]
    fn entities_decoded() {
        let out = render("<p>a &amp; b &#39;c&#39;</p>");
        assert!(out.iter().any(|l| l.contains("a & b 'c'")), "{out:?}");
    }

    #[test]
    fn script_and_style_suppressed() {
        let out = render("<style>p{color:red}</style><script>var x=1</script><p>visible</p>");
        let joined = out.join("\n");
        assert!(joined.contains("visible"), "{out:?}");
        assert!(!joined.contains("color:red"), "style leaked: {out:?}");
        assert!(!joined.contains("var x"), "script leaked: {out:?}");
    }

    #[test]
    fn whitespace_collapsed() {
        let out = render("<p>a\n  lot   of\tspace</p>");
        assert!(out.iter().any(|l| l.contains("a lot of space")), "{out:?}");
    }

    #[test]
    fn table_rendered_with_box() {
        let out = render(
            "<table><thead><tr><th>A</th><th>B</th></tr></thead>\
             <tbody><tr><td>1</td><td>2</td></tr></tbody></table>",
        );
        let joined = out.join("\n");
        assert!(joined.contains("┌"), "no top border: {out:?}");
        assert!(joined.contains("├"), "no header sep: {out:?}");
        assert!(joined.contains("└"), "no bottom border: {out:?}");
        assert!(joined.contains("A") && joined.contains('1'), "{out:?}");
    }

    #[test]
    fn id_attribute_registers_anchor() {
        let r = render_html("<p>x</p><h2 id=\"sec\">Section</h2>", 80, &colors(), false);
        assert!(
            r.anchors.iter().any(|(id, _)| id == "sec"),
            "{:?}",
            r.anchors
        );
    }

    #[test]
    fn unknown_tags_are_transparent() {
        let out = render("<custom-thing>kept <weird>text</weird></custom-thing>");
        assert!(out.iter().any(|l| l.contains("kept text")), "{out:?}");
    }

    #[test]
    fn inline_emphasis_bold() {
        // No panic, content preserved through nested inline styling.
        let out = render("<p>plain <b>bold <i>both</i></b> end</p>");
        assert!(
            out.iter().any(|l| l.contains("plain bold both end")),
            "{out:?}"
        );
    }
}
