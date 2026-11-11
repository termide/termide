# AGENTS

Lean engineering rules for coding agents working in this repository.

## Sources of truth

1. Source code in `crates/` and `src/` for actual behavior.
2. Shared contracts in `crates/core/` and config/i18n crates for cross-subsystem interfaces and expectations.
3. `doc/en/` for intended user-visible behavior.
4. `doc/ru/` and `doc/zh/` as translations that should stay aligned with `doc/en/`, but do not override code.
5. Workspace and crate manifests (`Cargo.toml`) for crate boundaries, dependencies, and package metadata.
6. Existing tests as executable expectations; when tests, docs, and code drift apart, align them instead of preserving inconsistency.

Resolution rules:
- If docs and code diverge, code is the effective behavior.
- If a user-visible behavior change is intentional, update `doc/en/` and `doc/ru/` in the same change at minimum.
- If tests contradict current intended behavior, fix the tests in the same change.

## Language policy

- Write commit messages, code comments, doc comments, and developer-facing docs in English.
- Keep new identifiers and user-visible config keys in English unless an established external format requires otherwise.
- User-facing documentation may be localized; engineering artifacts should not mix English and Russian in the same context.

## Commit discipline

- Use conventional commits when the scope is honest: `type(scope): subject`.
- Prefer the existing repo vocabulary: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `chore`, `style`, `revert`.
- Keep subjects short and in English.
- Make each commit one coherent unit: one feature, one fix, or one refactor.
- Agents must not create commits by default. In non-autonomous work, prepare the diff, report validation status, and wait for an explicit user request before committing.
- Auto-commit is allowed only when the task is explicitly operating in a fully autonomous execution mode.
- Include tests and required docs updates in the same commit as the behavior change.
- A user-visible change adds its CHANGELOG entry as a fragment in `changelog.d/<section>/` in the same commit (see `changelog.d/README.md`), never to `CHANGELOG.md` directly; the release folds the fragments in. A later change to the same feature before a release edits its fragment instead of adding one.

## Parallel sessions and worktrees

Several agent sessions often work in this repository at once, in the main checkout and in worktrees.

- The main checkout's tree and index are shared: stage by explicit path, never `git add -A` or `git commit -a`; check `git diff --cached --stat` before committing and `git show --stat HEAD` after, since another session's commit can sweep up your files or yours theirs. Never stash or reset what you did not write.
- Create a worktree from local `main` (`git worktree add -b feat/<name> <path> main`); `origin/main` usually lags it.
- Integrate a branch with `git merge --no-ff` (`Merge branch 'feat/<name>'`). When the main checkout is busy, build the merge in a worktree detached at `main`, validate it there, then `git merge --ff-only` in the main checkout. After `git worktree remove`, run `git worktree prune`.
- Do not share `CARGO_TARGET_DIR` between checkouts for anything that gates a commit: workspace crates get the same artifact ids in each, so a build in one overwrites the other's. A hook failure naming code that plainly exists is that; re-run before debugging.
- When builds or the hook turn slow, check `du -sh target` first: cargo never collects old artifacts, and `cargo clean` costs one rebuild. The hook warns past 40 GiB.
- `.git/index.lock` left for a moment is another session's or the git panel's; wait for it, do not delete it.

## Working rules

- Prefer small local changes over broad rewrites.
- Preserve existing crate boundaries and event flow unless the task explicitly requires changing them.
- Reuse existing helpers and conventions before adding new abstractions.
- Do not mix unrelated cleanup into a focused change.
- Do not silently change user-facing behavior as collateral refactoring.

## Architecture constraints

- Prefer typed communication through `PanelEvent`, `PanelCommand`, and shared contracts over ad hoc coupling or downcast-heavy shortcuts.
- Treat redraw behavior, event routing, focus handling, modal flow, and terminal input semantics as correctness-sensitive.
- Keep watcher reactions, cache invalidation, and per-frame work conservative and explicit.
- For UI and input changes, account for narrow terminals, alternate-screen terminal apps, and mouse/keyboard interactions together.

## Preferred patterns

- Add regression tests near the touched logic unless integration coverage is clearly the better fit.
- Start validation narrowly while working, then broaden only as needed:
  - `cargo test -p <crate>` (or `cargo nextest run -p <crate>`)
  - `cargo check -p <crate>`
  - `cargo clippy -p <crate> -- -D warnings`
- Use comments only for invariants, protocol details, and non-obvious constraints.
- Follow existing module and naming patterns unless there is a concrete reason to change them.
- A new UI string goes into all 15 TOMLs under `crates/i18n/i18n/`, the `Translation` trait in `crates/i18n/src/lib.rs` and the key list in `crates/i18n/src/runtime.rs`. A key with a `{placeholder}` goes under `[formats]`, not `[strings]`: there it renders empty.

## Anti-patterns

- No unrelated refactors in bug fixes or focused feature work.
- No new cross-crate coupling without a concrete architectural need.
- No duplicate special-case paths when an existing typed path can be extended.
- No unconditional redraws or avoidable idle-loop work.
- No silent changes to input semantics, focus behavior, modal routing, or panel lifecycle.
- No mixed-language engineering artifacts.

## Validation

- Run the narrowest useful checks first.
- This repository has a local `pre-commit` hook that mirrors CI: `cargo fmt --check`, `cargo machete`, `cargo clippy -- -D warnings` (which covers `cargo check`, so neither the hook nor CI runs one), and `cargo nextest run` (falling back to `cargo test` when nextest is absent). Nextest profiles live in `.config/nextest.toml`; CI uses `--profile ci`, which retries a failure once and reports a test that then passes as flaky.
- The hook checks the staged tree, not the working tree: when the index and the working tree differ it materialises what is being committed in a worktree kept under `target/` (moved to each staged commit, so the build stays incremental). Splitting one edit session into several coherent commits therefore needs no `--no-verify`.
- A commit that stages only `doc/`, `changelog.d/`, top-level Markdown or skill texts skips the build and tests in the hook: nothing compiled reads them.
- Do not rely on the hook as a substitute for targeted validation; use narrow crate-level checks to validate the changed area before committing.
- Live-check UI and agent changes before committing with `tools/tui-harness/run.sh` (see its README): it runs the built binary headlessly against scenario scripts, with a throwaway HOME and the mock model or ACP agents. Add or extend a scenario for the behaviour you changed and run it with the existing ones; do not write throwaway pty scripts instead.
- Treat completed implementation or an internally completed plan as insufficient reason to commit. If human review or local user validation is still expected, stop before commit and report the current state instead.
- Before finishing broader changes, run:
  - `cargo fmt --all`
  - `cargo nextest run --workspace` (or `cargo test --workspace`)
  - `cargo machete` — an unused dependency declaration produces no warning from cargo, so nothing else catches one
- Optional local tooling, all installable and all gated in CI:
  - `cargo install cargo-machete` — unused dependency declarations
  - `cargo install cargo-nextest` — parallel test runner, roughly 5x faster here
  - `cargo install cargo-deny` — reproduces the supply-chain job before a push

## Documentation policy

- Update docs only when behavior, workflow, or invariants actually changed.
- Treat `doc/en/` as the primary user documentation source and keep `doc/ru/` aligned at minimum for user-visible changes.
- Do not pad docs with obvious implementation trivia.
