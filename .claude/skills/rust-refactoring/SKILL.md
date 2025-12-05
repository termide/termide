# Rust Refactoring Skill

Comprehensive Rust codebase analysis and refactoring system with parallel execution, architectural improvements, and best practice enforcement.

## Overview

This skill provides systematic refactoring for Rust projects following DRY, KISS, and SOLID principles. It analyzes code quality, identifies issues, prioritizes fixes, and executes refactoring with continuous validation. Includes intelligent application of "1 file = 1 structure" principle for improved modularity.

**Key Features**:
- ⚡ **Parallel Analysis** - 3 independent analyzers run concurrently (60% faster)
- 📊 **Quality Scoring** - 10-point scale across 4 dimensions
- 🎯 **Interactive Prioritization** - User-driven execution plan
- 🔄 **Safe Execution** - Incremental changes with rollback points
- ✅ **Continuous Validation** - Tests + clippy after every change
- 📁 **Smart Decomposition** - "1 file = 1 structure" with intelligent exceptions

## When to Use This Skill

Invoke `/rust-refactor` or `/rust-refactoring` when you want to:

1. **Analyze code quality** - Get comprehensive assessment of architecture, performance, maintainability
2. **Improve performance** - Identify and fix algorithmic inefficiencies, memory issues
3. **Enforce best practices** - Apply SOLID principles, remove dead code, eliminate duplication
4. **Prepare for scale** - Refactor before adding major features
5. **Clean technical debt** - Systematic approach to improving existing codebase

**Not recommended for**:
- Greenfield projects (no code to refactor yet)
- Quick bug fixes (too heavyweight)
- Projects that don't compile (fix compilation first)

## Best Practice: "1 File = 1 Structure" Rule

This skill enforces the **"1 file = 1 structure"** principle where appropriate:

**The Rule**: Each significant type (struct, enum, trait) should ideally live in its own file.

**Benefits**:
- 🔍 Clear file-to-type mapping for easy navigation
- 📦 Single responsibility per file
- 🔄 Reduced merge conflicts
- 🧪 Test structure mirrors source structure

**Intelligent Application**:
The skill recognizes legitimate exceptions and won't force splitting when types should stay together:
- ✅ **Error + ErrorKind** pattern (tightly coupled types)
- ✅ **Builder patterns** (ConfigBuilder + Config)
- ✅ **Small helpers** (<30 LOC private types supporting main type)
- ✅ **DTO families** (related Request/Response types in same API domain)
- ✅ **Typestate patterns** (marker types for compile-time state)
- ✅ **Newtype collections** (multiple simple wrapper types)

**When Splitting is Required**:
- 2+ public types >80 LOC each in unrelated domains → **MUST SPLIT**
- File >1000 LOC with multiple types → **MUST SPLIT**
- 3+ public types regardless of size → **SHOULD SPLIT** (check exceptions first)

**Anti-Pattern to Avoid**:
- ❌ **Never** split enum variants into separate files!
- ❌ Don't split generated code
- ❌ Don't split test helper utilities

See `analyzers/file-decomposition-analyzer.md` for complete decision matrix and exception patterns.

## Prerequisites

Before starting, ensure:
- ✅ Project compiles (`cargo check` succeeds)
- ✅ Git repository initialized
- ✅ Working directory is clean (or willing to stash changes)
- ✅ Tests are passing (or known to be broken)
- ✅ Time budget: 4-5 days for comprehensive refactoring

## Workflow Overview

The skill follows a 6-phase process:

```
┌─────────────────────────────────────────────────────────┐
│                 REFACTORING WORKFLOW                    │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  Phase 1: Exploration (~10 min)                        │
│  ├─ Analyze project structure                          │
│  ├─ Understand dependencies                            │
│  └─ Establish baseline metrics                         │
│                                                         │
│  Phase 2: Diagnosis (~15 min, parallelized) ⚡         │
│  ├─ PARALLEL: SOLID Checker                            │
│  ├─ PARALLEL: Performance Auditor                      │
│  ├─ PARALLEL: Dead Code Finder                         │
│  └─ SEQUENTIAL: DRY Analyzer (uses SOLID context)      │
│                                                         │
│  Phase 3: Assessment (~10 min, interactive) 🎯         │
│  ├─ Calculate quality scores (0-10 scale)              │
│  ├─ ASK USER: Priorities and constraints               │
│  └─ Generate prioritized issue list                    │
│                                                         │
│  Phase 4: Planning (~5 min, interactive) 📋            │
│  ├─ Create execution roadmap with dependencies         │
│  ├─ Define rollback checkpoints                        │
│  └─ ASK USER: Approve plan                             │
│                                                         │
│  Phase 5: Execution (~4-5 days) ⚙️                     │
│  ├─ Batch 1: Quick wins & cleanup                      │
│  ├─ Batch 2: Performance algorithms                    │
│  ├─ Batch 3: Architectural refactoring                 │
│  ├─ Batch 4: Memory optimization                       │
│  ├─ Batch 5: DRY consolidation                         │
│  └─ Batch 6: Polish & documentation                    │
│                                                         │
│  Phase 6: Verification (~1 hour) ✅                     │
│  ├─ Run comprehensive tests                            │
│  ├─ Validate performance improvements                  │
│  ├─ Recalculate quality scores                         │
│  └─ ASK USER: Final sign-off                           │
│                                                         │
└─────────────────────────────────────────────────────────┘

Total Duration: ~4-5 days
Speedup: 40-50% via parallelization
```

## Detailed Phase Descriptions

### Phase 1: Exploration

**Goal**: Understand project structure and establish baseline.

**Actions**:
- Read Cargo.toml for dependencies and features
- Map directory structure (find, ls)
- Count LOC, functions, structs, traits
- Identify large files (>500 LOC)
- Run cargo check and clippy for baseline warnings
- Assess test coverage

**Output**: Project summary with metrics and hotspots

**See**: `phases/1-exploration.md` for details

---

### Phase 2: Diagnosis (PARALLELIZED)

**Goal**: Identify all code quality issues using specialized analyzers.

**Parallel Execution Strategy**:
```
3 CONCURRENT ANALYZERS (independent):
├─ SOLID Checker (Architecture)
├─ Performance Auditor (Speed & Memory)
└─ Dead Code Finder (Unused code)
      ↓ (wait for completion)
1 SEQUENTIAL ANALYZER (context-dependent):
└─ DRY Analyzer (Duplication)
```

**Time Savings**: ~12-15 minutes (vs ~20-25 sequential)

**Analyzers**:

1. **SOLID Principles Checker** (`analyzers/solid-checker.md`)
   - Single Responsibility violations
   - Open/Closed violations
   - Liskov Substitution violations
   - Interface Segregation violations
   - Dependency Inversion violations
   - Output: JSON with violations

2. **Performance Auditor** (`analyzers/performance-auditor.md`)
   - Unnecessary allocations (clone, to_string)
   - Algorithmic inefficiency (O(n²) loops)
   - Suboptimal collections
   - Inefficient string operations
   - Premature optimization
   - Output: JSON with issues and speedup estimates

3. **Dead Code Finder** (`analyzers/dead-code-finder.md`)
   - Unused imports, functions, variables
   - Unused types, constants
   - Unreachable code
   - Orphaned test code
   - Output: JSON with removable items

4. **DRY Analyzer** (`analyzers/dry-analyzer.md`)
   - Exact code duplication
   - Structural duplication
   - Similar match patterns
   - Repeated error handling
   - Also identifies KISS violations
   - Output: JSON with abstraction opportunities

5. **File Decomposition Analyzer** (`analyzers/file-decomposition-analyzer.md`)
   - Large files (>800 LOC) analysis
   - Multi-type files (violating "1 file = 1 structure")
   - 5 decomposition strategies (logical grouping, domain boundaries, abstraction levels, trait extraction, single type per file)
   - Exception pattern recognition (Error+ErrorKind, Builder, DTO families, etc.)
   - Output: JSON with decomposition opportunities and proposed structure

**Output**: Consolidated diagnosis with 100-150 issues, module heatmap, decomposition plan

**See**: `phases/2-diagnosis.md` for details

---

### Phase 3: Assessment (INTERACTIVE)

**Goal**: Score project quality and prioritize issues with user input.

**Scoring Dimensions** (0-10 scale):
- **Architecture**: SOLID compliance, module organization
- **Code Quality**: Dead code, duplication, error handling
- **Performance**: Algorithm efficiency, memory usage
- **Maintainability**: Test coverage, complexity, docs

**User Interaction**:
Uses `AskUserQuestion` tool to gather:
1. Primary goal (architecture / performance / cleanup / balanced)
2. Time budget (1-2 days / 3-5 days / 1-2 weeks / flexible)
3. Risk tolerance (conservative / moderate / aggressive)
4. Focus areas (multi-select: performance-critical / user-facing / architecture / i18n / testing)

**Output**: Prioritized issue list tailored to user preferences

**See**: `phases/3-assessment.md` for details

---

### Phase 4: Planning (INTERACTIVE)

**Goal**: Create detailed execution roadmap.

**Actions**:
- Map task dependencies
- Group into logical batches (6 batches over 4-5 days)
- Define rollback checkpoints
- Estimate effort and risk for each batch
- Create risk mitigation plans

**User Interaction**:
Uses `AskUserQuestion` to get approval on:
- Execution roadmap
- High-risk items inclusion
- Batch-specific details if requested

**Output**: Approved execution plan with timeline

**See**: `phases/4-planning.md` for details

---

### Phase 5: Execution

**Goal**: Implement refactoring batch-by-batch with validation.

**Execution Pattern** (for each batch):
```
1. Checkpoint    → git commit, cargo test
2. Implement     → Make code changes (Edit tool)
3. Test          → cargo test + clippy
4. Validate      → Manual checks if needed
5. Commit        → git commit
   ↓
If any step fails → Rollback to checkpoint
```

**6 Batches**:
1. **Quick Wins** (3h): Remove dead code, fix imports, extract constants
2. **Performance Algorithms** (6h): Fix O(n²), optimize hot paths
3. **Architectural** (8h): SOLID improvements, trait extraction ⚠️ High risk
4. **Memory** (6h): Reduce allocations, optimize buffers
5. **DRY** (5h): Consolidate duplications, extract helpers
6. **Polish** (4h): Docs, naming, cleanup

**Safety Features**:
- Git checkpoints between batches
- Tests run after every change
- Rollback procedures documented
- Incremental progress tracking

**See**: `phases/5-execution.md` for details

---

### Phase 6: Verification (INTERACTIVE)

**Goal**: Validate all improvements and get user sign-off.

**Verification Steps**:
1. Run full test suite (unit + integration + doc tests)
2. Run clippy with strict settings
3. Check compilation and build
4. Run performance benchmarks
5. Compare metrics to baseline
6. Recalculate quality scores

**Expected Results**:
- Tests: All passing
- Performance: 50-100x improvements in hotspots
- Score: +1.5 to +2.5 point improvement
- Grade: Typically C+ → A-

**User Interaction**:
Uses `AskUserQuestion` to get final decision:
- Merge to main
- Create pull request
- Review specific changes
- Address deferred issues
- Generate documentation

**See**: `phases/6-verification.md` for details

---

## Tools Required

This skill uses the following Claude Code tools:

- **Read**: Read source files, configuration
- **Edit**: Make code changes
- **Write**: Create new files (traits, modules)
- **Bash**: Run cargo commands, git, grep, find, wc
- **Grep**: Search code patterns
- **Glob**: Find files by pattern
- **Task**: Launch parallel analyzer subagents
- **AskUserQuestion**: Interactive priority gathering

## State Management

The skill maintains state across phases:

**Phase 1 → Phase 2**:
- Baseline metrics (LOC, warnings, file sizes)
- Hotspot files (>500 LOC)
- Module structure

**Phase 2 → Phase 3**:
- All analyzer JSON outputs
- Module heatmap with issue counts
- Cross-references between issues

**Phase 3 → Phase 4**:
- User priority selections
- Quality scores
- Prioritized issue list

**Phase 4 → Phase 5**:
- Execution roadmap (batches, tasks)
- Risk mitigation plans
- Checkpoint strategy

**Phase 5 → Phase 6**:
- Git commit history
- Changes made per batch
- Test results

## Example Invocation

**User**: `/rust-refactor`

**System**:
```
🚀 Rust Refactoring Skill - Starting...

Analyzing termide project...

Phase 1: Exploration ━━━━━━━━━━━━━━━━━━ 100% (8 minutes)
├─ 14,890 LOC across 32 files
├─ 12 top-level modules
├─ 34 clippy warnings
└─ 3 hotspot files identified

Phase 2: Diagnosis ━━━━━━━━━━━━━━━━━━ 100% (12 minutes)
├─ [✓] SOLID Checker (42 violations)
├─ [✓] Performance Auditor (38 issues)
├─ [✓] Dead Code Finder (45 items)
└─ [✓] DRY Analyzer (23 duplications)

Total issues: 148

Phase 3: Assessment...
Project score: 6.5 / 10 (Fair)

[Interactive questions about priorities]
...

Phase 4: Planning...
Created 6-batch roadmap over 4.5 days.

[Ask user approval]
...

Phase 5: Execution...
[Batch-by-batch implementation with progress updates]
...

Phase 6: Verification...
Final score: 8.6 / 10 (Excellent) ↑ +2.1

All tests passing ✓
Performance: 100x improvement ✓

[Ask user for final decision]
```

## Error Handling

### If Project Doesn't Compile

```
❌ ERROR: Project does not compile

Please fix compilation errors first:
cargo check

Then re-run /rust-refactor
```

### If Tests Fail During Execution

```
⚠️  TEST FAILURE in Batch 3

Failed test: file_manager::test_duplicate_detection

Options:
1. Rollback to checkpoint (git reset)
2. Investigate and fix
3. Skip this batch

What would you like to do?
```

### If User Cancels Mid-Execution

```
🛑 Refactoring cancelled at Batch 3

Current state:
- Batch 1-2: ✓ Complete and committed
- Batch 3: Partial (not committed)

Rollback options:
1. Keep Batch 1-2 changes
2. Rollback everything (git reset --hard baseline)

All changes are in branch: refactor-code-quality
```

## Success Criteria

A successful refactoring session achieves:

- ✅ All critical and high-priority issues resolved (95%+)
- ✅ Project score improvement of +1.5 to +2.5 points
- ✅ No test regressions (all tests still passing)
- ✅ Performance improvements verified (50-100x in hotspots)
- ✅ Code quality metrics improved (fewer warnings, less duplication)
- ✅ User approval obtained for merging

## Maintenance

To update this skill:

1. **Add new analyzers**: Create file in `analyzers/` with JSON output format
2. **Modify phases**: Edit individual phase files in `phases/`
3. **Adjust scoring**: Update formulas in Phase 3
4. **Change batching**: Modify Phase 4 batch definitions

## Implementation Notes

- **No report files**: Do NOT create any report files (REFACTORING_REPORT.md, DIAGNOSIS_REPORT.md, etc.). All reports and analysis must be displayed directly in the terminal only
- **Parallel execution**: Phase 2 launches 3 concurrent Task tools
- **State persistence**: Uses JSON for structured data between phases (terminal display only, not saved to files)
- **Interactive checkpoints**: AskUserQuestion at Phases 3, 4, 6
- **Safety-first**: Every change validated before proceeding
- **Incremental commits**: Small, atomic commits with rollback capability

## Limitations

- **Time intensive**: Requires 4-5 days for full execution
- **Rust-specific**: Only works with Rust/Cargo projects
- **Manual validation**: Some architectural decisions require human judgment
- **Test dependency**: Relies on existing test suite for validation

## Related Skills

- `release-management`: For managing releases after refactoring
- Consider creating: `test-coverage`, `documentation-audit`, `dependency-audit`

---

## Getting Started

Simply invoke:
```
/rust-refactor
```

Or for custom scope:
```
/rust-refactor focus:performance budget:3-days
```

The skill will guide you through the entire process with interactive prompts at key decision points.

---

**Version**: 1.0.0
**Last Updated**: 2025-12-05
**Maintainer**: Claude Code Rust Refactoring Team
