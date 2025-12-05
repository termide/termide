# Phase 4: Planning (Roadmap Creation)

**Goal**: Create detailed step-by-step refactoring plan with dependencies, rollback points, and risk mitigation.

**Duration**: ~5 minutes

## Objectives

1. Map dependencies between refactoring tasks
2. Identify safe rollback points
3. Group related changes into logical batches
4. Create detailed execution order
5. **INTERACT**: Get user approval before proceeding

## Step-by-Step Process

### Step 4.1: Analyze Task Dependencies

**Identify which tasks must be done in specific order**:

**Example dependency analysis**:
```
Task 1: Remove dead code imports
  ├─ No dependencies
  └─ Safe to do first

Task 2: Fix O(n²) loop in file_manager
  ├─ Depends on: Task 1 (cleaner code)
  └─ Must be done before: Task 3 (FileManager refactor)

Task 3: Refactor FileManager for SRP
  ├─ Depends on: Task 2 (performance fixes)
  ├─ Blocks: Task 7 (DRY consolidation in FM)
  └─ Major change - needs checkpoint

Task 4: Optimize buffer operations
  ├─ No dependencies (separate module)
  └─ Can run parallel to Task 3
```

**Create dependency graph**:
```
         ┌─────────┐
         │ Task 1  │ (Dead code removal)
         └────┬────┘
              │
         ┌────▼────┐
         │ Task 2  │ (Performance fix)
         └────┬────┘
              │
         ┌────▼────┐     ┌────────┐
         │ Task 3  │     │ Task 4 │ (Can run parallel)
         └────┬────┘     └────────┘
              │
         ┌────▼────┐
         │ Task 7  │ (DRY consolidation)
         └─────────┘
```

### Step 4.2: Define Rollback Checkpoints

**Create safety checkpoints** between major changes:

```
CHECKPOINT STRATEGY:

Checkpoint 0: Initial state
├─ Create git branch: refactor-phase1
├─ Ensure all tests pass
└─ Record baseline metrics

Checkpoint 1: After quick wins (Tasks 1-5)
├─ Commit: "refactor: remove dead code and fix imports"
├─ Run full test suite
├─ Run clippy
└─ If fails: git reset --hard

Checkpoint 2: After performance fixes (Tasks 6-10)
├─ Commit: "perf: optimize hot paths and algorithms"
├─ Run benchmarks (if available)
├─ Verify no regressions
└─ Tag: refactor-perf-fixes

Checkpoint 3: After architectural changes (Tasks 11-15)
├─ Commit: "refactor: improve SOLID compliance"
├─ Extensive testing required
└─ Major rollback point

Checkpoint 4: Final state
├─ All tasks complete
├─ Full verification
└─ Merge to main
```

### Step 4.3: Group Tasks into Batches

**Organize by type and risk**:

```
╔════════════════════════════════════════════════════════════╗
║           REFACTORING EXECUTION ROADMAP                    ║
╚════════════════════════════════════════════════════════════╝

BATCH 1: Quick Wins & Cleanup (Day 1 morning)
Risk: Very Low | Rollback: Easy

│ Task 1.1 │ Remove 45 dead code items
│ Task 1.2 │ Fix 12 unused imports
│ Task 1.3 │ Clean up 8 TODO/FIXME markers
│ Task 1.4 │ Extract 3 repeated constants

Effort: 3 hours
Tests affected: None (removals only)
Rollback: git reset

─────────────────────────────────────────────────

BATCH 2: Performance - Algorithm Fixes (Day 1 afternoon)
Risk: Low | Rollback: Moderate

│ Task 2.1 │ Fix O(n²) in file_manager::find_duplicates
│          │ Replace nested loops with HashSet
│          │ Estimated speedup: 100x for 1000+ files
│
│ Task 2.2 │ Optimize editor::search linear scans
│          │ Use binary search for sorted data
│          │ Estimated speedup: 50x
│
│ Task 2.3 │ Fix string concatenation in logger
│          │ Use String::with_capacity + push_str
│          │ Memory reduction: ~30%

Effort: 6 hours
Tests affected: file_manager, editor, logger (unit tests)
Rollback: Revert specific commits per task

─────────────────────────────────────────────────

CHECKPOINT: Run full test suite, benchmarks
Expected result: All tests pass, perf improves
If issues: Revert Batch 2, investigate

─────────────────────────────────────────────────

BATCH 3: Architectural - FileManager Refactor + File Decomposition (Day 2)
Risk: Medium | Rollback: Careful required

│ Task 3.1 │ Decompose large files (if prioritized)
│          │ Example: Split editor/mod.rs (1847 LOC → 5 files)
│          │ Strategy: Extract cursor.rs, selection.rs, render.rs, history.rs
│          │ Note: May be done before or as part of SOLID fixes
│
│ Task 3.2 │ Extract FileSystem trait
│          │ Create abstraction for filesystem ops
│
│ Task 3.3 │ Split FileManagerView from FileSystem
│          │ Separate UI concerns from business logic (SRP)
│
│ Task 3.4 │ Update all FileManager call sites
│          │ Migrate to new structure (24 locations)
│
│ Task 3.5 │ Add unit tests for FileSystem trait
│          │ Mock implementation for testing

Effort: 8-12 hours (1-1.5 days, depending on decomposition tasks)
Tests affected: ALL file_manager tests, integration tests, decomposed modules
Breaking change: Internal API only
Rollback: Revert to Checkpoint 2

─────────────────────────────────────────────────

BATCH 4: Performance - Memory Optimization (Day 3)
Risk: Medium | Rollback: Moderate

│ Task 4.1 │ Remove buffer cloning in editor
│          │ Modify in-place instead of clone-modify
│
│ Task 4.2 │ Reduce allocations in render loop
│          │ Reuse buffers, avoid String::from
│
│ Task 4.3 │ Optimize i18n string lookups
│          │ Use &'static str instead of String where possible

Effort: 6 hours
Tests affected: editor tests, render tests
Rollback: Per-task revert

─────────────────────────────────────────────────

BATCH 5: DRY & Code Consolidation (Day 4)
Risk: Low | Rollback: Easy

│ Task 5.1 │ Extract 4 duplicated helper functions
│          │ Move to shared utils module
│
│ Task 5.2 │ Consolidate error handling patterns
│          │ Create common error conversion helpers
│
│ Task 5.3 │ Reduce match duplication with enum methods
│          │ Add Status::display() and Status::color()

Effort: 5 hours
Tests affected: Minimal (mostly moves)
Rollback: Easy

─────────────────────────────────────────────────

BATCH 6: Polish & Documentation (Day 5)
Risk: Very Low | Rollback: Not needed

│ Task 6.1 │ Add documentation to 12 public APIs
│ Task 6.2 │ Improve naming in 8 functions
│ Task 6.3 │ Add module-level documentation
│ Task 6.4 │ Update CHANGELOG with refactoring notes

Effort: 4 hours
Tests affected: None
Rollback: Not applicable
```

### Step 4.4: Estimate Timeline and Resources

```
OVERALL TIMELINE: 4.5 days

Day 1: Batches 1-2 (Quick wins + Perf algos)  ✓
Day 2: Batch 3 (FileManager refactor)         ⚠️ Critical
Day 3: Batch 4 (Memory optimization)          ⚠️ Testing heavy
Day 4: Batch 5 (DRY consolidation)            ✓
Day 5: Batch 6 (Polish + docs)                ✓

RESOURCE REQUIREMENTS:
- Developer time: 32 hours (4.5 days)
- Test environment: Needed for Batch 3
- Code review: Recommended after Batch 3, 4
- Benchmark tools: For performance validation

RISKS BY BATCH:
Batch 1: ████░░░░░░ (10% risk)
Batch 2: ████████░░ (20% risk)
Batch 3: ████████████████░░ (60% risk) ⚠️
Batch 4: ████████████░░ (40% risk)
Batch 5: ████░░░░░░ (10% risk)
Batch 6: ░░░░░░░░░░ (0% risk)
```

### Step 4.5: Create Risk Mitigation Plan

**For each high-risk batch**:

```
BATCH 3 RISK MITIGATION (FileManager refactor):

Pre-execution:
✓ Review current FileManager usage (24 call sites)
✓ Write integration tests BEFORE refactor
✓ Create feature branch
✓ Ensure 100% test coverage on FileManager

During execution:
✓ Make changes incrementally (trait → split → migrate → test)
✓ Run tests after each sub-task
✓ Keep old code until new code is verified

Post-execution:
✓ Manual testing in actual terminal
✓ Check memory usage hasn't regressed
✓ Performance benchmark comparison

Rollback plan:
1. If tests fail: Revert to Checkpoint 2
2. If behavior changes: Investigate with git bisect
3. If performance regresses: Revert specific commits

Monitoring:
- Watch for increased compilation time
- Check binary size hasn't grown significantly
- Verify no new clippy warnings
```

### Step 4.6: INTERACTIVE - Get User Approval

**Use AskUserQuestion tool**:

```
Review the execution roadmap above.

The plan includes:
- 6 batches over 4.5 days
- 3 checkpoints for safe rollback
- Highest risk: Day 2 (FileManager refactor)

Do you want to proceed with this plan?
```

**Options**:
1. **Yes, execute as planned** - Start Phase 5
2. **Modify priorities** - Return to Phase 3
3. **Skip high-risk items** - Remove Batch 3, proceed with others
4. **Review specific batch** - Show detailed steps for one batch

**Store answer as**: `user_plan_approval`

**If user selects "Review specific batch"**, show detailed task breakdown:

```
DETAILED: BATCH 3 - FileManager Refactor

Task 3.1: Extract FileSystem trait (2 hours)
├─ Step 1: Define trait in src/filesystem.rs
│  pub trait FileSystem {
│      fn read(&self, path: &Path) -> Result<Vec<u8>>;
│      fn write(&self, path: &Path, data: &[u8]) -> Result<()>;
│      fn list(&self, path: &Path) -> Result<Vec<PathBuf>>;
│  }
│
├─ Step 2: Create concrete RealFileSystem implementation
│  Uses std::fs under the hood
│
└─ Step 3: Run: cargo check
   Expected: Compiles successfully

Task 3.2: Split FileManagerView (3 hours)
├─ Step 1: Create new FileManagerView struct
│  Handles only UI rendering and event handling
│
├─ Step 2: Move filesystem ops to FileSystemOps
│  Implements FileSystem trait
│
├─ Step 3: Inject FileSystem into FileManagerView
│  struct FileManagerView { fs: Box<dyn FileSystem> }
│
└─ Step 4: Run: cargo test
   Expected: Existing tests still pass

Task 3.3: Update call sites (2 hours)
├─ Step 1: Find all FileManager::new() calls (24 locations)
│  grep -rn "FileManager::new" src/
│
├─ Step 2: Update to: FileManagerView::new(RealFileSystem::new())
│
└─ Step 3: Run: cargo test
   Expected: All tests pass

Task 3.4: Add tests (1 hour)
├─ Create MockFileSystem for testing
│  Returns predictable data
│
├─ Write unit tests using mock
│  Test error handling, edge cases
│
└─ Run: cargo test
   Expected: New tests pass

Total: 8 hours, 4 checkpoints
```

## Phase 4 Output

Present final plan to user:

```
╔════════════════════════════════════════════════════════════╗
║         PHASE 4: PLANNING - COMPLETE ✓                     ║
╚════════════════════════════════════════════════════════════╝

📋 EXECUTION ROADMAP CREATED

Timeline: 4.5 days (32 hours)
Batches: 6 logical groups
Checkpoints: 4 rollback points
Highest risk: Batch 3 (60% - but mitigated)

BATCH SUMMARY:
Day 1: Quick wins + Performance algorithms   ✓ Low risk
Day 2: FileManager architectural refactor    ⚠️  Medium risk
Day 3: Memory optimizations                  ⚠️  Medium risk
Day 4: DRY consolidation                     ✓ Low risk
Day 5: Polish + documentation                ✓ No risk

SAFETY MEASURES:
✓ Git checkpoints every batch
✓ Tests run after each task
✓ Incremental changes
✓ Rollback plan for each batch

EXPECTED OUTCOME:
Performance score: 5.2 → 8.1
Overall score: 6.5 → 7.8
LOC reduction: ~400 lines

🎯 NEXT PHASE: Execution
   Will implement changes batch-by-batch with testing.

User approved plan. Proceeding...
```

## Tools Used

- **AskUserQuestion**: Plan approval
- **Bash**: Dependency analysis
- **Read**: Review prioritized tasks from Phase 3

## Success Criteria

- [x] Dependencies mapped
- [x] Execution batches defined
- [x] Rollback checkpoints identified
- [x] Timeline estimated
- [x] Risk mitigation planned
- [x] User approval obtained

## State to Carry Forward

Store for Phase 5 (Execution):
- Batch definitions with task lists
- Checkpoint strategy
- Rollback procedures
- Risk mitigation plans
- User approval status

---

**Proceed to Phase 5: Execution**
