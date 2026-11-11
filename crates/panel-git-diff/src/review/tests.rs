use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_config::Config;
use termide_core::{KeyChord, Panel, PanelCommand};

use super::*;
use crate::stage::Row;

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

/// `f` of twenty lines, with line `n` (1-based) of each pair replaced.
fn write_f(repo: &Path, changes: &[(usize, &str)]) {
    let mut lines: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
    for (n, text) in changes {
        lines[n - 1] = (*text).to_string();
    }
    std::fs::write(repo.join("f"), lines.join("\n") + "\n").unwrap();
}

fn read_f(repo: &Path) -> String {
    std::fs::read_to_string(repo.join("f")).unwrap()
}

/// A repository where the agent of session `s1`, in one request, changed
/// lines 2 and 19 of `f` and created `g`. The review draws `f` as:
///
/// ```text
/// 0  file f
/// 1  @@ hunk 0   (line 2)
/// 8  @@ hunk 1   (line 19)
/// ```
fn reviewed() -> (tempfile::TempDir, git::Review) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    write_f(repo, &[]);
    git(repo, &["add", "f"]);
    git(repo, &["commit", "-q", "-m", "one"]);
    let review = git::Review::open(repo, "s1").unwrap();
    review.begin_request().unwrap();
    write_f(repo, &[(2, "agent two"), (19, "agent nineteen")]);
    std::fs::write(repo.join("g"), "made by the agent\n").unwrap();
    review.end_request().unwrap();
    (dir, review)
}

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
    let mut panel = GitDiffPanel::new_for_review(repo.to_path_buf(), "s1".into());
    let mut config = Config::default();
    config.normalize();
    panel.hotkeys = crate::build_git_diff_hotkey_table(&config);
    panel.visible_height = 40;
    reload(&mut panel);
    panel
}

fn key(panel: &mut GitDiffPanel, c: char) -> Vec<PanelEvent> {
    panel.handle_key(KeyChord::identity(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

fn confirm(panel: &mut GitDiffPanel, action: &str) {
    panel.handle_command(PanelCommand::Confirmed {
        action: action.to_string(),
    });
}

fn paths(panel: &GitDiffPanel) -> Vec<&str> {
    panel.diffs.iter().map(|d| d.path.as_str()).collect()
}

#[test]
fn the_review_shows_the_agents_changes_and_space_keeps_one() {
    let (dir, _review) = reviewed();
    let mut panel = panel(dir.path());
    assert_eq!(paths(&panel), ["f", "g"]);
    assert!(panel.title().contains("files"), "{}", panel.title());
    assert!(!panel.can_stage());

    key(&mut panel, ']');
    assert_eq!(panel.row_at(panel.cursor), Some(Row::Hunk(0, 0)));
    key(&mut panel, ' ');
    reload(&mut panel);

    assert_eq!(panel.diffs[0].hunks.len(), 1);
    assert!(panel.diffs[0].hunks[0]
        .lines
        .iter()
        .any(|l| l.content == "agent nineteen"));
    // Kept, not staged: the file and the index are as they were.
    assert!(read_f(dir.path()).contains("agent two"));
    assert_eq!(git(dir.path(), &["diff", "--cached"]), "");
}

#[test]
fn d_rejects_a_hunk_and_u_takes_the_rejection_back() {
    let (dir, review) = reviewed();
    let mut panel = panel(dir.path());
    key(&mut panel, ']');
    key(&mut panel, ']');
    assert_eq!(panel.row_at(panel.cursor), Some(Row::Hunk(0, 1)));

    let events = key(&mut panel, 'd');
    assert!(!events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowConfirm { .. })));
    assert!(!read_f(dir.path()).contains("agent nineteen"));
    reload(&mut panel);
    assert_eq!(panel.diffs[0].hunks.len(), 1);

    key(&mut panel, 'u');
    assert!(read_f(dir.path()).contains("agent nineteen"));
    // Taken back, the agent is not told of it.
    assert!(review.begin_request().unwrap().is_empty());
}

#[test]
fn a_file_the_agent_created_goes_whole() {
    let (dir, _review) = reviewed();
    let mut panel = panel(dir.path());
    let g = panel.file_row(1);
    panel.set_cursor(g + 1);
    key(&mut panel, 'v');
    assert_eq!(panel.selection(), None, "no lines of a new file to choose");
    key(&mut panel, 'd');
    assert!(!dir.path().join("g").exists());
}

#[test]
fn a_file_edited_by_both_is_marked_and_asks_before_a_rejection() {
    let (dir, review) = reviewed();
    write_f(
        dir.path(),
        &[(2, "agent two"), (10, "user ten"), (19, "agent nineteen")],
    );
    review.begin_request().unwrap();
    review.end_request().unwrap();
    let mut panel = panel(dir.path());
    assert!(panel.edited_by_both("f") && !panel.edited_by_both("g"));

    key(&mut panel, ']');
    let events = key(&mut panel, 'd');
    assert!(matches!(
        events.as_slice(),
        [PanelEvent::ShowConfirm { message, .. }] if message.contains('f')
    ));
    assert!(read_f(dir.path()).contains("agent two"));
    confirm(&mut panel, REJECT_ACTION);
    assert!(!read_f(dir.path()).contains("agent two"));
}

#[test]
fn reject_all_asks_and_leaves_files_edited_by_both() {
    let (dir, review) = reviewed();
    write_f(
        dir.path(),
        &[(2, "agent two"), (10, "user ten"), (19, "agent nineteen")],
    );
    review.begin_request().unwrap();
    review.end_request().unwrap();
    let mut panel = panel(dir.path());

    let events = key(&mut panel, 'x');
    assert!(matches!(
        events.as_slice(),
        [PanelEvent::ShowConfirm { .. }]
    ));
    confirm(&mut panel, REJECT_ALL_ACTION);
    assert!(!dir.path().join("g").exists());
    assert!(read_f(dir.path()).contains("user ten"));
    assert!(panel
        .status_message
        .as_deref()
        .is_some_and(|m| m.contains('1')));

    key(&mut panel, 'a');
    reload(&mut panel);
    assert!(panel.diffs.is_empty());
}
