//! End-to-end regression for PHP highlighting through the editor's render
//! flow: build a buffer, run the whole-document gate exactly as
//! `render_editor_content` does, then pull per-line segments the way the
//! renderer does (`buffer.line_cow(i).trim_end_matches('\n')`).
//!
//! Guards the wiring between `TextBuffer`, the whole-document highlight pass
//! and the per-line lookup — a layer the `termide-highlight` unit tests can't
//! reach on their own.

use ratatui::style::Color;
use termide_buffer::TextBuffer;
use termide_highlight::{global_highlighter, HighlightCache, WHOLE_DOCUMENT_BYTE_LIMIT};

/// Number of segments on a line whose colour differs from the default fg.
fn styled(cache: &mut HighlightCache, buffer: &TextBuffer, line_idx: usize) -> usize {
    let line_cow = buffer.line_cow(line_idx).unwrap_or_default();
    let line_text = line_cow.trim_end_matches('\n');
    cache
        .get_line_segments(line_idx, line_text)
        .iter()
        .filter(|(_, style)| style.fg != Some(Color::White))
        .count()
}

#[test]
fn mixed_php_template_highlights_html_and_php() {
    // Lines: 0-3 HTML, 4 `<?php`, 5-6 PHP, 7 `?>`, 8-9 HTML.
    let src = "<!DOCTYPE html>\n\
               <html lang=\"ru\">\n\
               <body>\n\
                   <h1>Hi</h1>\n\
                   <?php\n\
                       $name = \"Ivan\";\n\
                       echo \"<p>$name</p>\";\n\
                   ?>\n\
               </body>\n\
               </html>\n";
    let buffer = TextBuffer::from_text(src);

    let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
    cache.set_syntax("php");
    assert!(cache.has_syntax(), "php syntax should be active");

    // The size gate from `render_editor_content`.
    assert!(buffer.len_bytes() <= WHOLE_DOCUMENT_BYTE_LIMIT);
    cache.set_document(&buffer.text());

    assert!(
        styled(&mut cache, &buffer, 1) > 0,
        "HTML line should be highlighted in a mixed template"
    );
    assert!(
        styled(&mut cache, &buffer, 5) > 0,
        "PHP statement should be highlighted in a mixed template"
    );
}
