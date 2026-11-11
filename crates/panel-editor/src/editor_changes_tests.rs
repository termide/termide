use std::path::{Path, PathBuf};
use std::process::Command;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_core::{KeyChord, Panel};

use super::super::build_editor_hotkey_table;
use crate::{
    Editor, EditorConfig, REVERT_CHANGE_ACTION, STAGE_CHANGE_ACTION, UNSTAGE_CHANGE_ACTION,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

const COMMITTED: &str = "a\nb\nc\nd\ne\nf\ng\nh\n";

/// An editor on `f` of a repository where it is committed as
/// [`COMMITTED`], its git markers loaded.
fn editor() -> (tempfile::TempDir, PathBuf, Editor) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f"), COMMITTED).unwrap();
    git(&repo, &["add", "f"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let mut e = Editor::open_file_with_config(repo.join("f"), EditorConfig::default()).unwrap();
    let mut config = termide_config::Config::default();
    config.normalize();
    e.hotkeys = build_editor_hotkey_table(&config);
    settle(&mut e);
    (dir, repo, e)
}

/// Let the git markers catch up with the index and the text.
fn settle(e: &mut Editor) {
    e.update_git_diff();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !e.check_git_diff_receiver() {
        assert!(std::time::Instant::now() < deadline, "no git diff");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let text = e.buffer.to_string();
    e.git
        .diff_cache
        .as_mut()
        .unwrap()
        .update_from_buffer(&text)
        .unwrap();
}

/// Replace line `line` (0-based) of the text with `text`, unsaved.
fn type_over(e: &mut Editor, line: usize, text: &str) {
    let start = termide_buffer::Cursor::at(line, 0);
    let end = termide_buffer::Cursor::at(line, e.buffer.line_len_graphemes(line));
    e.buffer.delete_range(&start, &end).unwrap();
    e.buffer.insert(&start, text).unwrap();
}

fn actions(e: &Editor) -> Vec<String> {
    Panel::context_menu_items(e)
        .into_iter()
        .map(|(_, action)| action)
        .filter(|a| {
            [
                STAGE_CHANGE_ACTION,
                UNSTAGE_CHANGE_ACTION,
                REVERT_CHANGE_ACTION,
            ]
            .contains(&a.as_str())
        })
        .collect()
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
    KeyChord::identity(KeyEvent::new(code, modifiers))
}

#[test]
fn an_unsaved_change_is_staged_from_the_menu_and_drawn_hollow() {
    let (_dir, repo, mut e) = editor();
    type_over(&mut e, 1, "B");
    type_over(&mut e, 6, "G");
    settle(&mut e);
    e.cursor.line = 1;
    assert_eq!(actions(&e), [STAGE_CHANGE_ACTION, REVERT_CHANGE_ACTION]);

    e.handle_status_action(STAGE_CHANGE_ACTION);
    settle(&mut e);
    assert_eq!(git(&repo, &["show", ":f"]), "a\nB\nc\nd\ne\nf\ng\nh\n");
    let cache = e.git.diff_cache.as_ref().unwrap();
    assert!(cache.is_staged(1));
    assert!(!cache.is_staged(6));
    // Hollow: the change's colour on the number, no background; the
    // unstaged one keeps its filled cell.
    let theme = termide_theme::Theme::default();
    let style = |line| crate::git::get_line_number_git_style(line, &e.git.diff_cache, true, &theme);
    assert_eq!(style(1), (theme.warning, None));
    assert_eq!(style(6).1, Some(theme.warning));
    assert_eq!(actions(&e), [UNSTAGE_CHANGE_ACTION]);
    // Nothing reached the file: the text is still unsaved.
    assert_eq!(std::fs::read_to_string(repo.join("f")).unwrap(), COMMITTED);

    e.handle_status_action(UNSTAGE_CHANGE_ACTION);
    settle(&mut e);
    assert_eq!(git(&repo, &["show", ":f"]), COMMITTED);
}

#[test]
fn a_revert_puts_the_lines_back_and_undo_takes_it_back() {
    let (_dir, _repo, mut e) = editor();
    type_over(&mut e, 3, "D");
    settle(&mut e);
    e.cursor.line = 3;
    e.handle_status_action(REVERT_CHANGE_ACTION);
    assert_eq!(e.buffer.to_string(), COMMITTED);
    e.handle_key(key(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(e.buffer.to_string(), "a\nb\nc\nD\ne\nf\ng\nh\n");
}

#[test]
fn f7_steps_through_the_changes_round_the_end() {
    let (_dir, _repo, mut e) = editor();
    type_over(&mut e, 1, "B");
    type_over(&mut e, 2, "C");
    type_over(&mut e, 5, "F");
    settle(&mut e);
    e.cursor.line = 0;
    e.handle_key(key(KeyCode::F(7), KeyModifiers::NONE));
    assert_eq!(e.cursor.line, 1);
    e.handle_key(key(KeyCode::F(7), KeyModifiers::NONE));
    assert_eq!(e.cursor.line, 5);
    e.handle_key(key(KeyCode::F(7), KeyModifiers::NONE));
    assert_eq!(e.cursor.line, 1);
    e.handle_key(key(KeyCode::F(7), KeyModifiers::SHIFT));
    assert_eq!(e.cursor.line, 5);
}

#[test]
fn an_unchanged_line_offers_nothing() {
    let (_dir, _repo, mut e) = editor();
    type_over(&mut e, 1, "B");
    settle(&mut e);
    e.cursor.line = 4;
    assert!(actions(&e).is_empty());
}
