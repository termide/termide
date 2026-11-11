---
name: integrate-worktree
description: Merge work done in a git worktree of termide back into main safely — rename the branch, build and validate the merge apart from a main checkout other sessions may be using, fast-forward main, and clean up the worktree. Use when a feature branch or worktree is finished and should land on main, or when several worktree branches are to be integrated together.
---

# Integrate a worktree

The main checkout is shared with other sessions and with the user's editor:
its working tree may hold someone's uncommitted work, and a build there can
clobber yours. So the merge is built and validated in a worktree of its own
and reaches `main` only as a fast-forward. The rules behind each step are in
AGENTS.md, "Parallel sessions and worktrees".

## 1. Make the branch ready

In the feature worktree (`git -C <wt> …` from outside it):

- `git -C <wt> status --short` — nothing left uncommitted that belongs to
  the work.
- Name the branch as the history does: `git -C <wt> branch -m worktree-<name> feat/<name>`
  (or `fix/`, `refactor/`).
- Its commits passed the pre-commit hook in that worktree, built with the
  worktree's own `target/` — not a `CARGO_TARGET_DIR` shared with the main
  checkout, which another build can overwrite.

## 2. Build the merge apart

```
git worktree add --detach target/integrate main
git -C target/integrate merge --no-ff feat/<name> -m "Merge branch 'feat/<name>'"
```

For several branches, merge each in turn, `--no-ff` each, in the order
they depend on one another. On a conflict, resolve it in
`target/integrate`, keeping both sides' intent; a `changelog.d/` fragment
never conflicts, a hand-edited `CHANGELOG.md` would.

## 3. Validate the merge

```
cd target/integrate
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo machete
```

Run `tools/tui-harness/run.sh` there too when the branches touch the UI or
the agent. A failure here that neither branch had alone is the merge's: fix
it on the branch it belongs to, or as a commit on top of the merge.

## 4. Land it

`main` may have moved meanwhile; then go back to step 2 from the new
`main`. Otherwise, in the main checkout:

```
git merge --ff-only "$(git -C target/integrate rev-parse HEAD)"
```

`--ff-only` touches only the files the merge changed. If the main checkout
holds uncommitted changes to those files, stop and ask: never stash,
reset or commit what you did not write.

## 5. Clean up

```
git worktree remove target/integrate
git worktree remove <wt>
git worktree prune
git branch -d feat/<name>
```

`prune` matters: the hook keeps its own worktree under `<wt>/target/`, which
stays registered after `<wt>` is removed. Then `git log --oneline -5` and
`git show --stat HEAD` to check what landed.
