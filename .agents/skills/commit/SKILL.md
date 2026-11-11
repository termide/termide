---
name: commit
description: Collect uncommitted changes and create a semantic commit with a descriptive message
user-invocable: true
allowed-tools: Bash(git status), Bash(git diff*), Bash(git log*), Bash(git add *), Bash(git commit *)
---

# Semantic Commit

Collect all uncommitted changes and create a single structured commit.

Platform note: agents that support frontmatter permissions may use `allowed-tools` and `user-invocable`. Other agents should ignore unsupported fields and follow the workflow below.

## Process

1. Run `git status` (without `-uall`) and `git diff` (staged + unstaged) in parallel to understand changes
2. Run `git log --oneline -5` for recent commit context
3. Analyze changes: which files are affected, what was added/removed, purpose of the changes
4. Stage files by name (do not use `git add -A`; never add `.env`, credentials, binary files)
5. Compose a commit message in conventional commits format:
   - `feat(scope): description` — new functionality
   - `fix(scope): description` — bug fix
   - `docs(scope): description` — documentation
   - `refactor(scope): description` — refactoring without behavior change
   - `perf(scope): description` — performance improvement
   - `test(scope): description` — tests
   - `chore(scope): description` — maintenance
   - Scope = affected crate or area (app, core, agent, config, i18n)
   - If changes span multiple scopes — use the primary one or omit
   - Commit body (after blank line) explains **why**, not what
   - Commit messages always in English
6. Create the commit via HEREDOC. Never use `--no-verify`.
7. If a pre-commit hook fails — fix the issue, re-stage and create a **new** commit (never `--amend`)
8. Show `git status` after the commit to confirm

## Rules

- Never commit files containing secrets
- Never use `--no-verify` or `--no-gpg-sign`
- Never amend without explicit request
- If there are no changes — report and stop
- Summary line up to 72 characters
- Communicate in the user's language
