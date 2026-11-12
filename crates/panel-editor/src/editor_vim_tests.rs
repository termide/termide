use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_core::{KeyChord, Panel};

use crate::{Editor, EditorConfig};

const TEXT: &str = "one\ntwo\nthree\nfour\nfive";

/// A Vim-mode editor on [`TEXT`] with the cursor on line `line`.
fn editor(line: usize) -> (tempfile::NamedTempFile, Editor) {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), TEXT).unwrap();
    let config = EditorConfig {
        vim_mode: true,
        ..EditorConfig::default()
    };
    let mut e = Editor::open_file_with_config(file.path().to_path_buf(), config).unwrap();
    e.cursor.line = line;
    (file, e)
}

fn type_keys(e: &mut Editor, keys: &str) {
    for c in keys.chars() {
        let modifiers = if c.is_ascii_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        e.handle_key(KeyChord::identity(KeyEvent::new(
            KeyCode::Char(c),
            modifiers,
        )));
    }
}

fn after(line: usize, keys: &str) -> String {
    let (_f, mut e) = editor(line);
    type_keys(&mut e, keys);
    assert!(
        !e.vim.as_ref().unwrap().has_pending(),
        "{keys}: the operator is spent"
    );
    e.buffer.text()
}

#[test]
fn an_operator_with_k_takes_the_lines_above() {
    assert_eq!(after(2, "dk"), "one\nfour\nfive");
    assert_eq!(after(3, "d2k"), "one\nfive");
    assert_eq!(after(2, "dj"), "one\ntwo\nfive");
}

#[test]
fn an_operator_with_g_or_gg_takes_the_lines_to_the_file_end_or_start() {
    assert_eq!(after(2, "dG"), "one\ntwo");
    assert_eq!(after(2, "dgg"), "four\nfive");
    assert_eq!(after(0, "d2G"), "three\nfour\nfive");
    assert_eq!(after(4, "d4gg"), "one\ntwo\nthree");
    assert_eq!(after(4, "dgg"), "");
}

#[test]
fn deleting_the_last_line_leaves_no_empty_line() {
    assert_eq!(after(4, "dd"), "one\ntwo\nthree\nfour");
    assert_eq!(after(3, "dj"), "one\ntwo\nthree");
}

#[test]
fn an_operator_with_0_takes_the_line_start() {
    let (_f, mut e) = editor(2);
    e.cursor.column = 3;
    type_keys(&mut e, "d0");
    assert_eq!(e.buffer.text(), "one\ntwo\nee\nfour\nfive");
    assert!(!e.vim.as_ref().unwrap().has_pending());
}

#[test]
fn escape_after_an_operator_and_a_prefix_drops_both() {
    for keys in ["dg", "d\u{17}", "2g"] {
        let (_f, mut e) = editor(2);
        for c in keys.chars() {
            let chord = match c {
                '\u{17}' => KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
                c => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            };
            e.handle_key(KeyChord::identity(chord));
        }
        e.handle_key(KeyChord::identity(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(!e.vim.as_ref().unwrap().has_pending(), "{keys:?} Esc");
        assert_eq!(e.buffer.text(), TEXT);
    }
}
