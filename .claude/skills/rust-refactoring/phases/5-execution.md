# Phase 5: Execution (Implementation)

**Goal**: Execute refactoring plan batch-by-batch with testing and validation after each change.

**Duration**: 4-5 days (32 hours developer time)

**Critical Feature**: Incremental changes with continuous validation and rollback capability

## Execution Philosophy

```
┌─────────────────────────────────────────────────────┐
│  SAFE REFACTORING CYCLE (repeat for each batch)    │
├─────────────────────────────────────────────────────┤
│                                                     │
│  1. Checkpoint    → Create rollback point          │
│  2. Implement     → Make code changes              │
│  3. Test          → cargo test + clippy            │
│  4. Validate      → Manual verification            │
│  5. Commit        → Save progress                  │
│                                                     │
│  If any step fails → Rollback to checkpoint        │
│                                                     │
└─────────────────────────────────────────────────────┘
```

## Pre-Execution Setup

### Step 5.0: Create Refactoring Branch

```bash
# Create feature branch
git checkout -b refactor-code-quality

# Ensure clean working tree
git status

# Record baseline
echo "Baseline metrics:" > refactor-log.txt
find src -name "*.rs" -exec wc -l {} + | tail -1 >> refactor-log.txt
cargo clippy 2>&1 | grep "warning:" | wc -l >> refactor-log.txt
```

**Output to user**:
```
🚀 EXECUTION STARTING

Branch: refactor-code-quality
Baseline: 14,890 LOC, 34 clippy warnings
Batches: 6 planned
Estimated: 4.5 days

Ready to begin Batch 1...
```

---

## Batch Execution Template

For each batch, follow this pattern:

### Batch Template

```
╔════════════════════════════════════════════════════════════╗
║         BATCH N: [NAME]                                    ║
╚════════════════════════════════════════════════════════════╝

Risk: [Low/Medium/High]
Tasks: [Count]
Estimated: [Hours]

┌─ CHECKPOINT N-1 ──────────────────────────────────────────┐
│ git commit -m "checkpoint: before [batch name]"           │
│ cargo test --all                                          │
│ Status: All tests passing ✓                              │
└───────────────────────────────────────────────────────────┘

TASK N.1: [Task description]
├─ Implementation:
│  [Specific code changes]
│
├─ Testing:
│  cargo test [specific tests]
│  cargo clippy -- [flags]
│
└─ Validation:
   [Manual checks if needed]

[Repeat for each task]

┌─ BATCH N COMPLETE ────────────────────────────────────────┐
│ git commit -m "batch N: [summary]"                        │
│ All tests: ✓ Passing                                     │
│ Clippy: ✓ No new warnings                                │
│ Manual validation: ✓ Complete                            │
└───────────────────────────────────────────────────────────┘

Time taken: [Actual hours]
Issues encountered: [List or "None"]
```

---

## Batch 1: Quick Wins & Cleanup

```
╔════════════════════════════════════════════════════════════╗
║         BATCH 1: QUICK WINS & CLEANUP                      ║
╚════════════════════════════════════════════════════════════╝

Risk: Very Low
Tasks: 4
Estimated: 3 hours
```

### Checkpoint 0
```bash
git add -A
git commit -m "checkpoint: baseline before refactoring"
cargo test --all
```

### Task 1.1: Remove Dead Code (45 items)

**Implementation**:
```bash
# Get list of dead code warnings
cargo check 2>&1 | grep "never used" > /tmp/dead-code.txt

# For each dead code item:
# 1. Use Edit tool to remove the code
# 2. Verify it compiles
# 3. Run affected tests
```

**Example**:
```rust
// Remove unused function
// File: src/utils/helper.rs:45-67
fn unused_helper() {  // DELETE THIS ENTIRE FUNCTION
    // ...
}
```

**Testing**:
```bash
cargo check
cargo test --package termide --lib utils
```

**Expected**: Compiles successfully, tests pass, 45 warnings removed

---

### Task 1.2: Fix Unused Imports (12 items)

**Implementation**:
```bash
# Run cargo with unused import warnings
cargo clippy -- -W unused_imports 2>&1 | tee /tmp/unused-imports.txt

# For each unused import, use Edit tool to remove it
```

**Testing**:
```bash
cargo check
# Should see: "warning count reduced by 12"
```

---

### Task 1.3: Extract Repeated Constants

**Implementation**:
```rust
// Create src/constants.rs
pub mod limits {
    pub const MAX_FILES: usize = 1000;
    pub const MAX_BUFFER_SIZE: usize = 10_000;
}

// Update all files using these magic numbers
use crate::constants::limits::MAX_FILES;
```

**Testing**:
```bash
cargo test --all
```

---

### Batch 1 Completion

```bash
git add src/
git commit -m "refactor: remove dead code and fix imports

- Removed 45 unused functions and variables
- Fixed 12 unused imports
- Extracted 3 repeated constants to shared module

Reduces clippy warnings from 34 to 22"
```

**Output to user**:
```
✅ BATCH 1 COMPLETE

Time: 2.5 hours (✓ under estimate)
Changes: 57 items cleaned up
LOC reduction: ~350 lines
Clippy warnings: 34 → 22 (-12)

All tests passing ✓
No issues encountered

Proceeding to Batch 2...
```

---

## Batch 2: Performance Algorithm Fixes

```
╔════════════════════════════════════════════════════════════╗
║         BATCH 2: PERFORMANCE - ALGORITHM FIXES             ║
╚════════════════════════════════════════════════════════════╝

Risk: Low
Tasks: 3
Estimated: 6 hours
```

### Task 2.1: Fix O(n²) in file_manager

**Current code** (src/panels/file_manager.rs:456):
```rust
// O(n²) - BAD
fn find_duplicates(files: &[String]) -> Vec<String> {
    let mut duplicates = Vec::new();
    for i in 0..files.len() {
        for j in (i+1)..files.len() {
            if files[i] == files[j] {
                duplicates.push(files[i].clone());
            }
        }
    }
    duplicates
}
```

**Refactored code**:
```rust
// O(n) - GOOD
use std::collections::HashSet;

fn find_duplicates(files: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut duplicates = HashSet::new();

    for file in files {
        if !seen.insert(file) {
            duplicates.insert(file.clone());
        }
    }

    duplicates.into_iter().collect()
}
```

**Implementation steps**:
1. Use Edit tool to replace the function
2. Add `use std::collections::HashSet;` at top of file
3. Run tests

**Testing**:
```bash
cargo test --package termide --test integration_tests file_manager
cargo clippy -- -W clippy::all
```

**Validation**:
- Test with 1000 files: should be instant (vs 500ms before)
- Verify duplicate detection still correct

---

### Batch 2 Completion

```bash
git add src/panels/file_manager.rs src/editor/search.rs src/logger.rs
git commit -m "perf: optimize algorithmic complexity

- Replace O(n²) loop with O(n) HashSet in file_manager
- Use binary search instead of linear scan in editor
- Optimize string concatenation in logger

Estimated speedups:
- File duplicate detection: 100x faster
- Editor search: 50x faster
- Logger: 30% memory reduction"

# Run benchmarks if available
cargo bench 2>&1 | tee /tmp/bench-after-batch2.txt
```

---

## Batch 3: Architectural Refactoring (CRITICAL)

```
╔════════════════════════════════════════════════════════════╗
║         BATCH 3: ARCHITECTURAL REFACTORING                 ║
║         ⚠️  HIGH RISK - CAREFUL EXECUTION REQUIRED         ║
╚════════════════════════════════════════════════════════════╝

Risk: Medium-High
Tasks: 4-5 (includes optional file decomposition)
Estimated: 8-12 hours (depending on decomposition scope)
```

### Critical Pre-Batch Steps

```bash
# Extra checkpoint before risky changes
git branch checkpoint-before-batch3
git tag refactor-safe-point

# Run full test suite
cargo test --all -- --test-threads=1

# Verify all tests pass
echo "Tests must be 100% passing before proceeding"
```

### Task 3.0: File Decomposition (if applicable)

**When to do this**: If file decomposition opportunities were identified in Phase 2 and prioritized in Phase 3.

**Example**: Decompose src/editor/mod.rs (1847 LOC → 5 files)

**Strategy**: Logical grouping - extract cursor, selection, rendering, and history logic

**Step-by-step process**:

```bash
# Step 1: Create subdirectory for decomposed modules
mkdir -p src/editor/

# Step 2: Identify extraction targets
# Read the large file and identify logical boundaries
# Look for:
# - Groups of related functions
# - Large impl blocks
# - Distinct responsibilities (cursor vs rendering vs history)

# Step 3: Extract first module (e.g., cursor logic)
# Create src/editor/cursor.rs with cursor-related code
# Update imports in original file
# Add pub mod cursor; to mod.rs

# Step 4: Verify after each extraction
cargo check
cargo test editor::cursor

# Step 5: Repeat for each module
# - src/editor/selection.rs (selection management)
# - src/editor/render.rs (rendering logic)
# - src/editor/history.rs (undo/redo)

# Step 6: Update main mod.rs with re-exports
# pub mod cursor;
# pub mod selection;
# pub mod render;
# pub mod history;
#
# pub use cursor::*;
# pub use selection::*;
# // etc.

# Step 7: Update imports in dependent files
grep -r "use.*editor::" src/ | # Find all editor imports
# Update paths if needed (usually re-exports make this transparent)
```

**Example decomposition**:

```rust
// Before: src/editor/mod.rs (1847 LOC)
pub struct Editor {
    buffer: Buffer,
    cursor: CursorState,
    selection: Option<Selection>,
    history: Vec<HistoryEntry>,
    // ...
}

impl Editor {
    // 400 LOC of cursor operations
    pub fn move_cursor_up(&mut self) { ... }
    pub fn move_cursor_down(&mut self) { ... }
    // ...

    // 350 LOC of selection operations
    pub fn start_selection(&mut self) { ... }
    pub fn extend_selection(&mut self) { ... }
    // ...

    // 450 LOC of rendering
    pub fn render(&self) -> Vec<Line> { ... }
    // ...

    // 450 LOC of history/undo
    pub fn undo(&mut self) { ... }
    pub fn redo(&mut self) { ... }
    // ...
}

// After: src/editor/mod.rs (200 LOC)
pub mod cursor;
pub mod selection;
pub mod render;
pub mod history;

pub use cursor::CursorOps;
pub use selection::SelectionOps;
pub use render::Renderer;
pub use history::History;

pub struct Editor {
    buffer: Buffer,
    cursor: cursor::CursorState,
    selection: Option<selection::Selection>,
    history: history::History,
}

// Core impl only (orchestration)
impl Editor {
    pub fn new() -> Self { ... }
    // Delegates to specialized modules
}

// src/editor/cursor.rs (400 LOC)
pub struct CursorState { ... }
pub trait CursorOps {
    fn move_cursor_up(&mut self);
    fn move_cursor_down(&mut self);
    // ...
}
impl CursorOps for super::Editor { ... }

// Similar for selection.rs, render.rs, history.rs
```

**Testing**:
```bash
# After each file extraction:
cargo check  # Ensure it compiles
cargo test editor  # Run all editor tests
cargo test --all  # Ensure nothing broke elsewhere

# Verify imports work correctly
grep -r "use.*editor" src/ | # Check all uses still work
```

**Benefits**:
- Reduces cognitive load (200 LOC vs 1847 LOC per file)
- Improves code navigation and findability
- Enables parallel development
- Often reveals SRP violations (helps with subsequent SOLID fixes)

**Estimated effort**: 2-4 hours depending on file complexity

**Rollback**: If decomposition introduces issues:
```bash
git reset --hard checkpoint-before-batch3
# Decomposition can be deferred to later batch or skipped
```

---

### Task 3.1: Extract FileSystem Trait

**Create new file**: src/filesystem.rs

```rust
use std::path::{Path, PathBuf};
use std::io::Result;

/// Abstraction for filesystem operations
/// Allows mocking in tests, supports SRP
pub trait FileSystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>>;
    fn write(&self, path: &Path, data: &[u8]) -> Result<()>;
    fn list(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn exists(&self, path: &Path) -> bool;
    fn delete(&self, path: &Path) -> Result<()>;
}

/// Real filesystem implementation
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &Path, data: &[u8]) -> Result<()> {
        std::fs::write(path, data)
    }

    fn list(&self, path: &Path) -> Result<Vec<PathBuf>> {
        // Implementation
    }

    // ... other methods
}

#[cfg(test)]
pub struct MockFileSystem {
    // Test implementation
}
```

**Update Cargo.toml** if needed (add to lib.rs):
```rust
pub mod filesystem;
```

**Testing**:
```bash
cargo check
cargo test filesystem::tests
```

---

### Task 3.2: Refactor FileManager

**IMPORTANT**: This is invasive. Do incrementally.

**Step 1**: Create FileManagerView (new struct)
**Step 2**: Move UI code to FileManagerView
**Step 3**: Move filesystem ops to separate impl
**Step 4**: Inject FileSystem trait
**Step 5**: Update call sites (24 locations)

**After EACH step**: `cargo test`

---

### Batch 3 Completion

```bash
# If all tests pass:
git add src/filesystem.rs src/panels/file_manager.rs
git commit -m "refactor: extract FileSystem trait and split FileManager (SRP)

Major architectural improvement:
- Created FileSystem trait abstraction
- Split FileManagerView (UI) from FileSystemOps (logic)
- Enables testing with mocks
- Fixes 8 SOLID violations

BREAKING CHANGE: Internal API only, no public API affected"

# If tests fail:
git reset --hard checkpoint-before-batch3
echo "Batch 3 failed. Investigate issues before retrying."
exit 1
```

---

## Batch 4-6: Continue Pattern

[Follow same template for remaining batches]

---

## Error Handling During Execution

### If Tests Fail

```bash
# Step 1: Identify failing test
cargo test 2>&1 | grep "FAILED"

# Step 2: Run specific test with output
cargo test failing_test_name -- --nocapture

# Step 3: Determine if it's:
# - Bug in refactoring → Fix and retry
# - Existing bug exposed → Fix separately
# - Test needs update → Update test

# Step 4: If can't fix quickly:
git reset --hard [last-checkpoint]
```

### If Clippy Introduces New Warnings

```bash
# Acceptable: Refactor reveals existing issues
cargo clippy 2>&1 | grep "warning:"

# Fix new warnings immediately
# OR: Add to backlog if unrelated
```

### If Performance Regresses

```bash
# Run benchmarks
cargo bench

# Compare to baseline
# If regression > 10%:
# - Investigate cause
# - Revert if necessary
# - Re-approach optimization
```

---

## Progress Tracking

Update user after each batch:

```
📊 REFACTORING PROGRESS

[████████████████░░░░░░░░░░░░] 60% Complete

Completed:
✅ Batch 1: Quick wins (2.5h)
✅ Batch 2: Performance algorithms (6h)
✅ Batch 3: Architectural refactor (9h - over estimate)

Current:
🔄 Batch 4: Memory optimization (in progress)

Remaining:
⏳ Batch 5: DRY consolidation
⏳ Batch 6: Polish & docs

Metrics so far:
- LOC reduced: 420 lines
- Clippy warnings: 34 → 8 (-26)
- Tests: All passing ✓
- Performance: +150% on file operations

Estimated completion: Tomorrow afternoon
```

---

## Tools Used

- **Edit**: Make code changes
- **Read**: Review current code before changes
- **Bash**: Run cargo commands, tests, git
- **Grep**: Find code patterns to refactor

## Success Criteria

- [x] All planned batches executed
- [x] Tests passing after each batch
- [x] No clippy regressions
- [x] Checkpoints created
- [x] Changes committed incrementally

---

**Proceed to Phase 6: Verification**
