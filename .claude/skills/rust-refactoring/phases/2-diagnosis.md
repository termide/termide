# Phase 2: Diagnosis (Parallel Analysis)

**Goal**: Identify all code quality issues, architectural problems, and refactoring opportunities using parallel specialized analyzers.

**Duration**: ~10-20 minutes (parallelized)

**Parallelization**: This phase launches 4 independent analyzers concurrently, then runs DRY analyzer with their context.

## Parallel Execution Strategy

This phase uses **parallel execution** to speed up analysis:

```
┌──────────────────────────────────────────────────────────────┐
│  PARALLEL BLOCK 1 (4 concurrent analyzers)                 │
├──────────────────────────────────────────────────────────────┤
│  ┌────────────┐ ┌──────────┐ ┌─────────┐ ┌─────────────┐  │
│  │   SOLID    │ │Performance│ │  Dead   │ │    File     │  │
│  │  Checker   │ │  Auditor  │ │  Code   │ │Decomposition│  │
│  │(Architecture)│(Speed &   │ │ Finder  │ │  Analyzer   │  │
│  │            │ │ Memory)   │ │         │ │ (Modularity)│  │
│  │  ~5-8 min  │ │ ~4-6 min  │ │ ~3 min  │ │  ~2-4 min   │  │
│  └────────────┘ └──────────┘ └─────────┘ └─────────────┘  │
└──────────────────────────────────────────────────────────────┘
                         ↓
┌─────────────────────────────────────────────────────┐
│  SEQUENTIAL BLOCK (uses results from parallel)    │
├─────────────────────────────────────────────────────┤
│  ┌────────────────────────────────────┐            │
│  │ DRY Analyzer                        │            │
│  │ (Uses SOLID context for better     │            │
│  │  abstraction suggestions)           │            │
│  │ ~4-5 min                            │            │
│  └────────────────────────────────────┘            │
└─────────────────────────────────────────────────────┘
                         ↓
               Consolidate Results
```

**Total time**: ~12-15 minutes (vs ~25-30 minutes sequential)
**Speedup**: ~50% faster

## Step-by-Step Process

### Step 2.1: Launch Parallel Analyzers

**IMPORTANT**: These 4 analyzers are INDEPENDENT and should execute CONCURRENTLY.

#### Analyzer 1: SOLID Principles Checker (PARALLEL)

**Task**: Analyze architectural quality against SOLID principles

**Scope**: All public structs, traits, implementations

**Analyzer location**: `../analyzers/solid-checker.md`

**Expected output**: JSON with SOLID violations

**Invocation**:
```
Use Task tool to spawn subagent with ../analyzers/solid-checker.md prompt.
Pass context: Project path, list of main modules from Phase 1.
```

**What it analyzes**:
- Single Responsibility Principle violations
- Open/Closed Principle violations
- Liskov Substitution Principle violations
- Interface Segregation Principle violations
- Dependency Inversion Principle violations

**Example findings**:
```json
{
  "total_violations": 42,
  "by_principle": {"SRP": 15, "OCP": 8, "LSP": 3, "ISP": 12, "DIP": 4},
  "violations": [...]
}
```

---

#### Analyzer 2: Performance Auditor (PARALLEL)

**Task**: Find performance bottlenecks and inefficiencies

**Scope**: Entire codebase, focus on hot paths identified in Phase 1

**Analyzer location**: `../analyzers/performance-auditor.md`

**Expected output**: JSON with performance issues

**Invocation**:
```
Use Task tool to spawn subagent with ../analyzers/performance-auditor.md prompt.
Pass context: Largest files from Phase 1, event handlers, loops.
```

**What it analyzes**:
- Unnecessary allocations (clone, to_string, Box::new)
- Algorithmic inefficiency (nested loops, O(n²) patterns)
- Suboptimal collection usage
- Inefficient string operations
- Premature optimization

**Example findings**:
```json
{
  "total_issues": 38,
  "by_impact": {"critical": 3, "high": 10, "medium": 15, "low": 10},
  "issues": [...]
}
```

---

#### Analyzer 3: Dead Code Finder (PARALLEL)

**Task**: Identify unused, unreachable, and orphaned code

**Scope**: Complete codebase including tests

**Analyzer location**: `../analyzers/dead-code-finder.md`

**Expected output**: JSON with dead code items

**Invocation**:
```
Use Task tool to spawn subagent with ../analyzers/dead-code-finder.md prompt.
Pass context: cargo check/clippy warnings from Phase 1.
```

**What it analyzes**:
- Unused imports
- Unused functions (private and public)
- Unused variables and parameters
- Unused types (structs, enums, type aliases)
- Unused constants and statics
- Unreachable code
- Orphaned test code

**Example findings**:
```json
{
  "total_dead_code_items": 45,
  "by_category": {"unused_imports": 12, "unused_functions": 8, ...},
  "total_lines_removable": 342,
  "items": [...]
}
```

---

#### Analyzer 4: File Decomposition Analyzer (PARALLEL)

**Task**: Identify large files that can benefit from decomposition into smaller modules

**Scope**: All Rust files, focusing on files >800 LOC

**Analyzer location**: `../analyzers/file-decomposition-analyzer.md`

**Expected output**: JSON with decomposition opportunities

**Invocation**:
```
Use Task tool to spawn subagent with ../analyzers/file-decomposition-analyzer.md prompt.
Pass context: Large files from Phase 1, module structure information.
```

**What it analyzes**:
- Files exceeding 1000 LOC threshold
- Logical grouping opportunities (related functions/structs)
- Domain boundary separation (User/Auth/Storage logic)
- Abstraction level separation (high-level API vs low-level impl)
- Large impl block extraction (trait + impl decomposition)

**Example findings**:
```json
{
  "total_large_files": 8,
  "decomposition_opportunities": 5,
  "total_lines_saved": 3200,
  "opportunities": [
    {
      "file": "src/editor/mod.rs",
      "current_lines": 1847,
      "severity": "high",
      "proposed_structure": {
        "mod.rs": {"lines": 200},
        "cursor.rs": {"lines": 400},
        "selection.rs": {"lines": 350},
        "render.rs": {"lines": 450},
        "history.rs": {"lines": 450}
      }
    }
  ]
}
```

---

### Step 2.2: Wait for Parallel Analyzers to Complete

**Display progress to user**:
```
🔄 PARALLEL ANALYSIS IN PROGRESS

   [✓] SOLID Checker              - Complete (42 violations found)
   [✓] Performance Auditor        - Complete (38 issues found)
   [✓] Dead Code Finder           - Complete (45 items found)
   [✓] File Decomposition Analyzer - Complete (5 opportunities found)

⏱️  Total time: 8 minutes (vs 20 minutes sequential)
📊 Issues detected: 130 total

Proceeding to DRY analysis...
```

---

### Step 2.3: Run DRY Analyzer (SEQUENTIAL)

**Why sequential?** DRY analyzer benefits from knowing SOLID violations to suggest better abstractions.

**Task**: Find code duplication and abstraction opportunities

**Scope**: Entire codebase, with special focus on:
- Modules flagged for SRP violations (likely have duplication)
- Files with OCP violations (repeated match statements)
- Areas with similar performance patterns

**Analyzer location**: `../analyzers/dry-analyzer.md`

**Expected output**: JSON with duplication findings

**Invocation**:
```
Use Task tool to spawn subagent with ../analyzers/dry-analyzer.md prompt.
Pass context:
- SOLID violation locations (from Analyzer 1)
- Large files from Phase 1
- Repeated patterns from Performance Auditor
```

**What it analyzes**:
- Exact code duplication
- Structural duplication (similar functions)
- Similar match patterns
- Repeated error handling
- Duplicated trait implementations
- Configuration/constants duplication
- Test code duplication

**Also identifies**: KISS principle violations (over-abstraction)

**Example findings**:
```json
{
  "total_duplications": 23,
  "by_type": {"exact_duplication": 8, "structural_duplication": 7, ...},
  "potential_loc_reduction": 320,
  "issues": [...]
}
```

---

### Step 2.4: Consolidate All Results

**Combine findings from all 5 analyzers**:

```
╔════════════════════════════════════════════════════════════╗
║         PHASE 2: DIAGNOSIS - RESULTS                       ║
╚════════════════════════════════════════════════════════════╝

📊 OVERALL SUMMARY

Total Issues Found: 153
├─ SOLID violations: 42 (Architecture)
├─ Performance issues: 38 (Speed & Memory)
├─ Dead code items: 45 (Maintainability)
├─ File decomposition opportunities: 5 (Modularity)
└─ DRY violations: 23 (Duplication)

🔥 CRITICAL ISSUES: 12
⚠️  HIGH PRIORITY: 34
📝 MEDIUM PRIORITY: 58
ℹ️  LOW PRIORITY: 44
```

**Cross-reference findings**:
- Match dead code locations with SOLID violations (often related)
- Link performance issues to DRY violations (duplicated inefficient code)
- Identify modules with multiple problem types (refactoring priorities)
- Connect large files with SOLID/SRP violations (often both present)

**Create issue matrix**:
```
MODULE HEATMAP (issues per module):

src/editor/mod.rs:
  ├─ SOLID: 6 violations (SRP: 4, ISP: 2)
  ├─ Performance: 4 issues (1 high, 3 medium)
  ├─ Dead code: 5 items
  ├─ File decomposition: 1 opportunity (1847 LOC → 5 files)
  └─ DRY: 3 duplications
  TOTAL: 19 issues ⚠️  HIGH REFACTORING PRIORITY (large + violations)

src/panels/file_manager.rs:
  ├─ SOLID: 8 violations (SRP: 3, ISP: 5)
  ├─ Performance: 5 issues (2 critical, 3 high)
  ├─ Dead code: 7 items
  ├─ File decomposition: 1 opportunity (1234 LOC → 4 files)
  └─ DRY: 4 duplications
  TOTAL: 25 issues ⚠️  HIGH REFACTORING PRIORITY

src/terminal/renderer.rs:
  ├─ SOLID: 3 violations (SRP: 2, DIP: 1)
  ├─ Performance: 12 issues (1 critical, 8 high)
  ├─ Dead code: 2 items
  ├─ File decomposition: No (cohesive despite size)
  └─ DRY: 1 duplication
  TOTAL: 18 issues ⚠️  PERFORMANCE CRITICAL

src/i18n/:
  ├─ SOLID: 1 violation
  ├─ Performance: 0 issues
  ├─ Dead code: 3 items
  ├─ File decomposition: 0 opportunities
  └─ DRY: 9 duplications (mostly translation helpers)
  TOTAL: 13 issues 📝 MEDIUM PRIORITY
```

### Step 2.5: Generate Unified Diagnosis Report

**Create comprehensive JSON**:
```json
{
  "diagnosis_summary": {
    "total_issues": 153,
    "by_severity": {
      "critical": 12,
      "high": 36,
      "medium": 60,
      "low": 45
    },
    "by_category": {
      "architecture": 42,
      "performance": 38,
      "dead_code": 45,
      "file_decomposition": 5,
      "duplication": 23
    },
    "estimated_refactoring_effort": "3-5 days for critical+high, 2-3 days for medium, 10-15 hours for decompositions"
  },
  "hotspot_modules": [
    {
      "path": "src/panels/file_manager.rs",
      "total_issues": 25,
      "priority": "high",
      "reason": "Multiple SOLID violations + performance issues + large file (1234 LOC)"
    },
    {
      "path": "src/editor/mod.rs",
      "total_issues": 19,
      "priority": "high",
      "reason": "Large file (1847 LOC) with SRP violations, needs decomposition"
    },
    {
      "path": "src/terminal/renderer.rs",
      "total_issues": 18,
      "priority": "critical",
      "reason": "Performance-critical code with significant issues"
    }
  ],
  "analyzer_results": {
    "solid": { ... },
    "performance": { ... },
    "dead_code": { ... },
    "file_decomposition": { ... },
    "dry": { ... }
  }
}
```

## Phase 2 Output

Present consolidated findings to user:

```
╔════════════════════════════════════════════════════════════╗
║         PHASE 2: DIAGNOSIS - COMPLETE ✓                    ║
╚════════════════════════════════════════════════════════════╝

📊 ANALYSIS COMPLETE

⏱️  Time: 12 minutes (50% faster via parallelization)
🔍 Analyzed: 14,890 lines across 32 files
🎯 Issues found: 153 total

BREAKDOWN BY SEVERITY:
🔥 12 Critical  - Must fix before release
⚠️  36 High     - Should fix soon (includes 2 large file decompositions)
📝 60 Medium    - Good to address
ℹ️  45 Low      - Nice to have

BREAKDOWN BY CATEGORY:
🏗️  42 Architecture (SOLID violations)
⚡ 38 Performance (Speed & Memory)
🗑️  45 Dead Code (Maintainability)
📦 5 File Decomposition (Modularity - 3200 LOC can be better organized)
🔁 23 Duplication (DRY violations)

TOP REFACTORING PRIORITIES:
1. src/panels/file_manager.rs (25 issues - large + violations)
2. src/editor/mod.rs (19 issues - 1847 LOC needs decomposition!)
3. src/terminal/renderer.rs (18 issues - performance critical!)

QUICK WINS (low effort, high impact):
- Remove 45 dead code items (saves 342 LOC)
- Fix 12 unused imports (immediate cleanup)
- Extract 8 duplicated functions (DRY improvements)
- Decompose src/panels/file_manager.rs (low complexity, 2-3 hours)

🎯 NEXT PHASE: Assessment (Scoring & Prioritization)
   Will generate project score and ask for your priorities.

Ready to proceed? (automatically continuing...)
```

## Tools Used

- **Task**: Launch parallel subagent analyzers
- **Bash**: Coordinate execution, track progress
- **Read**: Access analyzer results
- **Grep**: Cross-reference findings

## Success Criteria

- [x] All 5 analyzers completed successfully (4 parallel + 1 sequential)
- [x] Results consolidated into unified report
- [x] Module heatmap generated (including decomposition opportunities)
- [x] Quick wins identified
- [x] Refactoring priorities established
- [x] Estimated effort calculated

## State to Carry Forward

Store for Phase 3 (Assessment):
- Complete analyzer JSON outputs (SOLID, Performance, Dead Code, File Decomposition, DRY)
- Module heatmap with issue counts
- Quick wins list (including easy file decompositions)
- Hotspot modules ranking (large files prioritized)
- Cross-references between issue types (e.g., large files + SRP violations)

---

**Proceed to Phase 3: Assessment**
