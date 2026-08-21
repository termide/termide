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
