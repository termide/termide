//! Markdown → terminal pseudographics renderer.
//!
//! Parses Markdown with `pulldown-cmark` and drives the shared
//! [`termide_richtext::Builder`] layout engine, which wraps the styled runs to
//! width and emits owned `ratatui` lines plus link hit-areas. This module is
//! only the `pulldown-cmark` → `Builder` adapter; all layout lives in
//! `termide-richtext`.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use termide_core::ThemeColors;
use termide_html::HtmlState;
use termide_richtext::Builder;

pub use termide_richtext::{LinkSpan, Rendered};

/// Render `src` for the given inner `width`.
#[must_use]
pub fn render_markdown(src: &str, width: u16, colors: &ThemeColors, is_light: bool) -> Rendered {
    let mut b = Builder::new(width, colors, is_light);
    // Embedded HTML (block and inline) is driven through the shared HTML engine
    // with a persistent state, so elements that CommonMark splits across events
    // (e.g. a `<details>` wrapping Markdown) still nest correctly.
    let mut html = HtmlState::default();
    // Heading currently being collected, for generating a fragment anchor
    // (GitHub-style slug) so a table-of-contents `[x](#heading)` link works.
    let mut heading: Option<HeadingCap> = None;
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    for ev in Parser::new_ext(src, opts) {
        event(&mut b, &mut html, &mut heading, ev);
    }
    b.finish()
}

/// A heading being collected between its start and end events.
struct HeadingCap {
    /// Line index the heading will occupy (anchor target).
    line: usize,
    /// Accumulated heading text, for slug generation.
    text: String,
    /// Explicit `{#id}` from the source, if any.
    explicit_id: Option<String>,
}

fn event(b: &mut Builder, html: &mut HtmlState, heading: &mut Option<HeadingCap>, ev: Event<'_>) {
    match ev {
        Event::Start(tag) => start(b, heading, tag),
        Event::End(tag) => end(b, heading, tag),
        Event::Text(t) => {
            if let Some(h) = heading.as_mut() {
                h.text.push_str(&t);
            }
            b.text(&t);
        }
        Event::Code(t) => {
            if let Some(h) = heading.as_mut() {
                h.text.push_str(&t);
            }
            b.inline_code(&t);
        }
        Event::SoftBreak => b.soft_break(),
        Event::HardBreak => b.hard_break(),
        Event::Rule => b.rule(),
        Event::TaskListMarker(done) => b.task_marker(done),
        // Raw HTML (a block chunk or an inline tag token) renders through the
        // HTML engine instead of being shown as literal angle-bracket text.
        Event::Html(t) | Event::InlineHtml(t) => {
            let toks = termide_html::tokenize(&t);
            termide_html::drive(b, html, &toks);
        }
        _ => {}
    }
}

fn start(b: &mut Builder, heading: &mut Option<HeadingCap>, tag: Tag<'_>) {
    match tag {
        Tag::Paragraph => {}
        Tag::Heading { level, id, .. } => {
            b.start_heading(heading_depth(level));
            *heading = Some(HeadingCap {
                line: b.current_line(),
                text: String::new(),
                explicit_id: id.map(|s| s.into_string()),
            });
        }
        Tag::BlockQuote(_) => b.start_quote(),
        Tag::CodeBlock(kind) => {
            let lang = match kind {
                CodeBlockKind::Fenced(info) => {
                    let s = info.into_string();
                    s.split_whitespace().next().unwrap_or("").to_string()
                }
                CodeBlockKind::Indented => String::new(),
            };
            b.start_code_block(&lang);
        }
        Tag::List(start) => b.start_list(start),
        Tag::Item => b.start_item(),
        Tag::Emphasis => b.start_emphasis(),
        Tag::Strong => b.start_strong(),
        Tag::Strikethrough => b.start_strike(),
        Tag::Link { dest_url, .. } => b.start_link(dest_url.into_string()),
        Tag::Image { dest_url, .. } => b.start_image(dest_url.into_string()),
        Tag::Table(aligns) => b.start_table(aligns.len()),
        Tag::TableHead => b.start_table_head(),
        Tag::TableRow => b.start_table_row(),
        Tag::TableCell => b.start_table_cell(),
        _ => {}
    }
}

fn end(b: &mut Builder, heading: &mut Option<HeadingCap>, tag: TagEnd) {
    match tag {
        TagEnd::Paragraph => b.end_paragraph(),
        TagEnd::Heading(_) => {
            if let Some(h) = heading.take() {
                if let Some(id) = h.explicit_id {
                    b.add_anchor_at(id, h.line);
                }
                let slug = slugify(&h.text);
                b.add_anchor_at(slug, h.line);
            }
            b.end_heading();
        }
        // A raw HTML block is its own paragraph: flush the accumulated text and
        // separate it, so a following block (e.g. a heading) does not merge
        // onto the same line.
        TagEnd::HtmlBlock => b.flush_block(),
        TagEnd::BlockQuote(_) => b.end_quote(),
        TagEnd::CodeBlock => b.end_code_block(),
        TagEnd::List(_) => b.end_list(),
        TagEnd::Item => b.end_item(),
        TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => b.pop_style(),
        TagEnd::Link | TagEnd::Image => b.end_link(),
        TagEnd::TableCell => b.end_table_cell(),
        TagEnd::TableHead => b.end_table_head(),
        TagEnd::TableRow => b.end_table_row(),
        TagEnd::Table => b.end_table(),
        _ => {}
    }
}

fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// GitHub-style heading slug: lowercase, drop punctuation, spaces → `-`,
/// collapse and trim hyphens. Used as a fragment anchor so a `[x](#heading)`
/// table-of-contents link resolves.
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if ch == '_' {
            out.push('_'); // GitHub keeps underscores
        } else if ch == ' ' || ch == '-' {
            // Collapse runs of spaces/hyphens into a single hyphen.
            if !out.ends_with('-') {
                out.push('-');
            }
        }
        // All other punctuation is dropped (matches GitHub).
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn colors() -> ThemeColors {
        ThemeColors::default()
    }

    fn render(src: &str) -> Vec<String> {
        render_markdown(src, 80, &colors(), false)
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
    fn heading_generates_slug_anchor() {
        let r = render_markdown("# Hello, World!\n\ntext", 80, &colors(), false);
        assert!(
            r.anchors.iter().any(|(id, _)| id == "hello-world"),
            "{:?}",
            r.anchors
        );
    }

    #[test]
    fn heading_has_marker_and_text() {
        let out = render("# Title");
        assert!(out.iter().any(|l| l.contains("# Title")), "{out:?}");
    }

    #[test]
    fn html_block_does_not_merge_into_next_heading() {
        let out = render("<p>Some HTML paragraph.</p>\n\n## A heading\n\nbody\n");
        // The HTML text and the heading must land on separate lines.
        assert!(
            out.iter()
                .any(|l| l.contains("Some HTML paragraph.") && !l.contains("A heading")),
            "{out:?}"
        );
        assert!(
            out.iter()
                .any(|l| l.contains("# A heading") && !l.contains("HTML")),
            "{out:?}"
        );
    }

    #[test]
    fn nested_lists_indent() {
        let out = render("- a\n  - b\n- c");
        let joined = out.join("\n");
        assert!(joined.contains("• a"), "{out:?}");
        assert!(joined.contains("• b"), "{out:?}");
        let b = out.iter().find(|l| l.contains("• b")).unwrap();
        assert!(b.starts_with("  "), "nested not indented: {b:?}");
    }

    #[test]
    fn ordered_list_numbers() {
        let out = render("1. one\n2. two");
        let joined = out.join("\n");
        assert!(joined.contains("1. one"), "{out:?}");
        assert!(joined.contains("2. two"), "{out:?}");
    }

    #[test]
    fn blockquote_prefixed() {
        let out = render("> quoted");
        assert!(
            out.iter().any(|l| l.contains("│") && l.contains("quoted")),
            "{out:?}"
        );
    }

    #[test]
    fn table_drawn_with_box() {
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let out = render(md);
        let joined = out.join("\n");
        assert!(joined.contains("┌"), "no top border: {out:?}");
        assert!(joined.contains("├"), "no header sep: {out:?}");
        assert!(joined.contains("└"), "no bottom border: {out:?}");
        assert!(joined.contains("A") && joined.contains("B"), "{out:?}");
    }

    #[test]
    fn table_cell_inline_code_stays_in_cell() {
        // Inline code spans inside a cell must render in the cell, not leak
        // out as a paragraph below the table.
        let md = "| Crate | Path |\n|---|---|\n| core | `/p/x/*` |";
        let out = render(md);
        // The code content must appear on a bordered table line.
        assert!(
            out.iter().any(|l| l.contains("│") && l.contains("/p/x/*")),
            "code not rendered inside cell: {out:?}"
        );
        // And it must NOT appear on any non-table (borderless) line.
        assert!(
            !out.iter().any(|l| l.contains("/p/x/*") && !l.contains("│")),
            "code leaked outside the table: {out:?}"
        );
    }

    #[test]
    fn table_cell_formatting_is_styled() {
        let c = colors();
        let md = "| Head | X |\n|---|---|\n| a | `code` |";
        let r = render_markdown(md, 80, &c, false);
        // The inline-code fragment inside the cell keeps its success color.
        let code = r
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("code"))
            .expect("code span present");
        assert_eq!(code.style.fg, Some(c.success), "code lost its style");
        // Header cell text is bold.
        let head = r
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("Head"))
            .expect("header span present");
        assert!(
            head.style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD),
            "header not bold: {:?}",
            head.style
        );
    }

    #[test]
    fn table_cell_link_is_clickable() {
        let md = "| Site | Y |\n|---|---|\n| [docs](https://ex.com) | z |";
        let out = render_markdown(md, 80, &colors(), false);
        assert_eq!(out.links.len(), 1, "{:?}", out.links);
        let link = &out.links[0];
        assert_eq!(link.url, "https://ex.com");
        // The recorded columns must land exactly on the visible label inside
        // the cell (line is ASCII + box chars, so char index == column).
        let line: String = out.lines[link.line]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let slice: String = line
            .chars()
            .skip(link.start as usize)
            .take((link.end - link.start) as usize)
            .collect();
        assert_eq!(slice, "docs", "line={line:?} span={link:?}");
    }

    #[test]
    fn embedded_mermaid_renders_diagram() {
        let md = "```mermaid\nsequenceDiagram\nA->>B: hi\n```";
        let out = render(md);
        let joined = out.join("\n");
        // The diagram (boxes + arrow), not the raw source line.
        assert!(joined.contains('┌'), "no diagram boxes: {out:?}");
        assert!(joined.contains('▶'), "no arrowhead: {out:?}");
    }

    #[test]
    fn fenced_code_rendered() {
        let md = "```rust\nlet x = 1;\n```";
        let out = render(md);
        assert!(out.iter().any(|l| l.contains("let x = 1;")), "{out:?}");
    }

    #[test]
    fn horizontal_rule() {
        let out = render("a\n\n---\n\nb");
        assert!(out.iter().any(|l| l.contains("───")), "{out:?}");
    }

    #[test]
    fn wraps_long_paragraph() {
        let long = "word ".repeat(40);
        let out = render_markdown(&long, 20, &colors(), false);
        assert!(out.lines.len() > 1, "expected wrapping into multiple lines");
        for l in &out.lines {
            let w: usize = l.spans.iter().map(|s| s.content.width()).sum();
            assert!(w <= 20, "line exceeds width: {w}");
        }
    }

    #[test]
    fn link_hit_area_recorded() {
        let out = render_markdown("see [docs](https://example.com) now", 80, &colors(), false);
        assert_eq!(out.links.len(), 1, "{:?}", out.links);
        let link = &out.links[0];
        assert_eq!(link.url, "https://example.com");
        // The hit area should cover the visible "docs" label.
        let line: String = out.lines[link.line]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let slice: String = line
            .chars()
            .skip(link.start as usize)
            .take((link.end - link.start) as usize)
            .collect();
        assert_eq!(slice, "docs", "line={line:?} span={link:?}");
    }

    #[test]
    fn image_is_clickable_with_icon() {
        let out = render_markdown("![alt text](https://img.test/a.png)", 80, &colors(), false);
        assert!(!out.links.is_empty(), "image should be a link");
        assert_eq!(out.links[0].url, "https://img.test/a.png");
        let joined: String = out.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(joined.contains("🖼"), "no icon: {joined:?}");
        assert!(joined.contains("alt text"), "no alt: {joined:?}");
    }

    #[test]
    fn inline_html_kbd_is_reverse_styled() {
        use ratatui::style::Modifier;
        let out = render_markdown("Press <kbd>Esc</kbd> now", 80, &colors(), false);
        let span = out
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("Esc"))
            .expect("no Esc span");
        assert!(
            span.style.add_modifier.contains(Modifier::REVERSED),
            "kbd not reversed: {:?}",
            span.style
        );
    }

    #[test]
    fn block_html_img_renders_as_icon() {
        // A badge-style centered image, the common README pattern.
        let out = render("<p align=\"center\"><img src=\"a.png\" alt=\"Logo\"></p>");
        let joined = out.join("\n");
        assert!(joined.contains("🖼"), "no icon: {out:?}");
        assert!(joined.contains("Logo"), "no alt: {out:?}");
    }

    #[test]
    fn details_block_split_across_events_renders_both() {
        // CommonMark splits this into HtmlBlock + Markdown paragraph + HtmlBlock;
        // the persistent HTML state must keep <details> coherent.
        let out = render("<details><summary>more</summary>\n\nhidden body\n\n</details>");
        let joined = out.join("\n");
        assert!(joined.contains("more"), "no summary: {out:?}");
        assert!(joined.contains("hidden body"), "no body: {out:?}");
    }
}
