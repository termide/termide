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

fn lines() -> Vec<String> {
    (1..=20).map(|n| format!("line {n}")).collect()
}

/// `f` with line `n` (1-based) of each pair replaced.
fn write_f(repo: &Path, changes: &[(usize, &str)]) {
    let mut lines = lines();
    for (n, text) in changes {
        lines[n - 1] = (*text).to_string();
    }
    std::fs::write(repo.join("f"), lines.join("\n") + "\n").unwrap();
}

fn read(repo: &Path, name: &str) -> String {
    std::fs::read_to_string(repo.join(name)).unwrap()
}

/// A repository with `f` of twenty lines committed.
fn repo() -> (tempfile::TempDir, PathBuf, Review) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    git(&repo, &["init", "-q", "-b", "main"]);
    write_f(&repo, &[]);
    git(&repo, &["add", "f"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let review = Review::open(&repo, "s1").unwrap();
    (dir, repo, review)
}

fn patch(review: &Review) -> String {
    review.diff().unwrap().map(|d| d.patch).unwrap_or_default()
}

fn hunk(review: &Review, path: &str, index: usize) -> PatchPart {
    let header = patch(review)
        .split("diff --git ")
        .find(|file| file.starts_with(&format!("a/{path} ")))
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("@@"))
        .nth(index)
        .unwrap()
        .to_string();
    PatchPart {
        hunk: index,
        header,
        lines: None,
    }
}

#[test]
fn the_review_shows_the_agents_changes_and_not_the_users() {
    let (_dir, repo, review) = repo();
    // Uncommitted work of the user's before the agent starts.
    write_f(&repo, &[(2, "user two")]);
    review.begin_request().unwrap();
    write_f(&repo, &[(2, "user two"), (19, "agent nineteen")]);
    std::fs::write(repo.join("g"), "made by the agent\n").unwrap();
    review.end_request().unwrap();

    let shown = patch(&review);
    assert!(shown.contains("+agent nineteen") && shown.contains("+made by the agent"));
    assert!(!shown.contains("user two"), "{shown}");

    // The user's edit between requests goes into the base.
    std::fs::write(repo.join("h"), "the user's\n").unwrap();
    review.begin_request().unwrap();
    assert!(!patch(&review).contains("the user's"));
    assert!(review.diff().unwrap().unwrap().both.is_empty());
}

#[test]
fn a_file_both_edited_is_marked_and_left_by_reject_all() {
    let (_dir, repo, review) = repo();
    review.begin_request().unwrap();
    write_f(&repo, &[(19, "agent nineteen")]);
    std::fs::write(repo.join("g"), "made by the agent\n").unwrap();
    review.end_request().unwrap();
    write_f(&repo, &[(5, "user five"), (19, "agent nineteen")]);
    review.begin_request().unwrap();
    review.end_request().unwrap();

    let diff = review.diff().unwrap().unwrap();
    assert_eq!(diff.both, vec![PathBuf::from("f")]);
    assert_eq!(review.reject_all().unwrap(), vec![PathBuf::from("f")]);
    assert!(!repo.join("g").exists());
    assert!(read(&repo, "f").contains("user five"));
    assert!(review.begin_request().unwrap()[0].contains("\ng"));
}

#[test]
fn a_kept_hunk_leaves_the_review_and_stays_in_the_file() {
    let (_dir, repo, review) = repo();
    review.begin_request().unwrap();
    write_f(&repo, &[(2, "agent two"), (19, "agent nineteen")]);
    review.end_request().unwrap();

    review
        .keep(Path::new("f"), Some(&hunk(&review, "f", 0)))
        .unwrap();
    let shown = patch(&review);
    assert!(!shown.contains("agent two") && shown.contains("+agent nineteen"));
    assert!(read(&repo, "f").contains("agent two"));

    review.keep_all().unwrap();
    assert_eq!(review.diff().unwrap(), None);
    // The next request ending on what was kept leaves nothing behind.
    review.begin_request().unwrap();
    review.end_request().unwrap();
    assert!(!review.is_open());
    assert_eq!(git(&repo, &["for-each-ref", "refs/termide"]), "");
}

#[test]
fn a_rejected_hunk_goes_back_and_the_agent_hears_of_it_once() {
    let (_dir, repo, review) = repo();
    review.begin_request().unwrap();
    write_f(&repo, &[(2, "agent two"), (19, "agent nineteen")]);
    review.end_request().unwrap();

    review
        .reject(Path::new("f"), Some(&hunk(&review, "f", 1)))
        .unwrap();
    assert!(!read(&repo, "f").contains("agent nineteen"));
    assert!(read(&repo, "f").contains("agent two"));
    let notes = review.begin_request().unwrap();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].starts_with("f @@") && notes[0].contains("\n+agent nineteen"),
        "{notes:?}"
    );
    // The revert is the review's, not an edit of the user's: what is left
    // still waits for review.
    assert!(patch(&review).contains("+agent two"));
    review.end_request().unwrap();
    assert!(review.begin_request().unwrap().is_empty());
}

#[test]
fn an_undone_rejection_waits_for_review_again_untold() {
    let (_dir, repo, review) = repo();
    review.begin_request().unwrap();
    std::fs::write(repo.join("g"), "made by the agent\n").unwrap();
    review.end_request().unwrap();

    let rejected = review.reject(Path::new("g"), None).unwrap();
    assert!(!repo.join("g").exists());
    review.undo_reject(&rejected).unwrap();
    assert_eq!(read(&repo, "g"), "made by the agent\n");
    // Not taken for the user's own file at the next request.
    assert!(review.begin_request().unwrap().is_empty());
    assert!(patch(&review).contains("+made by the agent"));
}

#[test]
fn staging_keeps_a_change_but_one_back_to_the_index_is_not_taken_for_it() {
    let (_dir, repo, review) = repo();
    // The user's unstaged edit of line 2 is in the base.
    write_f(&repo, &[(2, "user two")]);
    review.begin_request().unwrap();
    // The agent puts line 2 back as the index has it, and changes 10 and 19.
    write_f(&repo, &[(10, "agent ten"), (19, "agent nineteen")]);
    review.end_request().unwrap();
    assert!(patch(&review).contains("-user two"));

    // The user stages one hunk: it is kept; the others still wait.
    let header = git(&repo, &["diff", "--no-color"])
        .lines()
        .filter(|l| l.starts_with("@@"))
        .find(|l| l.starts_with("@@ -16"))
        .unwrap()
        .to_string();
    crate::apply_part(
        &repo,
        Path::new("f"),
        crate::PatchAction::Stage,
        &PatchPart {
            hunk: 1,
            header,
            lines: None,
        },
    )
    .unwrap();
    let shown = patch(&review);
    assert!(!shown.contains("agent nineteen"), "{shown}");
    assert!(shown.contains("-user two") && shown.contains("+agent ten"));

    // Committing the whole file keeps all of it.
    git(&repo, &["commit", "-q", "-am", "two"]);
    assert_eq!(review.diff().unwrap(), None);
}

#[test]
fn the_refs_hold_trees_that_no_log_shows() {
    let (_dir, repo, review) = repo();
    review.begin_request().unwrap();
    write_f(&repo, &[(19, "agent nineteen")]);
    review.end_request().unwrap();
    let refs = git(
        &repo,
        &["for-each-ref", "--format=%(objecttype) %(refname)"],
    );
    assert!(refs.contains("tree refs/termide/review/s1/base"), "{refs}");
    assert_eq!(git(&repo, &["log", "--all", "--format=%s"]), "one\n");
    // The user's index is never touched.
    assert_eq!(git(&repo, &["diff", "--cached"]), "");
}
