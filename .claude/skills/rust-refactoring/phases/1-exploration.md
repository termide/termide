# Phase 1: Exploration

**Goal**: Understand the project structure, architecture, dependencies, and establish baseline metrics.

**Duration**: ~5-10 minutes for typical projects

## Objectives

1. Understand project metadata and dependencies
2. Map directory and module structure
3. Identify key architectural components
4. Establish baseline code metrics
5. Understand build configuration and features

## Step-by-Step Process

### Step 1.1: Analyze Project Metadata

**Read Cargo.toml to understand**:
```bash
cat Cargo.toml
```

Extract information:
- **Project name and version**: What are we analyzing?
- **Edition**: Rust 2015/2018/2021 (affects idiomatic patterns)
- **Dependencies**: Which crates are used? (analyze for bloat later)
- **Features**: Conditional compilation (affects dead code analysis)
- **Build configuration**: Is this a bin, lib, or both?

**Output to user**:
```
📦 Project: termide v0.3.0
🦀 Edition: Rust 2021
📚 Dependencies: 45 total
   - crossterm: Terminal handling
   - serde: Serialization
   - git2: Git integration
   ... (list key dependencies)
🎯 Build target: Binary application
⚙️  Features: 3 optional features detected
```

### Step 1.2: Map Directory Structure

**Discover source layout**:
```bash
# Get directory tree
find src -type d | sort

# Count Rust files per directory
find src -name "*.rs" -type f | xargs dirname | sort | uniq -c | sort -rn

# Get total lines of code
find src -name "*.rs" -exec wc -l {} + | tail -1
```

**Identify**:
- Main entry point (src/main.rs or src/lib.rs)
- Major modules (src/*/mod.rs or src/*.rs)
- Test organization (tests/, src/*/tests/)
- Binary vs library structure

**Output to user**:
```
📂 Source Structure:
   src/
   ├── main.rs (entry point)
   ├── panels/ (6 modules, 2,340 LOC)
   ├── editor/ (8 modules, 3,120 LOC)
   ├── terminal/ (4 modules, 1,450 LOC)
   ├── git/ (5 modules, 980 LOC)
   └── i18n/ (9 modules, 5,600 LOC)

📊 Total: 32 Rust files, 14,890 lines of code
```

### Step 1.3: Analyze Module Architecture

**Read main module files**:
```bash
# Find all mod declarations
grep -rn "^mod\|^pub mod" src/main.rs src/lib.rs --include="*.rs"

# Find public API surface
grep -rn "^pub fn\|^pub struct\|^pub enum\|^pub trait" src/ --include="*.rs" | wc -l
```

**Understand**:
- How many top-level modules?
- What's the module hierarchy depth?
- Which modules are public (library API) vs private?
- Are there circular dependencies? (check use statements)

**Output to user**:
```
🏗️  Architecture:
   - 12 top-level modules
   - Maximum depth: 3 levels
   - Public API: 145 public items
   - Module organization: Feature-based (good!)
```

### Step 1.4: Establish Code Metrics Baseline

**Collect metrics**:
```bash
# Count functions
grep -rn "fn " src/ --include="*.rs" | wc -l

# Count structs and enums
grep -rn "^struct\|^pub struct" src/ --include="*.rs" | wc -l
grep -rn "^enum\|^pub enum" src/ --include="*.rs" | wc -l

# Count impl blocks
grep -rn "^impl" src/ --include="*.rs" | wc -l

# Count traits
grep -rn "^trait\|^pub trait" src/ --include="*.rs" | wc -l

# Find largest files
find src -name "*.rs" -exec wc -l {} + | sort -rn | head -10
```

**Baseline metrics**:
- Total functions
- Total types (structs + enums)
- Total trait definitions
- Largest files (potential complexity hotspots)
- Average file size

**Output to user**:
```
📈 Code Metrics Baseline:
   Functions: 342
   Structs: 89
   Enums: 23
   Traits: 15
   Impl blocks: 156

   Largest files:
   1. src/panels/file_manager.rs (1,234 lines)
   2. src/editor/buffer.rs (987 lines)
   3. src/i18n/ru.rs (823 lines)

   Average file size: 465 lines
```

### Step 1.5: Identify Test Coverage

**Analyze test structure**:
```bash
# Count test modules
grep -rn "#\[cfg(test)\]" src/ --include="*.rs" | wc -l

# Count test functions
grep -rn "#\[test\]" src/ --include="*.rs" | wc -l

# Check for integration tests
ls tests/ 2>/dev/null && find tests -name "*.rs" | wc -l

# Check for benchmarks
ls benches/ 2>/dev/null && find benches -name "*.rs" | wc -l
```

**Output to user**:
```
🧪 Test Coverage:
   Unit test modules: 18
   Unit tests: 53
   Integration tests: 0
   Benchmarks: 0

   ⚠️  Note: Limited test coverage - will affect refactoring safety
```

### Step 1.6: Check Build Configuration

**Examine build settings**:
```bash
# Check for build.rs
test -f build.rs && echo "Custom build script detected"

# Check for feature gates
grep -n "cfg(feature" Cargo.toml src/ -r

# Check for platform-specific code
grep -rn "cfg(target" src/ --include="*.rs" | wc -l
```

**Output to user**:
```
⚙️  Build Configuration:
   Custom build script: No
   Feature flags: 3 (nix, packaging, release)
   Platform-specific code: 12 occurrences (mainly OS detection)
```

### Step 1.7: Assess Project Health

**Quick health checks**:
```bash
# Check if project builds
cargo check 2>&1 | tail -20

# Count compiler warnings
cargo check 2>&1 | grep "warning:" | wc -l

# Run clippy for lints
cargo clippy 2>&1 | grep "warning:" | wc -l
```

**Output to user**:
```
🏥 Project Health Check:
   ✅ Compiles successfully
   ⚠️  Compiler warnings: 12
   ⚠️  Clippy warnings: 34

   Common warnings:
   - unused imports (8)
   - dead code (4)
   - variable naming (3)
```

## Phase 1 Summary Output

At the end of exploration, provide comprehensive summary:

```
╔════════════════════════════════════════════════════════════╗
║         PHASE 1: EXPLORATION - COMPLETE ✓                  ║
╚════════════════════════════════════════════════════════════╝

📦 PROJECT: termide v0.3.0
🦀 Edition: Rust 2021
📊 Size: 14,890 lines across 32 files
🏗️  Architecture: 12 top-level modules, feature-based organization

KEY FINDINGS:
✓ Well-structured project with clear separation
✓ Modern Rust edition (2021)
⚠️  Large files detected (>1000 lines) - potential complexity
⚠️  Limited test coverage (53 tests for ~15k LOC)
⚠️  34 clippy warnings to address

HOTSPOTS FOR ANALYSIS:
1. src/panels/file_manager.rs (1,234 lines - complexity risk)
2. src/editor/buffer.rs (987 lines - performance critical)
3. src/i18n/ (5,600 lines - duplication potential)

🎯 NEXT PHASE: Diagnosis (Parallel Analysis)
   Will launch 3 concurrent analyzers:
   - SOLID Principles Checker
   - Performance Auditor
   - Dead Code Finder

Ready to proceed? (automatically continuing...)
```

## Tools Used

- **Bash**: cargo, find, grep, wc, cat
- **Read**: Cargo.toml, main.rs, key module files
- **Grep**: Pattern matching for code elements

## Success Criteria

- [x] Project metadata understood
- [x] Directory structure mapped
- [x] Module architecture documented
- [x] Baseline metrics established
- [x] Test coverage assessed
- [x] Build configuration checked
- [x] Project health evaluated
- [x] Hotspots identified for next phase

## State to Carry Forward

Store these for later phases:
- Total LOC count
- Number of modules/files
- Compiler + clippy warning count (baseline)
- List of largest files (>500 lines)
- List of modules with no tests
- Feature flags and conditional compilation info

---

**Proceed to Phase 2: Diagnosis**
