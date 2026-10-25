---
name: release-management
description: Prepare and publish a project release with version updates, changelog generation, tagging, and validation
---

# Release Management Skill

Comprehensive release management workflow for Termide project that ensures code quality, version consistency, and proper changelog documentation before creating releases.

Platform note: agents that support extra frontmatter fields may add their own permission hints around this skill. Agents that do not support them should ignore unsupported fields and follow the workflow below. When interactive prompts or inline editing are unavailable, use the closest equivalent workflow the platform provides.

## When to Use This Skill

Invoke this skill when you need to:
- Create a new release (patch/minor/major version bump)
- Recreate a failed release tag (after fixing CI/build issues)
- Update all version references across the project
- Generate changelog entries from git history and file changes
- Ensure code quality before tagging

## Prerequisites

- Clean git working directory (or explicitly handle uncommitted changes)
- All tests passing locally
- No pending changes that should be in separate commits

## Workflow Overview

This skill follows a comprehensive 12-step workflow:

1. **Code Quality Checks** - Run formatters, linters, tests, and build
2. **Change Analysis** - Analyze uncommitted changes, commits, and file states
3. **Version Detection** - Detect current version from multiple sources
4. **Version Selection** - Ask user for new version (patch/minor/major/custom/recreate)
5. **File Updates** - Update version in 12+ files across the project
6. **Documentation Review** - Prompt user to review and update docs if needed
7. **Changelog Generation** - Auto-generate CHANGELOG.md section from changes
8. **Re-check Quality** - Run quality checks again after file updates
9. **Create Commit** - Create conventional commit for release
10. **Create Tag** - Create annotated git tag
11. **Push** - Ask for confirmation before pushing to remote
12. **Report** - Show release URL and CI/CD status link

## Step-by-Step Implementation

### Step 1: Pre-Release Code Quality Checks

**CRITICAL: These checks must pass before proceeding with release**

Run these commands in sequence. If ANY fail, stop immediately and report errors:

```bash
# 1. Format check
cargo fmt --check

# 2. Clippy strict mode
cargo clippy -- -D warnings

# 3. Run test suite
cargo test

# 4. Release build check
cargo build --release
```

**Error Handling:**
- If `cargo fmt --check` fails: Show diffs and suggest running `cargo fmt`
- If `cargo clippy` fails: Show warnings/errors, suggest fixing before release
- If `cargo test` fails: Show failed tests, abort release
- If `cargo build --release` fails: Show build errors, abort release

**Output to user:**
```
🔍 Pre-Release Quality Checks
✅ Code formatting (cargo fmt --check)
✅ Linter checks (cargo clippy)
✅ Test suite (49 passed, 4 ignored)
✅ Release build

All quality checks passed! Proceeding with release...
```

### Step 2: Analyze Changes for CHANGELOG

This is **editorial** work, not mechanical commit-parsing. The goal is a
human-readable changelog, so the agent must *understand* what happened in
the release, not just translate `git log` line-by-line.

Collect raw material from **three sources**, then synthesize it into
themes (see "Synthesis" below before writing anything to CHANGELOG).

#### Source 1: Uncommitted Changes
```bash
git status --porcelain
git diff --stat
```

Parse output to detect:
- Modified files (M)
- Added files (A)
- Deleted files (D)
- Renamed files (R)

#### Source 2: Committed Changes Since Last Tag
```bash
# Get last tag
last_tag=$(git describe --tags --abbrev=0 2>/dev/null || echo "")

# Get commits since tag (use full subjects + bodies, not just --oneline)
if [ -n "$last_tag" ]; then
    git log ${last_tag}..HEAD --no-decorate --pretty=fuller
else
    git log --no-decorate --pretty=fuller
fi
```

Conventional commit type is a **hint** for categorization, never the final
word. Always cross-check with the actual diff before placing the change
under Added / Changed / Fixed:

- `feat:` → usually Added
- `fix:` → usually Fixed
- `docs:` → Changed only if user-visible behavior or workflow changed
- `refactor:` / `perf:` → Changed only if user-observable; otherwise drop
- `chore:` / `style:` → almost never in changelog
- `BREAKING CHANGE:` (footer) → call out explicitly regardless of type

#### Source 3: File States at Each Commit
For each commit between tag and HEAD, check what actually changed in files:

```bash
# For each commit
git log ${last_tag}..HEAD --format="%H" | while read commit_hash; do
    git show --stat $commit_hash
done
```

This reveals:
- Which features were actually added (new files in src/panels/, etc.)
- Configuration changes (config.rs, constants.rs)
- Documentation updates (README.md, doc/*)

If a commit message says "feat: add X" but the diff only touches tests or
unrelated files, **trust the diff**, not the message.

#### Synthesis — turn raw log into a coherent narrative

Git history contains a lot of noise the user will not want to read.
Before writing the changelog, **collapse the log into themes**. A theme
is a single user-visible change, possibly spanning many commits.

**Drop entirely (do not surface to user as changelog candidates):**
- Pure "WIP" / "checkpoint" / "save" commits with no standalone meaning.
- Typo fixes, formatting fixes, comment edits with no behavior change.
- Lint / clippy / fmt-only commits.
- Commits that were later fully reverted within the same release window.
- Dependency version bumps that don't change observable behavior (unless
  the bump is the release's headline).
- Iterative fixes-on-top-of-a-feature inside the same release — fold
  them into the feature's entry. The reader cares that "feature X
  works", not that it took three commits to land.
- Refactor-then-revert / "broke main, fixed main" pairs.
- Internal renames, file moves, module reorganizations with no API
  effect — unless they're a deliberate breaking change.

**Collapse together (one changelog line per theme, not per commit):**
- A feature plus its follow-up bugfixes from the same release window.
- A multi-step refactor that lands across N commits with one outcome.
- Translation updates for the same change in multiple languages
  (mention once, list affected locales if useful).

**Keep, with rewording:**
- User-visible behavior change → describe the *effect on the user*, not
  the implementation. "Migrated to XDG Base Directory Specification" is
  better than "feat(config): use dirs::config_dir() for state path".
- Bug fix → describe the bug as the user experienced it, not as the
  patch line in the diff. "Fixed crash when opening empty files" beats
  "fix: handle 0-length read in editor::load".
- Breaking change → say what breaks and what the user should do.

**Pull from `git show <hash>` for context** when a commit subject is
cryptic but the diff is meaningful — write the changelog from the diff.

Verify the synthesis pass actually happened: if the resulting bullet
count is roughly equal to the commit count, you almost certainly did not
collapse anything. Re-read the log and merge follow-ups.

**Output structure (raw analysis stage, before drafting CHANGELOG):**
```
Changes Analysis:
==================

Uncommitted Changes:
- Modified: src/main.rs, Cargo.toml
- Added: CHANGELOG.md

Commits Since 0.2.0 (raw):
- 6d2e247 refactor: remove FileManager special handling
- 4e7c694 feat: migrate to XDG Base Directory Specification
- 6509fdb feat: implement session autosave
- a1b2c3d fix: typo in autosave
- d4e5f6a fix(autosave): debounce write
- 7890abc chore: clippy

Themes (after synthesis):
- XDG Base Directory Specification support  [4e7c694]
- Automatic session persistence with debounced writes  [6509fdb + d4e5f6a, typo and clippy folded in]
- FileManager is now a regular closable panel — BREAKING  [6d2e247]

Dropped as noise:
- a1b2c3d (typo, folded into autosave entry)
- 7890abc (clippy-only)
```

These **themes**, not the raw commit list, are what Step 7 turns into
CHANGELOG entries.

### Step 3: Detect Current Version

Check version in multiple files and detect inconsistencies:

```bash
# Cargo.toml (primary source)
cargo_version=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

# flake.nix
flake_version=$(grep 'version = ' flake.nix | head -1 | sed 's/.*version = "\(.*\)";/\1/')

# Last git tag
git_version=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")
```

**If versions match:** Use that version as current.

**If versions differ:** Show table and ask user:

```
⚠️  Version Mismatch Detected:

File                     Version
-------------------------------------
Cargo.toml              0.2.0
flake.nix               0.1.5  ⚠️
Last git tag            0.2.0

Which version is correct as the current version?
1. 0.2.0 (Cargo.toml + git tag)
2. 0.1.5 (flake.nix)
3. Other (specify manually)
```

Use the platform's interactive prompt mechanism for this decision.

### Step 4: Request New Version

Show current version and offer version bump options:

```
Current version: 0.2.0

Select release type:
1. patch (0.2.0 → 0.2.1) - Bug fixes, minor changes
2. minor (0.2.0 → 0.3.0) - New features, backwards compatible
3. major (0.2.0 → 1.0.0) - Breaking changes
4. custom - Enter specific version (e.g., 0.2.5)
5. recreate 0.2.0 - Recreate existing tag (for failed CI/CD)
```

Use the platform's interactive prompt mechanism with options:
- patch
- minor
- major
- custom
- recreate

**For custom**: Prompt for version string, validate format (X.Y.Z).

**For recreate**: Ask confirmation:
```
⚠️  Are you sure you want to recreate tag 0.2.0?
This will:
- Delete local tag 0.2.0
- Delete remote tag 0.2.0 (if pushed)
- Create new tag 0.2.0 with current HEAD

This is typically done when CI/CD failed and you fixed the issues.

Proceed? [yes/no]
```

### Step 5: Update Version in All Files

Update version `NEW_VERSION` in these 10 files (plus `Cargo.lock`). Build the
authoritative list first, so a file added since this skill was written is not
missed:

```bash
git grep -c 'OLD_VERSION_ESCAPED' -- ':!CHANGELOG.md' ':!Cargo.lock'
```

(`OLD_VERSION_ESCAPED` is `OLD_VERSION` with the dots escaped, e.g. `0\.35\.0`.)
Every hit below is the release version itself, so a literal replace of all
occurrences is safe in each file; review the grep output once in case a new
file shows up with an unrelated match.

#### 1. Cargo.toml
```toml
[workspace.package]
version = "NEW_VERSION"
```
Line 9 (under `[workspace.package]`, which every crate inherits), exact match:
`version = "OLD_VERSION"`

#### 2. flake.nix (2 occurrences)
```nix
version = "NEW_VERSION";
```
One in `packages.default` (around line 84) and one in `packages.termide-static`
(around line 117). Both are the exact match `version = "OLD_VERSION";` — use
replace_all=true.

#### 3. README.md, README.ru.md, README.zh.md (23 occurrences each)
All occurrences are release asset file names in download URLs and commands:
```
termide-OLD_VERSION-x86_64-unknown-linux-gnu.tar.gz   (also musl, aarch64, apple-darwin)
termide-OLD_VERSION-x86_64-pc-windows-msvc.zip
termide_OLD_VERSION-1_amd64.deb
termide-OLD_VERSION-1.x86_64.rpm
```
Replace all occurrences of `OLD_VERSION` (note the `.deb` uses `termide_`, so a
`termide-VERSION-` pattern alone would miss it). URLs go through
`releases/latest/download/`, so there is no version in the URL path itself.

#### 4. doc/en/installation.md, doc/ru/installation.md, doc/zh/installation.md (16 occurrences each)
Same asset names as the READMEs; replace all occurrences.

#### 5. packaging/aur/PKGBUILD
```bash
pkgver=NEW_VERSION
```
Line 4, simple replacement. Reset `pkgrel=1` if it was bumped.

#### 6. packaging/aur/PKGBUILD-bin
```bash
pkgver=NEW_VERSION
```
Line 4, simple replacement. Reset `pkgrel=1` if it was bumped.

#### 7. Cargo.lock
The workspace crates' versions in `Cargo.lock` follow `Cargo.toml`, and the AUR
build uses `cargo fetch --locked`, so the lock file must be committed with the
new version. Refresh it after editing `Cargo.toml`:
```bash
cargo update --workspace
```
This touches only the workspace members' entries, not dependency versions.

**Not bumped:** `packaging/crates-io-redirect/` is a standalone tombstone crate
for the `termide` name on crates.io. Its `version` is unrelated to the release
(it must exceed the crates.io maximum and is set only when that crate is
republished by hand) — leave it alone. There is no Homebrew formula in the tree.

**Batch Update Strategy:**
All files above can be updated in one pass (the version is not a substring of
anything else in them):
```bash
git grep -l 'OLD_VERSION_ESCAPED' -- ':!CHANGELOG.md' ':!Cargo.lock' \
  | xargs sed -i '' 's/OLD_VERSION_ESCAPED/NEW_VERSION/g'   # GNU sed: -i without ''
cargo update --workspace
```
Or use the Edit tool with replace_all=true per file.

**Verification:**
After updates, grep for the old version to ensure all replaced — this must
print nothing:
```bash
git grep -n 'OLD_VERSION_ESCAPED' -- ':!CHANGELOG.md'
```
`Cargo.lock` is included on purpose: a leftover there means the workspace
entries were not refreshed (unless a third-party dependency happens to share
the old version number — check the `name` above any hit). Then confirm the new version landed everywhere
with the expected counts (Cargo.toml 1, flake.nix 2, each README 23, each
installation.md 16, each PKGBUILD 1):
```bash
git grep -c 'NEW_VERSION_ESCAPED' -- ':!CHANGELOG.md' ':!Cargo.lock'
```

### Step 6: Documentation Actuality Check

After version updates, prompt user to review documentation:

```
📝 Documentation Review

Version numbers have been updated in:
- README.md
- README.ru.md
- README.zh.md
- doc/en/installation.md
- doc/ru/installation.md
- doc/zh/installation.md

Please review these files for content accuracy:

Required reviews (version-critical):
✅ README.md - Download links updated
✅ README.ru.md - Download links updated
✅ README.zh.md - Download links updated
✅ doc/en/installation.md - Installation steps updated
✅ doc/ru/installation.md - Installation steps updated
✅ doc/zh/installation.md - Installation steps updated

Optional reviews (feature changes):
⚠️  README.md - Features list (check if new features added)
⚠️  doc/en/*.md - Feature documentation (check if needs updates)
⚠️  doc/ru/*.md - Russian translations (check if needs updates)
⚠️  doc/zh/*.md - Chinese translations (check if needs updates)

Based on the changes analysis:
- FileManager refactoring: May need architecture.md updates (DONE)
- Session autosave: May need configuration docs update
- XDG migration: May need installation docs update

Do you want to:
1. Proceed with release (docs are current)
2. Pause to update docs manually (I'll wait)
3. Cancel release (need more work)
```

Use the platform's interactive prompt mechanism.

If user selects "Pause", inform them:
```
✋ Release paused for documentation updates.

When you're done:
- Commit documentation changes separately, OR
- Leave them uncommitted to include in release commit

Then re-run this skill to continue.
```

### Step 7: Update CHANGELOG.md

The CHANGELOG is **written for humans reading release notes**, not for
agents reading git history. Work from the **themes** produced by Step 2
synthesis, not from the raw commit log.

**Read current CHANGELOG.md** to determine insert position (after header, before first ## version).

**Generate new section:**
```markdown
## [NEW_VERSION] - YYYY-MM-DD

### Added
[New user-facing capabilities — one bullet per theme, not per commit]

### Changed
[Modified behavior the user will notice — workflow, defaults, UX, perf]

### Fixed
[Bugs described as the user saw them, not as the diff fixed them]

### Removed
[Features or options no longer available — call out migration if any]

[NEW_VERSION]: https://github.com/termide/termide/releases/tag/NEW_VERSION
```

**Writing rules:**

1. **One theme = one bullet.** Multiple commits that landed the same
   feature collapse into a single bullet. Three follow-up bugfixes for
   the same feature do not deserve three Fixed entries in the same
   release — fold them into the feature's Added/Changed bullet.

2. **Describe effect, not implementation.** The reader does not know the
   module names. "Faster startup on large projects" beats "lazy-load
   tree-sitter parsers". "Cancel button now actually cancels the
   transfer" beats "wire OperationManager::cancel through chunk-as-
   command actor".

3. **Drop pure noise.** Themes the synthesis pass marked as
   noise/dropped do not appear in the changelog at all. If a fix only
   matters to maintainers, it is not changelog material.

4. **Be specific where it matters.** Vague entries ("various
   improvements", "bug fixes") are worse than nothing — either name the
   improvement concretely or drop the bullet. If you can't say what
   improved, the synthesis pass was incomplete.

5. **Call out breaking changes loudly.** Either a dedicated `### Breaking
   Changes` subsection at the top, or a `**BREAKING:**` prefix on the
   relevant bullet. Include the migration step or workaround.

6. **Past tense, declarative voice.** "Added X.", "Fixed Y.". Not "This
   release adds X" and not "X has been added".

7. **No commit hashes, no PR numbers in the bullets themselves.** Link
   them at the bottom of the section if useful, but keep the bullets
   readable for someone scanning the release notes.

**Sanity-check the draft against the raw log:**

- For every dropped commit, can you justify the drop? (noise, folded,
  reverted, no user effect)
- For every kept commit, is the changelog bullet *more useful* than the
  commit subject? If the bullet is just `git log --oneline | sed`, the
  synthesis didn't happen — rewrite it.
- Is the total bullet count noticeably smaller than the commit count?
  If not, fold more aggressively.

**Show draft to user** and allow editing:
```
Generated CHANGELOG entry:

## [0.3.0] - 2025-12-05

### Added
- XDG Base Directory Specification support for config/data/cache
- Automatic session persistence with configurable retention
- CHANGELOG.md with full project history

### Changed
- **BREAKING:** FileManager is now a regular closable panel.
  Existing sessions will load with the default layout (2 FM panels).
- Simplified layout architecture

### Fixed
- Sessions no longer require special FileManager handling and
  serialize cleanly

Do you want to:
1. Use this changelog as-is
2. Edit it manually before proceeding
3. Regenerate with different categorization
```

Use the platform's interactive prompt mechanism or allow inline editing when supported.

**Insert into CHANGELOG.md:**
Find line after `# Changelog` header and first blank line, insert new section.

**Update version links at bottom:**
Add new link after existing ones:
```markdown
[0.3.0]: https://github.com/termide/termide/releases/tag/0.3.0
```

### Step 8: Post-Update Quality Checks

**After all file updates**, run quality checks again to ensure changes didn't break anything:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build --release
```

If any check fails:
```
❌ Post-update quality check failed!

The version/changelog updates may have introduced issues:
[show specific error]

This could happen if:
- Version string appears in code (not just metadata)
- CHANGELOG.md has syntax errors
- Documentation updates broke something

Please fix the issues and restart the release process.
```

Abort if checks fail.

### Step 9: Create Release Commit

**Check for uncommitted changes:**
```bash
git status --porcelain
```

**Stage all changes:**
```bash
git add -A
```

**Generate commit message:**

Format: Conventional Commits
```
chore: release version NEW_VERSION

Major changes:
- [bullet point 1 from changelog Added/Changed sections]
- [bullet point 2]
- [bullet point 3 if significant breaking change]

[Footer: only if BREAKING CHANGE]
BREAKING CHANGE: [description]
```

Example:
```
chore: release version 0.3.0

Major changes:
- Add XDG Base Directory Specification support
- FileManager is now a regular panel (not fixed left panel)
- Automatic session persistence with cleanup

BREAKING CHANGE: FileManager is no longer a special fixed panel.
Existing sessions will load with default layout (2 FM panels).
```

**Create commit:**
```bash
git commit -m "chore: release version NEW_VERSION

[generated body]"
```

**Verify commit:**
```bash
git log -1 --oneline
git show --stat HEAD
```

Show to user:
```
✅ Release commit created:

abc1234 chore: release version 0.3.0

Files changed:
- Cargo.toml
- Cargo.lock
- flake.nix
- README.md
- README.ru.md
- README.zh.md
- doc/en/installation.md
- doc/ru/installation.md
- doc/zh/installation.md
- packaging/aur/*
- CHANGELOG.md

12 files changed, 87 insertions(+), 42 deletions(-)
```

### Step 10: Create Git Tag

**Tag format:** `NEW_VERSION` (NO 'v' prefix)

**Check if tag exists:**
```bash
if git rev-parse NEW_VERSION >/dev/null 2>&1; then
    # Tag exists
fi
```

**For recreate mode:**
```bash
# Delete local tag
git tag -d NEW_VERSION

# Delete remote tag (if exists)
git push origin :refs/tags/NEW_VERSION 2>/dev/null || true
```

**Create annotated tag:**
```bash
git tag -a NEW_VERSION -m "Release NEW_VERSION"
```

**Verify tag:**
```bash
git show NEW_VERSION --no-patch
```

Show to user:
```
✅ Git tag created:

Tag: 0.3.0
Commit: abc1234 chore: release version 0.3.0
Message: Release 0.3.0
```

### Step 11: Push and Create Release

**IMPORTANT:** Ask user before pushing!

```
🚀 Ready to Push Release

This will push to origin (github.com/termide/termide):
- Commit: abc1234 chore: release version 0.3.0
- Tag: 0.3.0

This will:
1. Push commit and tag to GitHub
2. Create GitHub Release with CHANGELOG description
3. Trigger GitHub Actions workflow which will:
   - Run quality checks (fmt, clippy, test)
   - Build cross-platform binaries (Linux x86/ARM, macOS x86/ARM)
   - Build .deb packages (Debian/Ubuntu)
   - Build .rpm packages (Fedora/RHEL)
   - Upload artifacts to the Release

Expected workflow duration: ~15-20 minutes

Do you want to push now? [yes/no]
```

Use the platform's interactive prompt mechanism.

**If yes:**
```bash
# Push commit and tag
git push && git push origin NEW_VERSION

# Extract changelog section for this version
changelog_content=$(awk '/^## \[NEW_VERSION\]/{flag=1; next} /^## \[/{flag=0} flag' CHANGELOG.md)

# Create GitHub release with changelog as description
gh release create NEW_VERSION \
  --title "Release NEW_VERSION" \
  --notes "$changelog_content"
```

**If no:**
```
Release prepared locally but not pushed.

To push later, run:
  git push && git push origin 0.3.0
  changelog=$(awk '/^## \[0.3.0\]/{flag=1; next} /^## \[/{flag=0} flag' CHANGELOG.md)
  gh release create 0.3.0 --title "Release 0.3.0" --notes "$changelog"

To undo this release:
  git tag -d 0.3.0
  git reset --hard HEAD^
```

### Step 12: Final Report

Show comprehensive release summary:

```
✅ Release 0.3.0 Created Successfully!

📦 Commit: abc1234 chore: release version 0.3.0
🏷️  Tag: 0.3.0
🚀 Pushed to: github.com/termide/termide

📊 Release Stats:
- Files updated: 12
- CHANGELOG sections: Added (3), Changed (3), Fixed (1)
- Quality checks: All passed ✅

🔗 Links:
- Release: https://github.com/termide/termide/releases/tag/0.3.0
- CI/CD Status: https://github.com/termide/termide/actions

⏱️  GitHub Actions Workflow:
The release workflow is now running. Expected completion: 15-20 minutes.

Workflow steps:
1. ✅ Trigger received (tag push)
2. ⏳ Quality checks (fmt, clippy, tests)
3. ⏳ Build binaries (4 platforms)
4. ⏳ Build .deb packages
5. ⏳ Build .rpm packages
6. ⏳ Create GitHub Release
7. ⏳ Update AUR, the Homebrew tap and the website

You can monitor progress at:
https://github.com/termide/termide/actions/workflows/release.yml

📧 Notifications:
GitHub will send you an email when the release is published.
```

The website job (`update-site`) sets the version and the download size on
termide.github.io and rebuilds its pages; it fails without the
`SITE_DEPLOY_KEY` secret. Screenshots are not part of it: when the UI they
show has changed, re-record them with `tools/screenshots/run.sh` and copy
`out/site/` into the website repository.

## Error Handling

### Uncommitted Changes at Start

If `git status --porcelain` shows uncommitted changes:

```
⚠️  Uncommitted changes detected:

M  src/main.rs
M  Cargo.toml

Options:
1. Include in release commit
2. Commit separately first (pause release)
3. Stash and continue (not recommended)
4. Cancel release
```

### Version Tag Already Exists (non-recreate mode)

```
❌ Tag 0.3.0 already exists!

This tag was created: 2025-12-04 19:03:10

Options:
1. Cancel and use different version (0.3.1?)
2. Switch to recreate mode (delete and recreate tag)
3. Delete tag manually and restart release
```

### Quality Check Failures

For any failing check, show:

```
❌ [Check Name] Failed

[Full error output]

Common fixes:
- cargo fmt failure: Run `cargo fmt` to fix formatting
- cargo clippy failure: Fix warnings shown above
- cargo test failure: Fix failing tests
- cargo build failure: Fix compilation errors

After fixing, restart the release process.
```

### Network/Push Failures

```
❌ Failed to push to origin

Error: [git error message]

The release commit and tag are created locally but not pushed.

To retry push:
  git push && git push origin 0.3.0

To undo release:
  git tag -d 0.3.0
  git reset --hard HEAD^
```

## Example Usage

User invokes skill by saying:
- "Create a new release"
- "Release a patch version"
- "Prepare version 0.3.0"
- "Recreate the 0.2.0 release tag"

## Implementation Notes

### Tools Required
- `Read` - Read files for version detection and changelog
- `Edit` - Update version strings in files
- `Bash` - Run git commands and quality checks
- `Grep` - Find version occurrences
- Interactive prompt or input mechanism - Use it for decisions that require user confirmation

### State Management
- Track current step in workflow
- Store user selections (version type, changelog edits)
- Remember old and new version strings
- Keep change analysis results

### Validation
- Validate semantic version format (X.Y.Z)
- Check all quality checks pass before proceeding
- Verify git tag format (no 'v' prefix)
- Ensure conventional commit format

### Safety
- Always run quality checks BEFORE and AFTER file updates
- Ask confirmation before destructive operations (recreate tag, push)
- Provide undo instructions if user wants to cancel
- Show clear diffs of what will change

## Success Criteria

A successful release execution should:
1. ✅ All quality checks pass (twice)
2. ✅ Version updated in all 12+ files consistently
3. ✅ CHANGELOG.md has complete entry
4. ✅ Documentation reviewed/updated as needed
5. ✅ Conventional commit created
6. ✅ Annotated tag created with correct format
7. ✅ Changes pushed to GitHub (if user confirmed)
8. ✅ GitHub Actions workflow triggered
9. ✅ User receives clear next steps and monitoring links

## Maintenance

When updating this skill:
- If new files need version updates, add to Step 5
- If new quality checks needed, add to Steps 1 and 8
- If changelog format changes, update Step 7
- Keep file paths and line numbers accurate with project structure
