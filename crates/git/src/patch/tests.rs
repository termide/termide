use super::*;
use std::path::PathBuf;
use std::process::Command;

const HEADER: &str = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n";

fn cut(diff: &str, hunk: usize, lines: Option<(usize, usize)>, reverse: bool) -> Option<String> {
    let patch = RawPatch::parse(diff.as_bytes());
    patch
        .cut(hunk, lines, reverse)
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

#[test]
fn a_whole_hunk_is_cut_alone_with_its_file_header() {
    let diff = format!("{HEADER}@@ -1,2 +1,2 @@ fn a\n-a\n+A\n b\n@@ -9,1 +9,1 @@\n-x\n+X\n");
    assert_eq!(
        cut(&diff, 1, None, false).unwrap(),
        format!("{HEADER}@@ -9,1 +9,1 @@\n-x\n+X\n")
    );
    assert_eq!(
        cut(&diff, 0, None, false).unwrap(),
        format!("{HEADER}@@ -1,2 +1,2 @@ fn a\n-a\n+A\n b\n")
    );
}

/// Forwards, a removal left out stays as context and an addition left out
/// goes; backwards, the other way round.
#[test]
fn lines_left_out_stay_as_the_target_side_has_them() {
    let diff = format!("{HEADER}@@ -1,3 +1,3 @@\n c\n-a\n-b\n+A\n+B\n");
    assert_eq!(
        cut(&diff, 0, Some((1, 1)), false).unwrap(),
        format!("{HEADER}@@ -1,3 +1,2 @@\n c\n-a\n b\n")
    );
    assert_eq!(
        cut(&diff, 0, Some((3, 3)), false).unwrap(),
        format!("{HEADER}@@ -1,3 +1,4 @@\n c\n a\n b\n+A\n")
    );
    assert_eq!(
        cut(&diff, 0, Some((3, 3)), true).unwrap(),
        format!("{HEADER}@@ -1,2 +1,3 @@\n c\n+A\n B\n")
    );
}

#[test]
fn context_alone_is_nothing_to_apply() {
    let diff = format!("{HEADER}@@ -1,3 +1,3 @@\n c\n-a\n+A\n d\n");
    assert_eq!(cut(&diff, 0, Some((0, 0)), false), None);
    assert_eq!(cut(&diff, 0, Some((3, 3)), true), None);
    assert_eq!(cut(&diff, 5, None, false), None);
}

#[test]
fn carriage_returns_and_missing_newlines_survive_the_cut() {
    let diff = format!(
        "{HEADER}@@ -1,2 +1,2 @@\n a\r\n-b\r\n\\ No newline at end of file\n+B\r\n\\ No newline at end of file\n"
    );
    assert_eq!(cut(&diff, 0, None, false).unwrap(), diff);
    // Leaving the addition out drops its marker with it.
    assert_eq!(
        cut(&diff, 0, Some((1, 1)), false).unwrap(),
        format!("{HEADER}@@ -1,2 +1,1 @@\n a\r\n-b\r\n\\ No newline at end of file\n")
    );
}

// --- On a repository -------------------------------------------------------

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

/// A repository whose `f` has twenty lines committed, and lines 2 and 19
/// changed in the working tree: two hunks.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    git(&repo, &["init", "-q", "-b", "main"]);
    let lines: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
    std::fs::write(repo.join("f"), lines.join("\n") + "\n").unwrap();
    git(&repo, &["add", "f"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let mut changed = lines.clone();
    changed[1] = "line two".into();
    changed[18] = "line nineteen".into();
    std::fs::write(repo.join("f"), changed.join("\n") + "\n").unwrap();
    (dir, repo)
}

fn part(repo: &Path, staged: bool, hunk: usize, lines: Option<(usize, usize)>) -> PatchPart {
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    let diff = git(repo, &args);
    let header = diff
        .lines()
        .filter(|l| l.starts_with("@@"))
        .nth(hunk)
        .unwrap();
    PatchPart {
        hunk,
        header: header.to_string(),
        lines,
    }
}

#[test]
fn staging_one_hunk_leaves_the_other_in_the_working_tree() {
    let (_dir, repo) = repo();
    let f = Path::new("f");
    apply_part(&repo, f, PatchAction::Stage, &part(&repo, false, 1, None)).unwrap();
    let staged = git(&repo, &["diff", "--cached"]);
    assert!(staged.contains("+line nineteen") && !staged.contains("+line two"));
    let unstaged = git(&repo, &["diff"]);
    assert!(unstaged.contains("+line two") && !unstaged.contains("+line nineteen"));

    // And back out of the index.
    apply_part(&repo, f, PatchAction::Unstage, &part(&repo, true, 0, None)).unwrap();
    assert_eq!(git(&repo, &["diff", "--cached"]), "");
}

#[test]
fn staging_lines_takes_only_them() {
    let (_dir, repo) = repo();
    // Hunk 0 is ` line 1`, `-line 2`, `+line two`, ` line 3`...: only the
    // removal goes in, so the index loses line 2 without gaining its rewrite.
    apply_part(
        &repo,
        Path::new("f"),
        PatchAction::Stage,
        &part(&repo, false, 0, Some((1, 1))),
    )
    .unwrap();
    let staged = git(&repo, &["diff", "--cached"]);
    assert!(
        staged.contains("-line 2") && !staged.contains("+line two"),
        "{staged}"
    );
}

#[test]
fn a_discarded_hunk_comes_back_from_its_kept_copy() {
    let (_dir, repo) = repo();
    let f = Path::new("f");
    let before = std::fs::read(repo.join(f)).unwrap();
    let kept = save_blob(&repo, f).unwrap();
    apply_part(&repo, f, PatchAction::Discard, &part(&repo, false, 0, None)).unwrap();
    let now = std::fs::read_to_string(repo.join(f)).unwrap();
    assert!(now.contains("line 2\n") && now.contains("line nineteen"));
    restore_blob(&repo, f, &kept).unwrap();
    assert_eq!(std::fs::read(repo.join(f)).unwrap(), before);
}

#[test]
fn a_hunk_that_changed_meanwhile_is_refused() {
    let (_dir, repo) = repo();
    let seen = part(&repo, false, 0, None);
    std::fs::write(repo.join("f"), "all new\n").unwrap();
    assert_eq!(
        apply_part(&repo, Path::new("f"), PatchAction::Stage, &seen),
        Err(PatchError::Stale)
    );
}

#[test]
fn a_last_line_without_newline_stages_and_unstages() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("n"), "a\nb").unwrap();
    git(repo, &["add", "n"]);
    git(repo, &["commit", "-q", "-m", "one"]);
    std::fs::write(repo.join("n"), "a\nB").unwrap();
    let n = Path::new("n");
    apply_part(repo, n, PatchAction::Stage, &part(repo, false, 0, None)).unwrap();
    assert_eq!(git(repo, &["show", ":n"]), "a\nB");
    apply_part(repo, n, PatchAction::Unstage, &part(repo, true, 0, None)).unwrap();
    assert_eq!(git(repo, &["show", ":n"]), "a\nb");
}

#[test]
fn crlf_lines_stage_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "core.autocrlf", "false"]);
    std::fs::write(repo.join("w"), "a\r\nb\r\nc\r\n").unwrap();
    git(repo, &["add", "w"]);
    git(repo, &["commit", "-q", "-m", "one"]);
    std::fs::write(repo.join("w"), "a\r\nB\r\nc\r\n").unwrap();
    apply_part(
        repo,
        Path::new("w"),
        PatchAction::Stage,
        &part(repo, false, 0, None),
    )
    .unwrap();
    assert_eq!(git(repo, &["show", ":w"]), "a\r\nB\r\nc\r\n");
}
