use super::*;
use std::process::Command;

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

/// Ten lines, `line 1` to `line 10`, with line `n` (1-based) of each pair
/// replaced; a `None` text removes the line.
fn text(changes: &[(usize, Option<&str>)]) -> String {
    let mut out = String::new();
    for n in 1..=10 {
        match changes.iter().find(|(at, _)| *at == n) {
            Some((_, Some(line))) => out.push_str(&format!("{line}\n")),
            Some((_, None)) => {}
            None => out.push_str(&format!("line {n}\n")),
        }
    }
    out
}

/// A repository with `f` committed as [`text`] has it unchanged.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("f"), text(&[])).unwrap();
    git(&repo, &["add", "f"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    (dir, repo)
}

fn index(repo: &Path) -> String {
    git(repo, &["show", ":f"])
}

#[test]
fn a_change_is_staged_alone_from_unsaved_text() {
    let (_dir, repo) = repo();
    let edited = text(&[(2, Some("two")), (8, Some("eight"))]);
    // The file on disk is untouched: the editor holds the edit unsaved.
    assert!(stage_lines(&repo.join("f"), &edited, 1..=1).unwrap());
    assert_eq!(index(&repo), text(&[(2, Some("two"))]));
    assert_eq!(std::fs::read_to_string(repo.join("f")).unwrap(), text(&[]));
    // Nothing is left to stage on line 2; line 5 shows no change.
    assert!(!stage_lines(&repo.join("f"), &edited, 1..=1).unwrap());
    assert!(!stage_lines(&repo.join("f"), &edited, 4..=4).unwrap());
}

#[test]
fn a_removal_belongs_to_the_line_above_it() {
    let (_dir, repo) = repo();
    let edited = text(&[(4, None), (5, None)]);
    assert!(!stage_lines(&repo.join("f"), &edited, 3..=3).unwrap());
    assert!(stage_lines(&repo.join("f"), &edited, 2..=2).unwrap());
    assert_eq!(index(&repo), edited);
}

#[test]
fn unstaging_takes_back_only_the_change_on_the_lines() {
    let (_dir, repo) = repo();
    let edited = text(&[(2, Some("two")), (8, Some("eight"))]);
    let f = repo.join("f");
    stage_lines(&f, &edited, 0..=9).unwrap();
    // Line 9 typed after staging moves nothing: the staged line 8 is still
    // line 8 of the text.
    let typed = text(&[(2, Some("two")), (8, Some("eight")), (9, Some("nine"))]);
    assert!(unstage_lines(&f, &typed, 7..=7).unwrap());
    assert_eq!(index(&repo), text(&[(2, Some("two"))]));
    assert!(!unstage_lines(&f, &typed, 7..=7).unwrap());
}

#[test]
fn a_new_file_unstaged_whole_leaves_the_index() {
    let (_dir, repo) = repo();
    let new = repo.join("g");
    std::fs::write(&new, "a\nb\n").unwrap();
    assert!(stage_lines(&new, "a\nb\n", 0..=0).unwrap());
    assert_eq!(git(&repo, &["show", ":g"]), "a\nb\n");
    assert!(unstage_lines(&new, "a\nb\n", 0..=1).unwrap());
    assert_eq!(git(&repo, &["ls-files", "g"]), "");
}

#[test]
fn crlf_lines_go_back_into_the_index_as_they_were() {
    let (_dir, repo) = repo();
    std::fs::write(repo.join("w"), "a\r\nb\r\n").unwrap();
    git(&repo, &["add", "w"]);
    // The editor holds `\n` alone.
    assert!(stage_lines(&repo.join("w"), "a\nB\n", 1..=1).unwrap());
    assert_eq!(git(&repo, &["show", ":w"]), "a\r\nB\r\n");
}

#[test]
fn a_conflicted_file_is_refused() {
    let (_dir, repo) = repo();
    git(&repo, &["checkout", "-q", "-b", "other"]);
    std::fs::write(repo.join("f"), text(&[(1, Some("theirs"))])).unwrap();
    git(&repo, &["commit", "-qam", "theirs"]);
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("f"), text(&[(1, Some("ours"))])).unwrap();
    git(&repo, &["commit", "-qam", "ours"]);
    let merge = Command::new("git")
        .args(["merge", "-q", "other"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let error = stage_lines(&repo.join("f"), "x\n", 0..=0).unwrap_err();
    assert!(error.contains("merge conflict"), "{error}");
}

#[test]
fn a_revert_replaces_only_the_changed_lines_on_the_cursor() {
    let index = text(&[]);
    let edited = text(&[(2, Some("two")), (3, Some("three")), (8, Some("eight"))]);
    assert_eq!(
        revert_lines(&index, &edited, 2..=2),
        Some(LineRevert {
            lines: 1..3,
            text: "line 2\nline 3\n".into(),
        })
    );
    assert_eq!(revert_lines(&index, &edited, 4..=4), None);
    // A selection over both changes replaces the lines between them too,
    // as they are.
    let both = revert_lines(&index, &edited, 2..=7).unwrap();
    assert_eq!(both.lines, 1..8);
    assert_eq!(
        both.text,
        "line 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n"
    );
    // A removal comes back after the line above it.
    let removed = text(&[(5, None)]);
    assert_eq!(
        revert_lines(&index, &removed, 3..=3),
        Some(LineRevert {
            lines: 4..4,
            text: "line 5\n".into(),
        })
    );
}
