//! Tests for `Editor`'s Panel-trait command handling and large-file behavior.

use super::*;
use std::io::Write;
use tempfile::NamedTempFile;
use termide_core::{CommandResult, Panel, PanelCommand};

fn create_editor_with_content(content: &str) -> (Editor, NamedTempFile) {
    let mut file = NamedTempFile::new().unwrap();
    write!(file, "{}", content).unwrap();
    let editor =
        Editor::open_file_with_config(file.path().to_path_buf(), EditorConfig::default()).unwrap();
    (editor, file)
}

#[test]
fn test_handle_command_get_modification_status_new_editor() {
    let mut editor = Editor::new();
    let result = editor.handle_command(PanelCommand::GetModificationStatus);

    if let CommandResult::ModificationStatus {
        is_modified,
        has_external_change,
    } = result
    {
        assert!(!is_modified);
        assert!(!has_external_change);
    } else {
        panic!("Expected ModificationStatus result");
    }
}

#[test]
fn test_handle_command_get_modification_status_after_edit() {
    let (mut editor, _file) = create_editor_with_content("hello");

    // Insert text to modify buffer
    let _ = editor.insert_char('x');

    let result = editor.handle_command(PanelCommand::GetModificationStatus);
    if let CommandResult::ModificationStatus {
        is_modified,
        has_external_change,
    } = result
    {
        assert!(is_modified);
        assert!(!has_external_change);
    } else {
        panic!("Expected ModificationStatus result");
    }
}

#[test]
fn test_handle_command_save_new_editor() {
    let mut editor = Editor::new();
    // New editor without file path should fail to save
    let result = editor.handle_command(PanelCommand::Save);

    if let CommandResult::SaveResult { success, error } = result {
        assert!(!success);
        assert!(error.is_some());
    } else {
        panic!("Expected SaveResult");
    }
}

#[test]
fn test_handle_command_save_with_file() {
    let (mut editor, _file) = create_editor_with_content("original");

    // Modify and save
    let _ = editor.insert_char('!');
    let result = editor.handle_command(PanelCommand::Save);

    if let CommandResult::SaveResult { success, error } = result {
        assert!(success);
        assert!(error.is_none());
    } else {
        panic!("Expected SaveResult");
    }

    // Check modification status after save
    let result = editor.handle_command(PanelCommand::GetModificationStatus);
    if let CommandResult::ModificationStatus { is_modified, .. } = result {
        assert!(!is_modified);
    }
}

#[test]
fn test_handle_command_reload() {
    let (mut editor, mut file) = create_editor_with_content("original");

    // Modify file externally
    write!(file, "modified content").unwrap();

    let result = editor.handle_command(PanelCommand::Reload);
    assert!(result.needs_redraw());
}

#[test]
fn test_handle_command_close_without_saving() {
    let (mut editor, _file) = create_editor_with_content("hello");
    editor.file_state.external_change_detected = true;

    let result = editor.handle_command(PanelCommand::CloseWithoutSaving);
    assert!(matches!(result, CommandResult::None));

    // External change flag should be cleared
    assert!(!editor.file_state.external_change_detected);
}

#[test]
fn test_handle_command_not_applicable() {
    let mut editor = Editor::new();

    // Commands not applicable to Editor should return None
    let result = editor.handle_command(PanelCommand::Resize { rows: 24, cols: 80 });
    assert!(matches!(result, CommandResult::None));

    let result = editor.handle_command(PanelCommand::RefreshDirectory);
    assert!(matches!(result, CommandResult::None));

    let result = editor.handle_command(PanelCommand::SetFsWatchRoot {
        root: None,
        is_git_repo: false,
    });
    assert!(matches!(result, CommandResult::None));
}

#[test]
fn test_editor_panel_trait_title() {
    let editor = Editor::new();
    assert_eq!(editor.title(), "Untitled");

    let (editor, _file) = create_editor_with_content("test");
    // Title should be the filename
    assert!(editor.title().ends_with(".tmp") || !editor.title().is_empty());
}

#[test]
fn test_editor_panel_trait_needs_close_confirmation() {
    let editor = Editor::new();
    // New unmodified editor doesn't need confirmation
    assert!(editor.needs_close_confirmation().is_none());

    let (mut editor, _file) = create_editor_with_content("hello");
    let _ = editor.insert_char('x');
    // Modified editor needs confirmation
    assert!(editor.needs_close_confirmation().is_some());
}

// === Large file handling tests ===

fn create_large_file(line_count: usize) -> (Editor, NamedTempFile) {
    let mut file = NamedTempFile::new().unwrap();
    for i in 0..line_count {
        writeln!(
            file,
            "Line {}: content with some text for testing large file behavior",
            i + 1
        )
        .unwrap();
    }
    file.flush().unwrap();
    let editor =
        Editor::open_file_with_config(file.path().to_path_buf(), EditorConfig::default()).unwrap();
    (editor, file)
}

#[test]
fn test_large_file_load_10k_lines() {
    let (editor, _file) = create_large_file(10_000);
    // writeln! adds trailing newline, so we get one extra empty line
    assert!(editor.buffer.line_count() >= 10_000);
    assert_eq!(editor.cursor.line, 0);
    assert_eq!(editor.cursor.column, 0);
}

#[test]
fn test_large_file_viewport_navigation() {
    let (mut editor, _file) = create_large_file(10_000);
    editor.viewport.resize(80, 24);

    // Initial state
    assert_eq!(editor.viewport().top_line, 0);
    assert!(editor.viewport().is_line_visible(0));
    assert!(!editor.viewport().is_line_visible(30));

    // Navigate to middle of file
    editor.set_cursor_line(4999);
    editor
        .viewport
        .ensure_cursor_visible(&editor.cursor, editor.buffer.line_count());
    assert!(editor.viewport().is_cursor_visible(&editor.cursor));
    assert_eq!(editor.cursor.line, 4999);

    // Navigate to end
    editor.set_cursor_line(9999);
    editor
        .viewport
        .ensure_cursor_visible(&editor.cursor, editor.buffer.line_count());
    assert_eq!(editor.cursor.line, 9999);
    assert!(editor.viewport().is_cursor_visible(&editor.cursor));
}

#[test]
fn test_large_file_cursor_movement() {
    let (mut editor, _file) = create_large_file(10_000);
    editor.viewport.resize(80, 24);

    // Move down page by page
    for _ in 0..100 {
        editor.page_down();
    }
    // Should be around line 2400+ (100 pages * ~24 lines)
    assert!(editor.cursor.line > 2000);

    // Move to end
    editor.move_to_document_end();
    // Should be at last line (buffer may have trailing empty line)
    assert_eq!(editor.cursor.line, editor.buffer.line_count() - 1);

    // Move to start
    editor.move_to_document_start();
    assert_eq!(editor.cursor.line, 0);
}

#[test]
fn test_large_file_edit_at_various_positions() {
    let (mut editor, _file) = create_large_file(1_000);
    editor.viewport.resize(80, 24);

    // Edit at beginning
    let _ = editor.insert_char('A');
    assert_eq!(editor.buffer.line(0).unwrap().chars().next().unwrap(), 'A');

    // Edit at middle
    editor.set_cursor_line(499);
    let _ = editor.insert_char('M');
    assert!(editor.buffer.line(499).unwrap().starts_with('M'));

    // Edit at end
    editor.set_cursor_line(999);
    let _ = editor.insert_char('Z');
    assert!(editor.buffer.line(999).unwrap().starts_with('Z'));

    // Verify buffer is modified
    assert!(editor.buffer.is_modified());
}

#[test]
fn test_large_file_undo_redo() {
    let (mut editor, _file) = create_large_file(1_000);

    // Make several edits
    let _ = editor.insert_char('X');
    editor.set_cursor_line(499);
    let _ = editor.insert_char('Y');
    editor.set_cursor_line(999);
    let _ = editor.insert_char('Z');

    // Undo all
    let _ = editor.buffer.undo();
    let _ = editor.buffer.undo();
    let _ = editor.buffer.undo();

    // Buffer should not be modified after full undo
    // (assuming we undid all changes)
    let first_line = editor.buffer.line(0).unwrap();
    assert!(first_line.starts_with("Line 1:"));
}

#[test]
fn test_large_file_search() {
    let (mut editor, _file) = create_large_file(1_000);

    // Search for a line in the middle
    editor.start_search("Line 500:".to_string(), false, false);
    editor.search_next();

    // Cursor should move to line 500 (0-indexed: line 499)
    assert_eq!(editor.cursor.line, 499);
}

#[test]
fn test_large_file_scroll_performance() {
    let (mut editor, _file) = create_large_file(50_000);
    editor.viewport.resize(80, 24);

    // Rapid scrolling should be efficient
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        editor.viewport.scroll_down(10, editor.buffer.line_count());
    }
    let scroll_time = start.elapsed();

    // Should complete in reasonable time (< 100ms for 50K lines)
    assert!(
        scroll_time.as_millis() < 100,
        "Scrolling took too long: {:?}",
        scroll_time
    );

    // Verify we actually scrolled
    assert!(editor.viewport().top_line > 0);
}

#[test]
fn open_php_file_renders_with_colors() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use std::collections::HashSet;

    // Real file-manager path: a .php file on disk opened via
    // `open_file_with_config` (which sets syntax from the extension), then
    // rendered through `render_content` under a light theme.
    let src =
        "<!DOCTYPE html>\n<html>\n<body>\n<?php\n$name = \"Ivan\";\necho $name;\n?>\n</body>\n";
    let file = tempfile::Builder::new().suffix(".php").tempfile().unwrap();
    std::fs::write(file.path(), src).unwrap();

    let mut editor =
        Editor::open_file_with_config(file.path().to_path_buf(), EditorConfig::default()).unwrap();
    assert_eq!(
        editor.render_cache.highlight.current_syntax(),
        Some("php"),
        "open path should detect php from the .php extension"
    );

    let theme = *termide_theme::Theme::get_by_name("github-light");
    editor
        .render_cache
        .highlight
        .set_light_theme(theme.is_light_theme());
    editor.render_cache.highlight.set_default_fg(theme.fg);

    let config = termide_config::Config::default();
    let area = Rect::new(0, 0, 80, 20);
    let mut buf = Buffer::empty(area);
    editor.render_content(area, &mut buf, &theme, &config, true, None);

    let mut colors: HashSet<Color> = HashSet::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(c) = buf.cell((x, y)) {
                if c.symbol() != " " {
                    colors.insert(c.fg);
                }
            }
        }
    }
    assert!(
        colors.len() > 1,
        "expected highlighting via the real open path, saw {colors:?}"
    );
}

#[test]
fn count_virtual_rows_counts_deletion_markers() {
    let (mut editor, file) = create_editor_with_content("a\nb\nc\n");

    // Seed an in-memory HEAD where two lines were later deleted (after line 0
    // and after line 1), producing two deletion markers, without touching git.
    let mut cache = termide_git::GitDiffCache::new(file.path().to_path_buf());
    cache.set_original_content(Some("a\nx\nb\ny\nc\n".to_string()));
    cache.update_from_buffer("a\nb\nc\n").unwrap();
    assert_eq!(
        cache.deletion_marker_count(),
        2,
        "setup: two markers expected"
    );
    editor.git.diff_cache = Some(cache);

    // The helper only counts markers when git diff is enabled in the config.
    let mut cfg = termide_config::Config::default();
    cfg.editor.show_git_diff = true;
    editor.render_cache.config = std::sync::Arc::new(cfg);

    // Markers sit after lines 0 and 1; the range [0, 3) covers both → 2 rows,
    // which is the offset the blame annotation must add to land on the cursor
    // line. A range past the marker lines counts nothing.
    assert_eq!(editor.count_virtual_rows_between(0, 3, 80), 2);
    assert_eq!(editor.count_virtual_rows_between(2, 3, 80), 0);
}

#[test]
fn backspace_word_deletes_previous_word() {
    let (mut editor, _f) = create_editor_with_content("hello world");
    editor.cursor = termide_buffer::Cursor::at(0, 11); // end of "world"
    editor.handle_delete_key(|e| e.backspace_word()).unwrap();
    assert_eq!(editor.buffer.text(), "hello ");
}

#[test]
fn backspace_word_at_start_is_noop() {
    let (mut editor, _f) = create_editor_with_content("abc");
    editor.cursor = termide_buffer::Cursor::at(0, 0);
    editor.handle_delete_key(|e| e.backspace_word()).unwrap();
    assert_eq!(editor.buffer.text(), "abc");
}

#[test]
fn delete_word_forward_deletes_next_word() {
    let (mut editor, _f) = create_editor_with_content("hello world");
    editor.cursor = termide_buffer::Cursor::at(0, 0); // start of "hello"
    editor.handle_delete_key(|e| e.delete_word()).unwrap();
    // Forward word delete removes up to the start of the next word.
    assert_eq!(editor.buffer.text(), "world");
}

#[test]
fn select_line_spans_whole_line() {
    let (editor, _f) = create_editor_with_content("hello world\nsecond");
    let cur = termide_buffer::Cursor::at(0, 3);
    let (sel, new_cur) = crate::selection::select_line(&editor.buffer, &cur).unwrap();
    assert_eq!(sel.start().column, 0);
    assert_eq!(new_cur.column, 11); // end of "hello world"
}

/// Dragging the scrollbar thumb must move the *content*, not just the thumb.
/// The viewport snaps back to the cursor on every render unless the scroll is
/// detached from it — wheel scrolling detaches, so the thumb drag must too.
#[test]
fn set_scroll_offset_moves_the_content_not_just_the_thumb() {
    use ratatui::{buffer::Buffer, layout::Rect};

    let content: String = (0..200).map(|i| format!("line{i}\n")).collect();
    let (mut editor, _f) = create_editor_with_content(&content);
    let theme = *termide_theme::Theme::get_by_name("github-light");
    let config = termide_config::Config::default();
    let area = Rect::new(0, 0, 40, 10);

    let mut buf = Buffer::empty(area);
    editor.render_content(area, &mut buf, &theme, &config, true, None);

    editor.handle_command(PanelCommand::SetScrollOffset {
        axis: termide_core::ScrollAxis::Vertical,
        offset: 50,
    });
    editor.render_content(area, &mut buf, &theme, &config, true, None);

    assert_eq!(
        editor.viewport.top_line, 50,
        "render snapped the viewport back to the cursor"
    );
    let first_row: String = (0..area.width)
        .map(|x| buf[(x, 0)].symbol().to_string())
        .collect();
    assert!(
        first_row.contains("line50"),
        "content did not follow the scroll offset: {first_row:?}"
    );
}

/// Push the file's mtime past what the editor recorded, so a rewrite within
/// the same clock tick still counts as newer.
fn rewrite_later(file: &NamedTempFile, content: &str) {
    std::fs::write(file.path(), content).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    file.as_file().set_modified(later).unwrap();
}

#[test]
fn a_clean_buffer_follows_the_file_on_disk_and_keeps_its_place() {
    let (mut editor, file) = create_editor_with_content("a\nb\nc\nd\ne\nf\n");
    editor.cursor.line = 4;
    editor.cursor.column = 1;
    editor.viewport.top_line = 3;

    rewrite_later(&file, "a\nb\nc2\n");
    let result = editor.handle_command(PanelCommand::OnFsUpdate {
        changed_path: file.path(),
    });
    assert!(result.needs_redraw());

    assert_eq!(editor.buffer().text(), "a\nb\nc2\n");
    assert!(!editor.has_external_change());
    assert!(!editor.buffer().is_modified());
    // Clamped to the shorter file rather than reset to the top.
    let last = editor.buffer().line_count() - 1;
    assert_eq!(editor.cursor.line, last);
    assert!(editor.viewport.top_line <= last);

    // A rewrite that keeps the line leaves the cursor exactly where it was.
    editor.cursor.line = 1;
    editor.cursor.column = 1;
    rewrite_later(&file, "a\nbb\nc3\n");
    editor.handle_command(PanelCommand::OnFsUpdate {
        changed_path: file.path(),
    });
    assert_eq!(editor.buffer().text(), "a\nbb\nc3\n");
    assert_eq!((editor.cursor.line, editor.cursor.column), (1, 1));
}

#[test]
fn a_buffer_with_unsaved_work_keeps_it_and_flags_the_conflict() {
    let (mut editor, file) = create_editor_with_content("a\nb\n");
    editor.insert_text("x").unwrap();
    assert!(editor.buffer().is_modified());

    rewrite_later(&file, "a\nb\nc\n");
    editor.handle_command(PanelCommand::OnFsUpdate {
        changed_path: file.path(),
    });

    assert!(editor.has_external_change());
    assert!(editor.buffer().text().starts_with("xa\nb\n"));
    assert!(editor.buffer().is_modified());
}

/// A TAB (or any control character) in a line must not reach a cell: the
/// host terminal would move its cursor to the next tab stop and every cell
/// written after it would land off by the difference (#55). A TAB is drawn
/// as blanks up to its tab stop, any other control character as one blank.
#[test]
fn control_characters_never_reach_the_frame() {
    use ratatui::{buffer::Buffer, layout::Rect};

    for word_wrap in [false, true] {
        let (mut editor, _file) = create_editor_with_content("\tname = 1,\r\nx\u{1b}y\r\n");
        editor.config.word_wrap = word_wrap;
        let theme = *termide_theme::Theme::get_by_name("github-light");
        let config = termide_config::Config::default();
        let area = Rect::new(0, 0, 40, 5);
        let mut buf = Buffer::empty(area);
        editor.render_content(area, &mut buf, &theme, &config, true, None);

        let row = |y: u16| -> String { (0..area.width).map(|x| buf[(x, y)].symbol()).collect() };
        for y in 0..area.height {
            assert!(
                !row(y).contains(char::is_control),
                "word_wrap={word_wrap}: row {y} carries a control character: {:?}",
                row(y)
            );
        }
        let first = row(0);
        let name_col = first.find("name").expect("line text rendered");
        assert_eq!(
            &first[name_col - 1..name_col],
            " ",
            "word_wrap={word_wrap}: the TAB is drawn blank: {first:?}"
        );
        assert!(
            row(1).contains("x y"),
            "word_wrap={word_wrap}: ESC becomes a blank: {:?}",
            row(1)
        );
    }
}

#[test]
fn a_click_in_the_find_bar_places_its_cursor_and_a_drag_selects() {
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_core::RenderContext;

    let (mut editor, _file) = create_editor_with_content("hello\n");
    editor.open_find_bar(false);
    let typed = |editor: &mut Editor, c| {
        editor.handle_find_bar_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    };
    for c in "needle".chars() {
        typed(&mut editor, c);
    }
    let colors = termide_core::ThemeColors::default();
    let ctx = RenderContext {
        theme: &colors,
        config: &termide_core::PanelConfig {
            tab_size: 4,
            word_wrap: false,
            show_line_numbers: false,
            show_hidden_files: false,
        },
        is_focused: true,
        panel_index: 0,
        terminal_width: 60,
        terminal_height: 12,
        border_right_x: Some(59),
        border_bottom_y: Some(11),
    };
    let area = Rect::new(0, 0, 60, 12);
    let mut buf = Buffer::empty(area);
    Panel::render(&mut editor, area, &mut buf, &ctx);
    let (row, start) = (0..area.height)
        .find_map(|y| {
            // Cells, not bytes: the row holds multi-byte glyphs such as `›`.
            let cells: Vec<String> = (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            (0..cells.len().saturating_sub(6))
                .find(|&x| cells[x..x + 6].concat() == "needle")
                .map(|x| (y, x as u16))
        })
        .expect("the find field is drawn");
    let at = |kind, column| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let panel_area = Rect::new(0, 0, 62, 14);

    // A click after the "n" places the cursor there.
    Panel::handle_mouse(
        &mut editor,
        at(MouseEventKind::Down(MouseButton::Left), start + 1),
        panel_area,
    );
    Panel::handle_mouse(
        &mut editor,
        at(MouseEventKind::Up(MouseButton::Left), start + 1),
        panel_area,
    );
    typed(&mut editor, 'X');
    assert_eq!(editor.find_bar.as_ref().unwrap().find_text(), "nXeedle");

    // A drag back over "Xee" selects it, and typing replaces the selection.
    // (It starts at the other end: a quick second press where the first
    // landed would be a double click.)
    Panel::handle_mouse(
        &mut editor,
        at(MouseEventKind::Down(MouseButton::Left), start + 4),
        panel_area,
    );
    Panel::handle_mouse(
        &mut editor,
        at(MouseEventKind::Drag(MouseButton::Left), start + 1),
        panel_area,
    );
    Panel::handle_mouse(
        &mut editor,
        at(MouseEventKind::Up(MouseButton::Left), start + 1),
        panel_area,
    );
    typed(&mut editor, 'Y');
    assert_eq!(editor.find_bar.as_ref().unwrap().find_text(), "nYdle");
}

/// Render `content` into a `width`-column frame and return its rows with the
/// line-number gutter cut off.
fn rendered_text_rows(editor: &mut Editor, width: u16, height: u16) -> Vec<String> {
    use ratatui::{buffer::Buffer, layout::Rect};

    let theme = *termide_theme::Theme::get_by_name("github-light");
    let config = termide_config::Config::default();
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    editor.render_content(area, &mut buf, &theme, &config, true, None);
    let gutter = crate::rendering::line_number_width(editor.buffer.line_count()) as u16;
    (0..height)
        .map(|y| (gutter..width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

/// A TAB reaches the next multiple of the tab size, with and without word
/// wrap, and follows a per-editor tab size change.
#[test]
fn tabs_expand_to_the_next_tab_stop() {
    for word_wrap in [false, true] {
        let (mut editor, _file) = create_editor_with_content("\tname\nab\tc\n");
        editor.config.word_wrap = word_wrap;
        editor.config.tab_size = 4;
        let rows = rendered_text_rows(&mut editor, 30, 4);
        assert!(
            rows[0].starts_with("    name "),
            "word_wrap={word_wrap}: {rows:?}"
        );
        assert!(
            rows[1].starts_with("ab  c "),
            "word_wrap={word_wrap}: {rows:?}"
        );

        editor.config.tab_size = 8;
        let rows = rendered_text_rows(&mut editor, 30, 4);
        assert!(
            rows[0].starts_with("        name "),
            "word_wrap={word_wrap}: {rows:?}"
        );
        assert!(
            rows[1].starts_with("ab      c "),
            "word_wrap={word_wrap}: {rows:?}"
        );
    }
}

/// Wrap points are measured with tabs expanded, and a tab size change
/// re-wraps lines whose rows were cached with the old one.
#[test]
fn a_tab_size_change_rewraps_the_line() {
    let (mut editor, _file) = create_editor_with_content("ab\tcdefgh\nz\n");
    editor.config.word_wrap = true;
    editor.config.tab_size = 4;
    let gutter = crate::rendering::line_number_width(editor.buffer.line_count()) as u16;
    // Ten content columns: "ab" + a two-column tab + "cdefgh" fill one row.
    let rows = rendered_text_rows(&mut editor, gutter + 10, 4);
    assert_eq!(rows[0], "ab  cdefgh");
    assert!(rows[1].starts_with('z'), "{rows:?}");

    // With tab stops every 8 columns the tab takes six, so the row wraps.
    editor.config.tab_size = 8;
    let rows = rendered_text_rows(&mut editor, gutter + 10, 4);
    assert!(rows[0].starts_with("ab      "), "{rows:?}");
    assert!(rows[1].starts_with("cdefgh"), "{rows:?}");
    assert!(rows[2].starts_with('z'), "{rows:?}");
}

/// Without word wrap the view scrolls by display columns: a cursor past a
/// few tabs is scrolled into view even though its grapheme index is small.
#[test]
fn horizontal_scroll_counts_tab_columns() {
    let (mut editor, _file) = create_editor_with_content("\t\t\t\tabc\n");
    editor.config.word_wrap = false;
    editor.config.tab_size = 4;
    editor.cursor = termide_buffer::Cursor::at(0, 5); // on the "b", column 17
    let gutter = crate::rendering::line_number_width(editor.buffer.line_count()) as u16;
    let rows = rendered_text_rows(&mut editor, gutter + 10, 2);
    assert_eq!(editor.cursor_in_display_columns().column, 17);
    assert_eq!(editor.viewport().left_column, 8);
    // Columns 8..18: the last two tabs, then "ab" with the cursor on "b".
    assert_eq!(rows[0], "        ab");
}

/// Up and down keep the cursor in its screen column across lines indented
/// differently, with and without word wrap: from column 8 on a line indented
/// with spaces the cursor lands on the grapheme drawn there on a tab line.
#[test]
fn vertical_movement_keeps_the_screen_column_across_tabs() {
    for word_wrap in [false, true] {
        let (mut editor, _file) = create_editor_with_content("        xy\n\t\txy\n        xy\n");
        editor.config.word_wrap = word_wrap;
        editor.config.tab_size = 4;
        let gutter = crate::rendering::line_number_width(editor.buffer.line_count()) as u16;
        rendered_text_rows(&mut editor, gutter + 30, 5);
        editor.cursor = termide_buffer::Cursor::at(0, 9); // on "y", column 9

        if word_wrap {
            editor.move_cursor_down_visual();
        } else {
            editor.move_cursor_down();
        }
        assert_eq!(
            (editor.cursor.line, editor.cursor.column),
            (1, 3),
            "word_wrap={word_wrap}: \"y\" after two tabs"
        );

        if word_wrap {
            editor.move_cursor_down_visual();
        } else {
            editor.move_cursor_down();
        }
        assert_eq!(
            (editor.cursor.line, editor.cursor.column),
            (2, 9),
            "word_wrap={word_wrap}: back on the spaces line"
        );
    }
}

/// A diagnostic's underline sits under the text as drawn: its LSP range
/// counts UTF-16 units (an emoji is two), and the tab before it is expanded.
#[test]
fn a_diagnostic_underline_follows_tabs_and_wide_text() {
    use lsp_types::{Diagnostic, Position, Range};

    for word_wrap in [false, true] {
        // "😀" is two columns and two UTF-16 units; the tab then reaches
        // column 4, so "foo" (UTF-16 3..6) is drawn in columns 4..7.
        let (mut editor, _file) = create_editor_with_content("😀\tfoo bar\n");
        editor.config.word_wrap = word_wrap;
        editor.config.tab_size = 4;
        editor.lsp.diagnostics = vec![Diagnostic {
            range: Range::new(Position::new(0, 3), Position::new(0, 6)),
            message: "boom".to_string(),
            ..Default::default()
        }];
        let rows = rendered_text_rows(&mut editor, 40, 4);
        assert!(rows[0].starts_with("😀"), "word_wrap={word_wrap}: {rows:?}");
        assert!(
            rows[1].starts_with("    ~~~ boom"),
            "word_wrap={word_wrap}: {rows:?}"
        );
    }
}

/// The completion popup opens on the row under the cursor, starting at the
/// cursor's column, as drawn: after tabs and with the view scrolled sideways.
#[test]
fn the_completion_popup_opens_under_the_drawn_cursor() {
    use lsp_types::{CompletionItem, CompletionResponse};

    let (mut editor, _file) = create_editor_with_content("\t\tfoo\nbar\n");
    editor.config.word_wrap = false;
    editor.config.tab_size = 4;
    editor.cursor = termide_buffer::Cursor::at(0, 3); // on "o", column 9
    editor.lsp.completion_popup = Some(crate::completion_popup::CompletionPopup::from_response(
        CompletionResponse::Array(vec![CompletionItem::new_simple(
            "food".into(),
            String::new(),
        )]),
    ));
    let gutter = crate::rendering::line_number_width(editor.buffer.line_count()) as u16;

    rendered_text_rows(&mut editor, 40, 10);
    let rect = editor.lsp.popup_rect.expect("popup drawn");
    assert_eq!((rect.x, rect.y), (gutter + 9, 1));

    // Six content columns: the view scrolls so column 9 is the last one.
    rendered_text_rows(&mut editor, gutter + 6, 10);
    assert_eq!(editor.viewport().left_column, 4);
    let rect = editor.lsp.popup_rect.expect("popup drawn");
    assert_eq!(rect.y, 1);
    assert!(rect.x <= gutter + 5, "{rect:?}");
}

/// A diagnostic repeated on a line is drawn once, and the rows the cursor
/// and blame are shifted by count it once too.
#[test]
fn a_repeated_diagnostic_is_counted_as_drawn() {
    use lsp_types::{Diagnostic, Position, Range};

    let (mut editor, _file) = create_editor_with_content("foo\nbar\n");
    let diag = Diagnostic {
        range: Range::new(Position::new(0, 0), Position::new(0, 3)),
        message: "boom".to_string(),
        ..Default::default()
    };
    editor.lsp.diagnostics = vec![diag.clone()];
    let once = editor.count_virtual_rows_between(0, 1, 40);
    assert_eq!(once, 1);

    editor.lsp.diagnostics = vec![diag.clone(), diag];
    assert_eq!(editor.count_virtual_rows_between(0, 1, 40), once);
    let rows = rendered_text_rows(&mut editor, 40, 4);
    assert!(rows[1].starts_with("~~~ boom"), "{rows:?}");
    assert!(rows[2].starts_with("bar"), "{rows:?}");
}

/// Without word wrap too, passing through a short line keeps the screen
/// column: the cursor comes back to it on the next long line. A cursor set
/// by anything else (a click, a jump) starts from its own column.
#[test]
fn a_short_line_does_not_lose_the_column() {
    let (mut editor, _file) = create_editor_with_content("abcdefgh\nab\n\tefgh\nabcdefgh\n");
    editor.config.word_wrap = false;
    editor.config.tab_size = 4;
    rendered_text_rows(&mut editor, 40, 6);
    editor.cursor = termide_buffer::Cursor::at(0, 6);

    editor.move_cursor_down();
    assert_eq!((editor.cursor.line, editor.cursor.column), (1, 2));
    editor.move_cursor_down();
    // Column 6 on "\tefgh" is the "g" (the tab spans columns 0..4).
    assert_eq!((editor.cursor.line, editor.cursor.column), (2, 3));
    editor.move_cursor_down();
    assert_eq!((editor.cursor.line, editor.cursor.column), (3, 6));

    // Placed elsewhere, the cursor keeps its own column from there.
    editor.cursor = termide_buffer::Cursor::at(3, 1);
    editor.move_cursor_up();
    assert_eq!((editor.cursor.line, editor.cursor.column), (2, 0));
}

#[test]
fn clipboard_commands_edit_the_focused_find_bar_not_the_buffer() {
    termide_ui::clipboard::use_fake_clipboard("ell");
    let (mut editor, _file) = create_editor_with_content("hello\n");
    editor.open_find_bar(false);

    // A terminal paste lands in the query and re-runs the search.
    let result = editor.handle_command(PanelCommand::PasteText { text: "he".into() });
    assert!(matches!(result, CommandResult::Handled(true)));
    assert_eq!(editor.find_bar.as_ref().unwrap().find_text(), "he");
    assert_eq!(editor.buffer.line(0).as_deref(), Some("hello\n"));
    let matches = |editor: &Editor| editor.search.state.as_ref().map(|s| s.matches.len());
    assert_eq!(matches(&editor), Some(1));

    // Ctrl+A in the bar, then Ctrl+V replaces the query with the clipboard.
    editor.handle_find_bar_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('a'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    assert!(matches!(
        editor.handle_command(PanelCommand::Paste),
        CommandResult::Handled(true)
    ));
    assert_eq!(editor.find_bar.as_ref().unwrap().find_text(), "ell");
    assert_eq!(editor.buffer.line(0).as_deref(), Some("hello\n"));

    // Ctrl+X takes the query's selection out and the search follows it.
    editor.handle_find_bar_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('a'),
        crossterm::event::KeyModifiers::CONTROL,
    ));
    assert!(matches!(
        editor.handle_command(PanelCommand::Cut),
        CommandResult::Handled(true)
    ));
    assert_eq!(editor.find_bar.as_ref().unwrap().find_text(), "");
    assert_eq!(termide_ui::clipboard::paste().as_deref(), Some("ell"));
    assert_eq!(editor.buffer.line(0).as_deref(), Some("hello\n"));

    // With focus on the buffer the paste goes into the text again.
    editor.find_bar_focus_buffer = true;
    editor.handle_command(PanelCommand::PasteText { text: "X".into() });
    assert_eq!(editor.find_bar.as_ref().unwrap().find_text(), "");
    assert!(editor.buffer.line(0).unwrap().contains('X'));
}
