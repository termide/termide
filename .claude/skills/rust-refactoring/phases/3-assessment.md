# Phase 3: Assessment (Scoring & Prioritization)

**Goal**: Generate project quality scores, prioritize issues, and get user input on refactoring priorities.

**Duration**: ~5-10 minutes (includes user interaction)

**Key Feature**: **Interactive** - uses AskUserQuestion tool to gather priorities

## Objectives

1. Calculate project quality scores (0-10 scale)
2. Categorize issues by severity and impact
3. Estimate refactoring effort
4. **INTERACT**: Ask user about priorities and constraints
5. Generate prioritized issue list based on user input

## Step-by-Step Process

### Step 3.1: Calculate Quality Scores

Using diagnosis results from Phase 2, calculate scores for each dimension:

#### Architecture Score (0-10)

**Factors**:
- SOLID violations: -0.2 per critical, -0.1 per high, -0.05 per medium
- Module organization: +2 if well-structured
- Dependency management: +1 if minimal circular deps
- Base score: 10

**Calculation**:
```
Architecture Score = 10
  - (SOLID_critical × 0.2)
  - (SOLID_high × 0.1)
  - (SOLID_medium × 0.05)
  + module_structure_bonus
  + dependency_bonus
```

**Example**: 42 SOLID violations (3 critical, 12 high, 18 medium, 9 low)
```
Score = 10 - (3×0.2) - (12×0.1) - (18×0.05) + 0 + 0
      = 10 - 0.6 - 1.2 - 0.9
      = 7.3 / 10
```

#### Code Quality Score (0-10)

**Factors**:
- Dead code items: -0.1 per 10 items
- DRY violations: -0.15 per significant duplication
- Error handling: +2 if proper Result/Option usage
- Naming conventions: +1 if consistent
- Documentation: +1 if comprehensive

**Calculation**:
```
Code Quality = 10
  - (dead_code_items / 10 × 0.1)
  - (DRY_violations × 0.15)
  + error_handling_bonus
  + naming_bonus
  + docs_bonus
```

#### Performance Score (0-10)

**Factors**:
- Critical performance issues: -1.0 each
- High impact issues: -0.3 each
- Medium impact issues: -0.1 each
- Hot path optimization: +1 if well-optimized

**Calculation**:
```
Performance = 10
  - (perf_critical × 1.0)
  - (perf_high × 0.3)
  - (perf_medium × 0.1)
  + hot_path_bonus
```

#### Maintainability Score (0-10)

**Factors**:
- Test coverage: 0-3 points (0% = 0, 100% = 3)
- Average file size: +2 if <500 LOC
- Cyclomatic complexity: +2 if low
- Documentation quality: 0-3 points

**Calculation**:
```
Maintainability = test_coverage_score
                + file_size_bonus
                + complexity_bonus
                + documentation_score
```

### Step 3.2: Generate Project Report Card

Present scores to user:

```
╔════════════════════════════════════════════════════════════╗
║         PHASE 3: ASSESSMENT - PROJECT SCORES               ║
╚════════════════════════════════════════════════════════════╝

📊 PROJECT QUALITY REPORT CARD

┌─────────────────────────────────────────────────────┐
│ Architecture                             7.3 / 10   │
│ ████████████████████████████░░░░░░░░░░░  (Good)    │
│                                                     │
│ Code Quality                             6.8 / 10   │
│ ████████████████████████░░░░░░░░░░░░░░░  (Fair)    │
│                                                     │
│ Performance                              5.2 / 10   │
│ ████████████████░░░░░░░░░░░░░░░░░░░░░░░  (Needs    │
│                                          Work)     │
│ Maintainability                          6.5 / 10   │
│ ████████████████████░░░░░░░░░░░░░░░░░░░  (Fair)    │
└─────────────────────────────────────────────────────┘

Overall Project Score: 6.5 / 10 (Fair)

INTERPRETATION:
✓ Architecture: Solid foundation, some SOLID violations to address
⚠️ Code Quality: Dead code and duplication need cleanup
⚠️ Performance: Several critical issues in hot paths - priority!
⚠️ Maintainability: Limited test coverage, large files

BENCHMARK: Average Rust project scores ~7.2/10
```

### Step 3.3: Categorize Issues by Impact vs Effort

Create impact/effort matrix:

```
IMPACT vs EFFORT MATRIX

High Impact, Low Effort (QUICK WINS):        Priority: ⭐⭐⭐
├─ Remove 45 dead code items (saves 342 LOC)
├─ Fix 12 unused imports
├─ Extract 4 duplicated helper functions
└─ Fix 3 algorithmic O(n²) loops
   Estimated time: 1-2 days

High Impact, High Effort (MAJOR REFACTORS):  Priority: ⭐⭐
├─ Refactor FileManager for SRP (8 violations)
├─ Optimize Buffer operations (12 perf issues)
└─ Restructure error handling (15 locations)
   Estimated time: 5-7 days

Low Impact, Low Effort (NICE TO HAVE):       Priority: ⭐
├─ Improve naming in 8 functions
├─ Add documentation to 12 public APIs
└─ Extract 3 small constants
   Estimated time: 0.5-1 day

Low Impact, High Effort (AVOID):             Priority: ❌
├─ None identified (good!)
```

### Step 3.4: INTERACTIVE - Ask User Priorities

**Use AskUserQuestion tool** to gather user preferences:

#### Question 1: Primary Goal

```
What is your primary goal for this refactoring session?
```

**Options**:
1. **Ship quickly** - Focus only on critical bugs and blockers
2. **Improve architecture** - Address SOLID violations and structure
3. **Boost performance** - Fix performance bottlenecks first
4. **Clean codebase** - Remove dead code, duplication, improve maintainability
5. **Balanced approach** - Mix of quick wins and important issues

**Store answer as**: `user_priority_goal`

#### Question 2: Time Budget

```
How much time can you dedicate to refactoring?
```

**Options**:
1. **1-2 days** - Quick wins only
2. **3-5 days** - Quick wins + some high-impact issues
3. **1-2 weeks** - Comprehensive refactoring (all high priority)
4. **Flexible** - Let the assessment guide the timeline

**Store answer as**: `user_time_budget`

#### Question 3: Risk Tolerance

```
What's your tolerance for breaking changes during refactoring?
```

**Options**:
1. **Very conservative** - Only safe, non-breaking changes
2. **Moderate** - Breaking changes OK if tests pass
3. **Aggressive** - Major restructuring acceptable if it improves design

**Store answer as**: `user_risk_tolerance`

#### Question 4: Specific Focus Areas

```
Are there specific modules or issues you want to prioritize?
(Select all that apply)
```

**Options (multi-select)**:
1. Performance-critical code (editor, rendering)
2. User-facing panels (file manager, terminal)
3. Core architecture (traits, error handling)
4. Internationalization (i18n modules)
5. Testing infrastructure
6. No preference - follow recommendations

**Store answer as**: `user_focus_areas` (array)

### Step 3.5: Generate Prioritized Issue List

Based on user responses, reorder issues:

**Priority Algorithm**:
```
priority_score = base_severity_score
               + goal_alignment_bonus
               + time_budget_filter
               + risk_level_adjustment
               + focus_area_bonus
```

**Example output**:

```
╔════════════════════════════════════════════════════════════╗
║   PRIORITIZED REFACTORING PLAN (based on your input)      ║
╚════════════════════════════════════════════════════════════╝

Your priorities:
✓ Goal: Boost performance
✓ Time: 3-5 days
✓ Risk: Moderate
✓ Focus: Performance-critical code, User-facing panels

RECOMMENDED EXECUTION ORDER:

🔥 CRITICAL (Must fix - Day 1-2):
   1. [Performance] O(n²) loop in file_manager::find_duplicates
      Impact: 100x speedup for large directories
      Effort: 2 hours
      Risk: Low

   2. [Performance] Buffer cloning on every keystroke in editor
      Impact: Removes lag in large files
      Effort: 4 hours
      Risk: Medium (requires careful testing)

   3. [SOLID/SRP] FileManager handles UI + filesystem
      Impact: Easier testing, better separation
      Effort: 1 day
      Risk: Medium

⚠️  HIGH (Should fix - Day 3-4):
   4. [Dead Code] Remove 45 unused items (saves 342 LOC)
      Impact: Cleaner codebase, faster compilation
      Effort: 3 hours
      Risk: Very low

   5. [Performance] Excessive string allocations in logger
      Impact: 30% memory reduction
      Effort: 2 hours
      Risk: Low

   ... (10 more high priority items)

📝 MEDIUM (If time permits - Day 5):
   ... (20 items)

ℹ️  LOW (Future improvements):
   ... (15 items)

DEFERRED (Not in scope based on your priorities):
   - Low-impact architectural changes
   - Non-performance related DRY violations in i18n
   - Documentation improvements

Total estimated effort: 4.5 days (fits your 3-5 day budget)
```

### Step 3.6: Generate Assessment Summary

Create JSON summary for next phase:

```json
{
  "scores": {
    "architecture": 7.3,
    "code_quality": 6.8,
    "performance": 5.2,
    "maintainability": 6.5,
    "overall": 6.5
  },
  "user_priorities": {
    "goal": "boost_performance",
    "time_budget": "3-5_days",
    "risk_tolerance": "moderate",
    "focus_areas": ["performance_critical", "user_facing_panels"]
  },
  "prioritized_issues": [
    {
      "rank": 1,
      "category": "performance",
      "severity": "critical",
      "location": "src/panels/file_manager.rs:456",
      "issue": "O(n²) loop in find_duplicates",
      "effort_hours": 2,
      "impact": "100x speedup",
      "risk": "low"
    },
    ...
  ],
  "execution_plan_preview": {
    "critical_count": 3,
    "high_count": 12,
    "total_estimated_days": 4.5,
    "fits_budget": true
  }
}
```

## Phase 3 Output

Present final assessment to user:

```
╔════════════════════════════════════════════════════════════╗
║         PHASE 3: ASSESSMENT - COMPLETE ✓                   ║
╚════════════════════════════════════════════════════════════╝

📊 PROJECT SCORE: 6.5 / 10 (Fair)

🎯 YOUR PRIORITIES:
   Focus: Performance improvements
   Budget: 3-5 days
   Risk: Moderate changes OK

✅ PLAN GENERATED:
   Critical issues: 3 (must fix)
   High priority: 12 (should fix)
   Estimated time: 4.5 days (✓ fits budget)

🏆 EXPECTED IMPROVEMENT:
   Performance score: 5.2 → 8.1 (+2.9)
   Overall score: 6.5 → 7.8 (+1.3)

TOP 3 ITEMS:
   1. Fix O(n²) file search (2h, 100x faster)
   2. Remove buffer cloning (4h, eliminates lag)
   3. Refactor FileManager SRP (1d, better architecture)

🎯 NEXT PHASE: Planning (Detailed Roadmap)
   Will create step-by-step execution plan with dependencies.

Ready to proceed? (automatically continuing...)
```

## Tools Used

- **AskUserQuestion**: Interactive priority gathering
- **Bash**: Calculations, sorting
- **Read**: Access diagnosis results from Phase 2

## Success Criteria

- [x] Quality scores calculated for all dimensions
- [x] Impact vs Effort matrix created
- [x] User priorities gathered interactively
- [x] Issues prioritized based on user input
- [x] Execution order determined
- [x] Estimated timeline generated
- [x] Expected improvement calculated

## State to Carry Forward

Store for Phase 4 (Planning):
- User priority selections
- Prioritized issue list (ranked)
- Estimated effort for each issue
- Risk levels
- Expected score improvements
- Time budget constraints

---

**Proceed to Phase 4: Planning**
