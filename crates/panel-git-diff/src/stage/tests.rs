use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_config::Config;
use termide_core::{KeyChord, Panel, PanelCommand};

use super::*;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false", "-c", "color.ui=never"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

const ORIGINAL: usize = 20;

/// A repository whose `f` has twenty lines committed and lines 2 and 19
/// changed in the working tree. The panel draws it as:
///
/// ```text
/// 0  file f
/// 1  @@ hunk 0      2..7  " line 1", "-line 2", "+line two", " line 3"...
/// 8  @@ hunk 1      9..14 " line 16", ..., "-line 19", "+line nineteen", ...
/// ```
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    let lines: Vec<String> = (1..=ORIGINAL).map(|n| format!("line {n}")).collect();
    std::fs::write(repo.join("f"), lines.join("\n") + "\n").unwrap();
    git(repo, &["add", "f"]);
    git(repo, &["commit", "-q", "-m", "one"]);
    let mut changed = lines.clone();
    changed[1] = "line two".into();
    changed[18] = "line nineteen".into();
    std::fs::write(repo.join("f"), changed.join("\n") + "\n").unwrap();
    dir
}

/// Wait until the panel shows what git has now.
fn reload(panel: &mut GitDiffPanel) {
    for _ in 0..500 {
        if panel.poll_refresh() && panel.refresh_rx.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the refresh worker never delivered a result");
}

fn panel(repo: &Path) -> GitDiffPanel {
    let mut panel = GitDiffPanel::new(repo.to_path_buf());
    let mut config = Config::default();
    config.normalize();
    panel.hotkeys = crate::build_git_diff_hotkey_table(&config);
    panel.visible_height = 40;
    reload(&mut panel);
    panel
}

fn press(panel: &mut GitDiffPanel, code: KeyCode, modifiers: KeyModifiers) -> Vec<PanelEvent> {
    panel.handle_key(KeyChord::identity(KeyEvent::new(code, modifiers)))
}

fn key(panel: &mut GitDiffPanel, c: char) -> Vec<PanelEvent> {
    press(panel, KeyCode::Char(c), KeyModifiers::NONE)
}

fn down(panel: &mut GitDiffPanel, times: usize) {
    for _ in 0..times {
        press(panel, KeyCode::Down, KeyModifiers::NONE);
    }
}

fn staged(repo: &Path) -> String {
    git(repo, &["diff", "--cached"])
}

#[test]
fn space_on_a_hunk_header_stages_that_hunk_alone() {
    let dir = repo();
    let mut panel = panel(dir.path());
    key(&mut panel, ']');
    key(&mut panel, ']');
    assert_eq!(panel.cursor, 8);
    assert_eq!(panel.row_at(8), Some(Row::Hunk(0, 1)));

    key(&mut panel, ' ');

    let staged = staged(dir.path());
    assert!(staged.contains("+line nineteen") && !staged.contains("+line two"));
    reload(&mut panel);
    // The unstaged side lost its second hunk; the staged side shows it.
    assert_eq!(panel.diffs.len(), 2);
    assert_eq!(panel.diffs[0].hunks.len(), 1);
    assert!(panel.diffs[1].staged);

    // On the staged side the same key takes it back out.
    let staged_hunk = panel.file_row(1) + 1;
    panel.set_cursor(staged_hunk);
    key(&mut panel, ' ');
    assert_eq!(self::staged(dir.path()), "");
}

#[test]
fn a_selection_keeps_to_its_hunk_and_stages_its_lines() {
    let dir = repo();
    let mut panel = panel(dir.path());
    down(&mut panel, 3);
    assert_eq!(panel.row_at(3), Some(Row::Line(0, 0, 1)));

    key(&mut panel, 'v');
    down(&mut panel, 20);
    assert_eq!(panel.cursor, 7, "the selection ends with its hunk");
    assert_eq!(panel.selection(), Some((3, 7)));

    press(&mut panel, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(panel.selection(), None);
    panel.set_cursor(3);
    press(&mut panel, KeyCode::Down, KeyModifiers::SHIFT);
    assert_eq!(panel.selection(), Some((3, 4)));
    key(&mut panel, ' ');

    let staged = staged(dir.path());
    assert!(staged.contains("-line 2\n+line two"), "{staged}");
    assert!(!staged.contains("nineteen"), "{staged}");
}

#[test]
fn context_alone_stages_nothing_and_says_so() {
    let dir = repo();
    let mut panel = panel(dir.path());
    panel.set_cursor(2);
    key(&mut panel, ' ');
    assert_eq!(staged(dir.path()), "");
    assert_eq!(
        panel.status_message.as_deref(),
        Some(termide_i18n::t().git_diff_nothing_chosen())
    );
}

#[test]
fn a_discard_asks_first_and_comes_back_on_undo() {
    let dir = repo();
    let file = dir.path().join("f");
    let before = std::fs::read_to_string(&file).unwrap();
    let mut panel = panel(dir.path());
    panel.set_cursor(1);

    let events = key(&mut panel, 'd');
    let [PanelEvent::ShowConfirm {
        on_confirm: ConfirmAction::Custom(action),
        ..
    }] = events.as_slice()
    else {
        panic!("no confirmation: {events:?}");
    };
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        before,
        "asked first"
    );

    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(after.contains("line 2\n") && after.contains("line nineteen"));

    key(&mut panel, 'u');
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    key(&mut panel, 'u');
    assert_eq!(
        panel.status_message.as_deref(),
        Some(termide_i18n::t().git_diff_nothing_to_undo())
    );
}

#[test]
fn an_untracked_file_goes_whole_and_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let file = dir.path().join("new.txt");
    std::fs::write(&file, "fresh\n").unwrap();
    let mut panel = panel(dir.path());
    assert_eq!(panel.diffs[0].status, FileStatus::Untracked);

    let events = key(&mut panel, 'd');
    let [PanelEvent::ShowConfirm { message, .. }] = events.as_slice() else {
        panic!("no confirmation: {events:?}");
    };
    assert!(message.contains("new.txt"), "{message}");
    panel.handle_command(PanelCommand::Confirmed {
        action: DISCARD_ACTION.into(),
    });
    assert!(!file.exists());

    key(&mut panel, 'u');
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "fresh\n");

    key(&mut panel, ' ');
    assert!(git(dir.path(), &["diff", "--cached", "--name-only"]).contains("new.txt"));
}

#[test]
fn a_diff_that_cannot_be_staged_keeps_space_for_folding() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = GitDiffPanel::from_unified(
        dir.path().to_path_buf(),
        "diff".into(),
        "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+y\n",
    );
    let mut config = Config::default();
    config.normalize();
    panel.hotkeys = crate::build_git_diff_hotkey_table(&config);

    key(&mut panel, ' ');
    assert!(panel.collapsed.contains(&0));
    assert!(key(&mut panel, 'd').is_empty());
}

#[test]
fn the_cursor_keeps_its_row_number_across_a_reload() {
    let dir = repo();
    let mut panel = panel(dir.path());
    panel.set_cursor(12);
    panel.refresh();
    reload(&mut panel);
    assert_eq!(panel.cursor, 12);

    // Fewer rows: the cursor stays within them.
    std::fs::write(dir.path().join("f"), "short\n").unwrap();
    panel.refresh();
    reload(&mut panel);
    assert!(panel.cursor < panel.total_lines);
}
