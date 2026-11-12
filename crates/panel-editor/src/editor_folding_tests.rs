use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_core::{KeyChord, Panel};

use super::super::build_editor_hotkey_table;
use crate::{Editor, EditorConfig, FOLD_ALL_ACTION, UNFOLD_ALL_ACTION};

/// Lines: 0 `fn foo() {`, 1-4 its body with an `if` block at 2-3, 5 `}`,
/// 6 `fn bar() {`, 7 its body, 8 `}`, 9 `end`. By the indentation: (0, 4),
/// (2, 3), (6, 7).
const TEXT: &str = "fn foo() {\n    let a = 1;\n    if a {\n        b();\n    }\n}\nfn bar() {\n    c();\n}\nend\n";

/// An editor on [`TEXT`] in a file no language server reads, so folds go by
/// the indentation at once.
fn editor(config: EditorConfig) -> (tempfile::NamedTempFile, Editor) {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), TEXT).unwrap();
    let mut e = Editor::open_file_with_config(file.path().to_path_buf(), config).unwrap();
    let mut config = termide_config::Config::default();
    config.normalize();
    e.hotkeys = build_editor_hotkey_table(&config);
    (file, e)
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
    KeyChord::identity(KeyEvent::new(code, modifiers))
}

fn alt(c: char) -> KeyChord {
    key(KeyCode::Char(c), KeyModifiers::ALT)
}

fn press(e: &mut Editor, code: KeyCode) {
    e.handle_key(key(code, KeyModifiers::NONE));
}

fn folds(e: &Editor) -> Vec<(usize, usize)> {
    e.buffer.folds().iter().map(|f| (f.start, f.end)).collect()
}

/// The text rows of a render, the gutter left out.
fn rows(e: &mut Editor, width: u16, height: u16) -> Vec<String> {
    use ratatui::{buffer::Buffer, layout::Rect};
    let theme = *termide_theme::Theme::get_by_name("github-light");
    let config = termide_config::Config::default();
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    e.render_content(area, &mut buf, &theme, &config, true, None);
    let gutter = crate::rendering::line_number_width(e.buffer.line_count()) as u16;
    (0..height)
        .map(|y| (gutter..width).map(|x| buf[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn folding_again_takes_the_enclosing_block_and_unfolding_the_header_opens_it() {
    let (_f, mut e) = editor(EditorConfig::default());
    e.cursor.line = 3;
    e.handle_key(alt(','));
    assert_eq!(folds(&e), [(2, 3)]);
    assert_eq!(e.cursor.line, 2, "the cursor goes to the header");

    e.handle_key(alt(','));
    assert_eq!(folds(&e), [(0, 4), (2, 3)]);
    assert_eq!(e.cursor.line, 0);

    e.handle_key(alt('.'));
    assert_eq!(folds(&e), [(2, 3)], "the inner fold stays");
}

#[test]
fn arrows_step_over_a_fold_and_the_header_shows_what_it_hides() {
    for word_wrap in [false, true] {
        let (_f, mut e) = editor(EditorConfig::default());
        e.config.word_wrap = word_wrap;
        e.handle_key(alt(','));
        assert_eq!(folds(&e), [(0, 4)], "word_wrap={word_wrap}");

        let shown = rows(&mut e, 40, 4);
        assert!(shown[0].starts_with("fn foo() { ⋯ 4 lines"), "{shown:?}");
        assert!(shown[1].starts_with('}'), "{shown:?}");
        assert!(shown[2].starts_with("fn bar()"), "{shown:?}");

        press(&mut e, KeyCode::Down);
        assert_eq!(e.cursor.line, 5, "word_wrap={word_wrap}");
        press(&mut e, KeyCode::Up);
        assert_eq!(e.cursor.line, 0, "word_wrap={word_wrap}");
        // Right at the end of the header goes on past the fold.
        press(&mut e, KeyCode::End);
        press(&mut e, KeyCode::Right);
        assert_eq!(e.cursor.line, 5, "word_wrap={word_wrap}");
    }
}

#[test]
fn the_cursor_landing_inside_a_fold_opens_it() {
    let (_f, mut e) = editor(EditorConfig::default());
    e.handle_key(alt(','));
    // As a search or a jump puts it.
    e.cursor = termide_buffer::Cursor::at(3, 4);
    rows(&mut e, 40, 4);
    assert!(folds(&e).is_empty());
    assert_eq!(e.cursor.line, 3);
}

#[test]
fn a_page_down_counts_the_lines_that_show() {
    let (_f, mut e) = editor(EditorConfig::default());
    e.config.word_wrap = false;
    e.handle_status_action(FOLD_ALL_ACTION);
    assert_eq!(folds(&e), [(0, 4), (2, 3), (6, 7)]);
    // Lines that show: 0, 5, 6, 8, 9.
    let shown = rows(&mut e, 40, 3);
    assert!(shown[1].starts_with('}') && shown[2].starts_with("fn bar()"));
    press(&mut e, KeyCode::PageDown);
    assert!(!e.buffer.is_line_hidden(e.cursor.line));
    assert!(e.cursor.line >= 6, "moved {}", e.cursor.line);
}

#[test]
fn fold_all_and_unfold_all_come_from_the_menu() {
    let (_f, mut e) = editor(EditorConfig::default());
    let menu = |e: &Editor| -> Vec<String> {
        Panel::context_menu_items(e)
            .into_iter()
            .map(|(_, action)| action)
            .filter(|a| [FOLD_ALL_ACTION, UNFOLD_ALL_ACTION].contains(&a.as_str()))
            .collect()
    };
    assert_eq!(menu(&e), [FOLD_ALL_ACTION]);
    e.handle_status_action(FOLD_ALL_ACTION);
    assert_eq!(folds(&e).len(), 3);
    assert_eq!(menu(&e), [FOLD_ALL_ACTION, UNFOLD_ALL_ACTION]);
    e.handle_status_action(UNFOLD_ALL_ACTION);
    assert!(folds(&e).is_empty());
}

#[test]
fn vim_z_commands_fold_and_unfold() {
    let config = EditorConfig {
        vim_mode: true,
        ..EditorConfig::default()
    };
    let (_f, mut e) = editor(config);
    let z = |e: &mut Editor, c: char| {
        press(e, KeyCode::Char('z'));
        press(e, KeyCode::Char(c));
    };
    e.cursor.line = 3;
    z(&mut e, 'c');
    assert_eq!(folds(&e), [(2, 3)]);
    z(&mut e, 'o');
    assert!(folds(&e).is_empty());
    z(&mut e, 'a');
    assert_eq!(folds(&e), [(2, 3)]);
    z(&mut e, 'a');
    assert!(folds(&e).is_empty());
    z(&mut e, 'M');
    assert_eq!(folds(&e).len(), 3);
    z(&mut e, 'R');
    assert!(folds(&e).is_empty());
}

#[test]
fn an_edit_inside_a_fold_drops_it_and_one_on_the_header_keeps_it() {
    let (_f, mut e) = editor(EditorConfig::default());
    e.handle_key(alt(','));
    press(&mut e, KeyCode::End);
    press(&mut e, KeyCode::Char('x'));
    assert_eq!(folds(&e), [(0, 4)]);
    // Lines added above shift it.
    e.cursor = termide_buffer::Cursor::at(0, 0);
    press(&mut e, KeyCode::Enter);
    assert_eq!(folds(&e), [(1, 5)]);
    let start = termide_buffer::Cursor::at(3, 0);
    e.buffer.insert(&start, "z").unwrap();
    assert!(folds(&e).is_empty());
}

#[test]
fn a_server_answer_too_late_for_one_fold_serves_the_next() {
    use std::time::{Duration, Instant};

    use crate::folding::FoldAction;

    let (_f, mut e) = editor(EditorConfig::default());
    e.cursor.line = 1;
    let (tx, rx) = std::sync::mpsc::channel();
    e.folding.pending = Some(FoldAction::Fold);
    let asked = Instant::now() - Duration::from_millis(600);
    e.folding.rx = Some((rx, e.buffer.edit_version(), asked));

    assert!(e.poll_folding_ranges(), "the fold no longer waits");
    assert_eq!(folds(&e), [(0, 4)], "it goes by the indentation");

    // The server's block takes the closing brace too.
    let range = lsp_types::FoldingRange {
        start_line: 0,
        end_line: 5,
        ..Default::default()
    };
    tx.send(Some(vec![range])).unwrap();
    assert!(!e.poll_folding_ranges(), "nothing waits on it any more");
    assert!(e.folding.rx.is_none());

    e.handle_key(alt('.'));
    e.handle_key(alt(','));
    assert!(!e.folding.request_wanted, "the ranges are known");
    assert_eq!(folds(&e), [(0, 5)]);
}
