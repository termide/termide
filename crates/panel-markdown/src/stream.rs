//! Rendering a Markdown document that grows at its end, as a streamed
//! answer does, without laying out its finished blocks again on each piece.
//!
//! The document is cut where a block ends for good: after a blank line
//! outside a code fence, before a line that cannot continue what came
//! before (not indented, not a list item, quote, table row or HTML). The
//! head before the cut is rendered once and kept; each call renders only
//! the tail and joins the two, as the whole document would render. Where
//! a cut is not certain no cut is made, and the whole document renders.

use termide_core::ThemeColors;
use termide_richtext::{LinkSpan, Rendered, RowCopy, WideDiagram};

use crate::render::render_markdown_with;

/// The rendered head of a growing document, for [`render_streamed`].
pub struct StreamedHead {
    width: u16,
    is_light: bool,
    pan_wide: bool,
    /// The source the head was rendered from: the document up to its cut.
    source: String,
    /// Whether a line seen past the cut may open an HTML block, which a
    /// blank line need not close: no later cut is certain then.
    frozen: bool,
    rendered: Rendered,
}

/// Render `src` as [`render_markdown_with`] does with no diagrams folded,
/// reusing the head kept in `head` when `src` still starts with it, and
/// keeping a longer one for the next call.
#[must_use]
pub fn render_streamed(
    head: &mut Option<StreamedHead>,
    src: &str,
    width: u16,
    colors: &ThemeColors,
    is_light: bool,
    pan_wide: bool,
) -> Rendered {
    let usable = head.as_ref().is_some_and(|head| {
        head.width == width
            && head.is_light == is_light
            && head.pan_wide == pan_wide
            && src.starts_with(head.source.as_str())
    });
    if !usable {
        *head = None;
    }
    let (from, frozen) = head
        .as_ref()
        .map_or((0, false), |head| (head.source.len(), head.frozen));
    if !frozen {
        let (cut, frozen) = last_cut(src, from);
        if cut > from {
            let whole = render_markdown_with(&src[..cut], width, colors, is_light, None, pan_wide);
            *head = Some(StreamedHead {
                width,
                is_light,
                pan_wide,
                source: src[..cut].to_string(),
                frozen,
                rendered: whole,
            });
        } else if let Some(head) = head.as_mut() {
            head.frozen = frozen;
        } else if frozen {
            *head = Some(StreamedHead {
                width,
                is_light,
                pan_wide,
                source: String::new(),
                frozen,
                rendered: render_markdown_with("", width, colors, is_light, None, pan_wide),
            });
        }
    }
    let Some(head) = head.as_ref() else {
        return render_markdown_with(src, width, colors, is_light, None, pan_wide);
    };
    let tail = render_markdown_with(
        &src[head.source.len()..],
        width,
        colors,
        is_light,
        None,
        pan_wide,
    );
    join(&head.rendered, tail)
}

/// The last place in `src` past `from` (a cut, or 0) where the document
/// can be cut, or `from` when there is none; and whether a line past
/// `from` may open an HTML block.
fn last_cut(src: &str, from: usize) -> (usize, bool) {
    let mut cut = from;
    // The fence open at the start of the line: its character and length.
    let mut fence: Option<(char, usize)> = None;
    let mut after_blank = false;
    let mut at = from;
    for line in src[from..].split_inclusive('\n') {
        let start = at;
        at += line.len();
        let complete = line.ends_with('\n');
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        if let Some((mark, len)) = fence {
            if complete && closes_fence(trimmed, mark, len) {
                fence = None;
            }
            after_blank = false;
            continue;
        }
        if trimmed.is_empty() {
            after_blank = complete;
            continue;
        }
        // A line that may open an HTML block: a blank line does not close
        // every kind (`<pre>`, `<!--`, `<script>`), so no cut is certain
        // from here on.
        if trimmed.starts_with('<') {
            return (cut, true);
        }
        if after_blank && start > 0 && starts_a_block(body) {
            cut = start;
        }
        after_blank = false;
        if complete {
            fence = opens_fence(trimmed);
        }
    }
    (cut, false)
}

/// Whether `line`, after a blank line, starts a block nothing before it
/// can claim: a list or quote it could continue, an indented block, a
/// table row and anything that may be HTML do not count.
fn starts_a_block(line: &str) -> bool {
    line.chars().next().is_some_and(|first| {
        !first.is_whitespace()
            && !first.is_ascii_digit()
            && !matches!(first, '-' | '*' | '+' | '>' | '|' | '<')
    })
}

/// The fence `line` (trimmed) opens: ```` ``` ```` or `~~~`, and how long.
fn opens_fence(line: &str) -> Option<(char, usize)> {
    let mark = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = line.chars().take_while(|&c| c == mark).count();
    (len >= 3).then_some((mark, len))
}

/// Whether `line` (trimmed) closes a fence of `len` `mark`s.
fn closes_fence(line: &str, mark: char, len: usize) -> bool {
    let run = line.chars().take_while(|&c| c == mark).count();
    run >= len && line[run * mark.len_utf8()..].trim().is_empty()
}

/// `head` and `tail` rendered apart, joined as one document renders: a
/// blank line between them, as between any two blocks.
fn join(head: &Rendered, tail: Rendered) -> Rendered {
    if head.lines.is_empty() {
        return tail;
    }
    let gap = usize::from(
        !tail.lines.is_empty() && tail.lines.first().is_some_and(|l| !l.spans.is_empty()),
    );
    let offset = head.lines.len() + gap;
    let link_ids = head.links.iter().map(|link| link.id + 1).max().unwrap_or(0);
    let mut lines = head.lines.clone();
    let mut copy = head.copy.clone();
    if gap > 0 {
        lines.push(ratatui::text::Line::default());
        copy.push(RowCopy::default());
    }
    lines.extend(tail.lines);
    copy.extend(tail.copy);
    let mut links = head.links.clone();
    links.extend(tail.links.into_iter().map(|link| LinkSpan {
        line: link.line + offset,
        id: link.id + link_ids,
        ..link
    }));
    let mut anchors = head.anchors.clone();
    anchors.extend(
        tail.anchors
            .into_iter()
            .map(|(id, line)| (id, line + offset)),
    );
    let mut wide_diagrams = head.wide_diagrams.clone();
    wide_diagrams.extend(tail.wide_diagrams.into_iter().map(|wide| WideDiagram {
        line: wide.line + offset,
        index: wide.index + head.diagrams,
        ..wide
    }));
    Rendered {
        lines,
        copy,
        links,
        anchors,
        diagrams: head.diagrams + tail.diagrams,
        folded_diagrams: head.folded_diagrams + tail.folded_diagrams,
        wide_diagrams,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rendered: &Rendered) -> Vec<String> {
        rendered
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Whole and joined renderings agree, link ids up to their numbering.
    fn assert_same(whole: &Rendered, joined: &Rendered, src: &str) {
        assert_eq!(text(whole), text(joined), "source:\n{src}");
        assert_eq!(whole.lines, joined.lines, "source:\n{src}");
        assert_eq!(whole.copy, joined.copy, "source:\n{src}");
        let regions = |r: &Rendered| -> Vec<(usize, u16, u16, String)> {
            r.links
                .iter()
                .map(|l| (l.line, l.start, l.end, l.url.clone()))
                .collect()
        };
        assert_eq!(regions(whole), regions(joined), "source:\n{src}");
        let groups = |r: &Rendered| -> Vec<bool> {
            r.links.windows(2).map(|w| w[0].id == w[1].id).collect()
        };
        assert_eq!(groups(whole), groups(joined), "source:\n{src}");
        assert_eq!(whole.diagrams, joined.diagrams, "source:\n{src}");
        assert_eq!(whole.wide_diagrams, joined.wide_diagrams, "source:\n{src}");
    }

    const DOCUMENTS: &[&str] = &[
        "First paragraph with a [link](https://a.b) in it.\n\nSecond one, \
         long enough to wrap over more than one row of the panel it is in.\n\n\
         ## A heading\n\nText under it.\n",
        "Intro.\n\n```rust\nfn main() {\n\n    println!(\"hi\");\n}\n```\n\nAfter the code.\n\n\
         ~~~\nraw\n\n~~~\n\nEnd.",
        "Steps:\n\n- one\n- two\n\n  continued\n\n- three\n\nThen 1. not a list.\n\n\
         1. first\n2. second\n\nDone with **bold** and `code`.",
        "> a quote\n> going on\n\nNot quoted.\n\n> another\n\nText.\n\n| a | b |\n|---|---|\n\
         | 1 | 2 |\n\nAfter the table.",
        "Before.\n\n<details>\n<summary>More</summary>\n\nHidden *text*.\n\n</details>\n\nAfter.",
        "A diagram:\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nAnd text https://x.y after.\n\n\
         ```mermaid\ngraph TD\n  C --> D\n```\n\nEnd.",
        "Loose:\n\n1. first\n\n2. second\n\n10. tenth\n\n* star\n\n+ plus\n\nEnd.",
        "Para\n\n    indented code\n\n    more\n\nPara two\n\n---\n\nAfter a rule.",
    ];

    /// Every document, fed a few bytes at a time, renders the same at each
    /// step as when it is rendered whole.
    #[test]
    fn a_streamed_document_renders_as_the_whole_one() {
        let colors = ThemeColors::default();
        for doc in DOCUMENTS {
            for width in [20, 80] {
                let mut head = None;
                let mut end = 0;
                while end < doc.len() {
                    end = (end + 7).min(doc.len());
                    while !doc.is_char_boundary(end) {
                        end += 1;
                    }
                    let src = &doc[..end];
                    let joined = render_streamed(&mut head, src, width, &colors, false, true);
                    let whole = render_markdown_with(src, width, &colors, false, None, true);
                    assert_same(&whole, &joined, src);
                }
            }
        }
    }

    #[test]
    fn a_finished_block_is_kept_and_the_tail_renders_alone() {
        let colors = ThemeColors::default();
        let mut head = None;
        let src = "One.\n\nTwo.\n\nThr";
        let _ = render_streamed(&mut head, src, 40, &colors, false, false);
        assert_eq!(
            head.as_ref().map(|h| h.source.as_str()),
            Some("One.\n\nTwo.\n\n")
        );

        // Inside an open fence no cut is made past it.
        let src = "One.\n\nTwo.\n\nThree\n\n```\ncode\n\nT";
        let _ = render_streamed(&mut head, src, 40, &colors, false, false);
        assert_eq!(
            head.as_ref().map(|h| h.source.as_str()),
            Some("One.\n\nTwo.\n\nThree\n\n")
        );

        // A different document starts over.
        let _ = render_streamed(&mut head, "Other.\n\nText", 40, &colors, false, false);
        assert_eq!(head.as_ref().map(|h| h.source.as_str()), Some("Other.\n\n"));

        // So does another width.
        let _ = render_streamed(&mut head, "Other.\n\nText", 30, &colors, false, false);
        assert_eq!(head.as_ref().map(|h| h.width), Some(30));
    }

    #[test]
    fn html_stops_cutting() {
        let colors = ThemeColors::default();
        let mut head = None;
        let src = "One.\n\nTwo.\n\n<!-- a\n\nb -->\n\nThree.\n\nFour";
        let _ = render_streamed(&mut head, src, 40, &colors, false, false);
        let head = head.expect("a head before the HTML");
        assert_eq!(head.source, "One.\n\n");
        assert!(head.frozen);
    }
}
