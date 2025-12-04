# Release Management Skill

Comprehensive release management workflow for TermIDE project that ensures code quality, version consistency, and proper changelog documentation before creating releases.

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

Analyze changes from **three sources** as requested by user:

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

# Get commits since tag
if [ -n "$last_tag" ]; then
    git log ${last_tag}..HEAD --oneline --no-decorate
else
    git log --oneline --no-decorate
fi
```

Parse commit messages to categorize by conventional commit types:
- `feat:` → Added section
- `fix:` → Fixed section
- `docs:` → Changed section (if user-facing docs)
- `refactor:` → Changed section
- `perf:` → Changed section
- `chore:` → Usually not in changelog unless significant
- `BREAKING CHANGE:` → Special highlight in changelog

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

**Combine all three sources** to create comprehensive change analysis:
- Uncommitted changes → Include in "Pending changes" section
- Commits → Parse messages and actual file changes
- File states → Validate commit messages match reality

**Output structure:**
```
Changes Analysis:
==================

Uncommitted Changes:
- Modified: src/main.rs, Cargo.toml
- Added: CHANGELOG.md

Commits Since 0.2.0:
- 6d2e247 refactor: remove FileManager special handling
- 4e7c694 feat: migrate to XDG Base Directory Specification
- 6509fdb feat: implement session autosave

Categorized Changes:
Added:
- XDG Base Directory Specification support
- Session autosave with debounce
- CHANGELOG.md file

Changed:
- FileManager is now a regular panel (BREAKING CHANGE)
- Simplified layout architecture (-350 LOC)

Fixed:
- [none detected]
```

### Step 3: Detect Current Version

Check version in multiple files and detect inconsistencies:

```bash
# Cargo.toml (primary source)
cargo_version=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

# flake.nix
flake_version=$(grep 'version = ' flake.nix | head -1 | sed 's/.*version = "\(.*\)";/\1/')

# Last git tag
git_version=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")

# i18n files
en_version=$(grep -A 1 'pub fn help_version' src/i18n/en.rs | grep return | sed 's/.*"\(.*\)".*/\1/')
ru_version=$(grep -A 1 'pub fn help_version' src/i18n/ru.rs | grep return | sed 's/.*"\(.*\)".*/\1/')
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
src/i18n/en.rs          0.1.5  ⚠️
src/i18n/ru.rs          0.1.5  ⚠️

Which version is correct as the current version?
1. 0.2.0 (Cargo.toml + git tag)
2. 0.1.5 (flake.nix + i18n)
3. Other (specify manually)
```

Use AskUserQuestion tool for this.

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

Use AskUserQuestion tool with options:
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

Update version `NEW_VERSION` in these 12+ files using Edit tool:

#### 1. Cargo.toml
```toml
version = "NEW_VERSION"
```
Line 3, exact match: `version = "OLD_VERSION"`

#### 2. flake.nix
```nix
version = "NEW_VERSION";
```
Around line 68, exact match: `version = "OLD_VERSION";`

#### 3. README.md (8 occurrences)
Replace all download URLs:
```
termide-OLD_VERSION-x86_64-unknown-linux-gnu.tar.gz
→
termide-NEW_VERSION-x86_64-unknown-linux-gnu.tar.gz
```

Pattern: `termide-\d+\.\d+\.\d+-` → `termide-NEW_VERSION-`

Also update version tags in examples:
```
download/OLD_VERSION/
→
download/NEW_VERSION/
```

Pattern: `download/\d+\.\d+\.\d+/` → `download/NEW_VERSION/`

#### 4. doc/en/installation.md (4 occurrences)
Same pattern as README.md for download URLs.

#### 5. doc/ru/installation.md (4 occurrences)
Same pattern as README.md for download URLs.

#### 6. src/i18n/en.rs
```rust
pub fn help_version(&self) -> &'static str {
    "NEW_VERSION"
}
```
Line ~222, replace string literal.

#### 7. src/i18n/ru.rs
```rust
pub fn help_version(&self) -> &'static str {
    "NEW_VERSION"
}
```
Line ~236, replace string literal.

#### 8. packaging/homebrew/termide.rb (5 occurrences)
```ruby
version "NEW_VERSION"
url "https://github.com/termide/termide/archive/refs/tags/NEW_VERSION.tar.gz"
sha256 "..."  # This will need to be updated AFTER release
```

Update version and URL, note that sha256 will be wrong until after release.

#### 9. packaging/aur/PKGBUILD
```bash
pkgver=NEW_VERSION
```
Line 4, simple replacement.

#### 10. packaging/aur/PKGBUILD-bin
```bash
pkgver=NEW_VERSION
```
Line 4, simple replacement.

**Batch Update Strategy:**
Use replace_all=true where appropriate for URL patterns in README and docs.

**Verification:**
After updates, grep for old version to ensure all replaced:
```bash
grep -r "OLD_VERSION" --exclude-dir=.git --exclude-dir=target --exclude=CHANGELOG.md .
```

### Step 6: Documentation Actuality Check

After version updates, prompt user to review documentation:

```
📝 Documentation Review

Version numbers have been updated in:
- README.md
- doc/en/installation.md
- doc/ru/installation.md

Please review these files for content accuracy:

Required reviews (version-critical):
✅ README.md - Download links updated
✅ doc/en/installation.md - Installation steps updated
✅ doc/ru/installation.md - Installation steps updated

Optional reviews (feature changes):
⚠️  README.md - Features list (check if new features added)
⚠️  doc/en/*.md - Feature documentation (check if needs updates)
⚠️  doc/ru/*.md - Russian translations (check if needs updates)
⚠️  help/en.txt - Keyboard shortcuts (check if any changed)
⚠️  help/ru.txt - Keyboard shortcuts (check if any changed)

Based on the changes analysis:
- FileManager refactoring: May need architecture.md updates (DONE)
- Session autosave: May need configuration docs update
- XDG migration: May need installation docs update

Do you want to:
1. Proceed with release (docs are current)
2. Pause to update docs manually (I'll wait)
3. Cancel release (need more work)
```

Use AskUserQuestion tool.

If user selects "Pause", inform them:
```
✋ Release paused for documentation updates.

When you're done:
- Commit documentation changes separately, OR
- Leave them uncommitted to include in release commit

Then re-run this skill to continue.
```

### Step 7: Update CHANGELOG.md

Generate new CHANGELOG section from analyzed changes.

**Read current CHANGELOG.md** to determine insert position (after header, before first ## version).

**Generate new section:**
```markdown
## [NEW_VERSION] - YYYY-MM-DD

### Added
[List features from feat: commits and new files detected]

### Changed
[List from refactor:, perf:, chore: commits and file changes]

### Fixed
[List from fix: commits]

### Removed
[List any deleted features/files]

[NEW_VERSION]: https://github.com/termide/termide/releases/tag/NEW_VERSION
```

**Categorization logic:**

For each commit message:
- Parse conventional commit type
- Extract description after type
- Place in appropriate section

For uncommitted changes:
- Parse file changes
- Categorize based on file paths:
  - src/panels/new_panel.rs → Added
  - Modified existing panel → Changed
  - Deleted file → Removed

**Show draft to user** and allow editing:
```
Generated CHANGELOG entry:

## [0.3.0] - 2025-12-05

### Added
- XDG Base Directory Specification support for config/data/cache
- Automatic session persistence with configurable retention
- CHANGELOG.md with full project history

### Changed
- FileManager is now a regular closable panel (BREAKING CHANGE)
- Simplified layout architecture (-350 lines of code)
- Default initialization with 2 FileManager panels (50/50 layout)

### Fixed
- Session serialization without special FileManager handling

Do you want to:
1. Use this changelog as-is
2. Edit it manually before proceeding
3. Regenerate with different categorization
```

Use AskUserQuestion tool or allow inline editing.

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
- flake.nix
- README.md
- doc/en/installation.md
- doc/ru/installation.md
- src/i18n/en.rs
- src/i18n/ru.rs
- packaging/*
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

### Step 11: Push to Remote

**IMPORTANT:** Ask user before pushing!

```
🚀 Ready to Push Release

This will push to origin (github.com/termide/termide):
- Commit: abc1234 chore: release version 0.3.0
- Tag: 0.3.0

This will trigger GitHub Actions workflow which will:
1. Run quality checks (fmt, clippy, test)
2. Build cross-platform binaries (Linux x86/ARM, macOS x86/ARM)
3. Build .deb packages (Debian/Ubuntu)
4. Build .rpm packages (Fedora/RHEL)
5. Publish to crates.io
6. Create GitHub Release with all artifacts

Expected workflow duration: ~15-20 minutes

Do you want to push now? [yes/no]
```

Use AskUserQuestion tool.

**If yes:**
```bash
git push && git push origin NEW_VERSION
```

**If no:**
```
Release prepared locally but not pushed.

To push later, run:
  git push && git push origin 0.3.0

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
- Crates.io: https://crates.io/crates/termide

⏱️  GitHub Actions Workflow:
The release workflow is now running. Expected completion: 15-20 minutes.

Workflow steps:
1. ✅ Trigger received (tag push)
2. ⏳ Quality checks (fmt, clippy, tests)
3. ⏳ Build binaries (4 platforms)
4. ⏳ Build .deb packages
5. ⏳ Build .rpm packages
6. ⏳ Publish to crates.io
7. ⏳ Create GitHub Release

You can monitor progress at:
https://github.com/termide/termide/actions/workflows/release.yml

📧 Notifications:
GitHub will send you an email when the release is published.
```

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
- `AskUserQuestion` - Interactive prompts for decisions

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
