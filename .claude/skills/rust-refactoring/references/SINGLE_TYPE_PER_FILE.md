# Single Type Per File: Reference Guide

Comprehensive guide to applying the "1 file = 1 structure" principle in Rust codebases with intelligent exception handling.

## Table of Contents

1. [The Principle](#the-principle)
2. [Benefits](#benefits)
3. [When to Apply](#when-to-apply)
4. [Exception Patterns](#exception-patterns)
5. [Decision Matrix](#decision-matrix)
6. [Real-World Examples](#real-world-examples)
7. [Migration Strategies](#migration-strategies)
8. [Common Mistakes](#common-mistakes)

---

## The Principle

**Core Idea**: Each significant type definition (struct, enum, or trait) should ideally reside in its own dedicated file.

```rust
// ❌ BEFORE: widgets.rs (multiple types)
pub struct Button {
    label: String,
    enabled: bool,
}

impl Button { ... }

pub struct TextField {
    content: String,
    cursor: usize,
}

impl TextField { ... }

// ✅ AFTER: Proper structure
// widgets/button.rs
pub struct Button {
    label: String,
    enabled: bool,
}
impl Button { ... }

// widgets/text_field.rs
pub struct TextField {
    content: String,
    cursor: usize,
}
impl TextField { ... }

// widgets/mod.rs
pub mod button;
pub mod text_field;
pub use button::Button;
pub use text_field::TextField;
```

---

## Benefits

### 1. Improved Navigation
**Problem**: "Where is the `Editor` struct defined?"
- With multiple types per file: Search through large file, scan hundreds of lines
- With single type per file: `src/panels/editor.rs` ← immediate answer

### 2. Clear Responsibility
Each file has exactly one primary purpose - defining and implementing one type.
- Easier to understand file's role
- Reduced cognitive load when reading code
- Obvious where to add new functionality

### 3. Reduced Merge Conflicts
**Scenario**: Two developers working on different types
- Multiple types per file: Both modify same file → merge conflict
- Single type per file: Each touches different file → clean merge

### 4. Self-Documenting Architecture
The file tree itself reveals the structure:
```
panels/
├── editor/
│   ├── mod.rs
│   ├── cursor.rs       ← "Ah, cursor management is separate"
│   ├── selection.rs    ← "Selection is its own concern"
│   └── rendering.rs    ← "Rendering is isolated"
└── terminal.rs         ← "Terminal is simpler, single file"
```

### 5. Test Structure Mirrors Source
```
src/panels/editor/cursor.rs
tests/panels/editor/cursor_test.rs  ← Clear correspondence
```

---

## When to Apply

### ✅ MUST Split (High Priority)

#### Criterion 1: Multiple Large Public Types
```rust
// File: user_management.rs (800 LOC)
pub struct User { ... }        // 250 LOC (struct + impl)
pub struct UserSettings { ... } // 180 LOC
pub struct UserProfile { ... }  // 200 LOC

// Action: Split into 3 files
// - user.rs (User)
// - user_settings.rs (UserSettings)
// - user_profile.rs (UserProfile)
```
**Threshold**: 2+ public types, each >80 LOC

#### Criterion 2: File Size + Type Count
```rust
// File: config.rs (1200 LOC, 4 types)
pub struct AppConfig { ... }
pub struct ThemeConfig { ... }
pub struct KeyBindings { ... }
pub struct PluginConfig { ... }

// Action: MUST decompose
```
**Threshold**: File >1000 LOC with multiple types

#### Criterion 3: Unrelated Domains
```rust
// File: utilities.rs
pub struct Logger { ... }    // Logging concern
pub struct HttpClient { ... } // Network concern
pub struct Cache { ... }      // Caching concern

// Action: Split - these are completely different domains
```
**Threshold**: Types from different functional domains

### ⚠️ SHOULD Split (Medium Priority)

#### Criterion 4: Three or More Public Types
```rust
// File: api_models.rs
pub struct CreateRequest { ... }  // 60 LOC
pub struct UpdateRequest { ... }  // 55 LOC
pub struct Response { ... }        // 70 LOC

// Action: Consider splitting (check for DTO exception first)
```
**Threshold**: 3+ public types, regardless of size

#### Criterion 5: Evolving Independently
```rust
// File: connection.rs
pub struct Connection { ... }     // Frequently modified
pub struct ConnectionPool { ... } // Rarely changes

// Action: Split if change patterns differ significantly
```
**Threshold**: Types with different change frequencies

### ✅ CAN Group (Low Priority)

#### Criterion 6: Small Related Types
```rust
// File: colors.rs (80 LOC total)
pub struct Color { ... }      // 30 LOC
pub enum ColorSpace { ... }   // 25 LOC
pub struct Palette { ... }    // 25 LOC

// Action: Can keep together - all small, all color-related
```
**Threshold**: All types <50 LOC, same domain

---

## Exception Patterns

### Exception 1: Error + ErrorKind

**Pattern Recognition**:
```rust
// error.rs
pub enum ErrorKind {
    Io,
    Parse,
    Network,
    InvalidInput,
}

pub struct Error {
    kind: ErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error>>,
}
```

**Why Keep Together**:
- `Error` and `ErrorKind` are conceptually one unit
- Always used together: `Error::new(ErrorKind::Io, "message")`
- Splitting would require circular imports
- Standard Rust error pattern (see `std::io`)

**Detection Criteria**:
- File contains both "Error" and "ErrorKind" type names
- Total file size <200 LOC
- Types are tightly coupled (ErrorKind used in Error struct)

**Variation: Result Type Aliases**
```rust
// Also acceptable in same file
pub type Result<T> = std::result::Result<T, Error>;
```

---

### Exception 2: Builder Pattern

**Pattern Recognition**:
```rust
// config.rs
pub struct Config {
    host: String,
    port: u16,
    timeout: Duration,
}

pub struct ConfigBuilder {
    host: Option<String>,
    port: Option<u16>,
    timeout: Option<Duration>,
}

impl ConfigBuilder {
    pub fn new() -> Self { ... }
    pub fn host(mut self, host: String) -> Self { ... }
    pub fn build(self) -> Result<Config, BuildError> { ... }
}
```

**Why Keep Together**:
- Builder exists solely to construct the target type
- Tightly coupled: Builder → Config (one-way dependency)
- Common Rust idiom (see `std::process::Command`)
- Often smaller than primary type

**Detection Criteria**:
- Type name ends with "Builder"
- File contains both Builder and target type
- Builder has `build()` method returning target type

**Variation: Multiple Builders**
```rust
// If you have ConfigBuilder AND ConfigValidator,
// consider splitting: config.rs, config_builder.rs, config_validator.rs
```

---

### Exception 3: Small Private Helpers

**Pattern Recognition**:
```rust
// editor.rs
pub struct Editor {
    content: Vec<String>,
    cursor: Cursor,
    mode: Mode,
    state: State,
}

// Private helpers (<30 LOC each)
struct State {
    modified: bool,
    read_only: bool,
}

enum Mode {
    Normal,
    Insert,
    Visual,
}

struct Cursor {
    line: usize,
    col: usize,
}
```

**Why Keep Together**:
- Helpers are not public API
- Small (<30 LOC each)
- Tightly coupled to main type
- No independent use case

**When This Becomes Invalid**:
```rust
// If Cursor grows to >50 LOC:
// editor/cursor.rs (extract)
pub(crate) struct Cursor { ... }
impl Cursor { ... }  // 100 LOC of cursor logic

// editor/mod.rs
mod cursor;
use cursor::Cursor;  // Now internal module

pub struct Editor {
    cursor: Cursor,  // Still private field, but Cursor is in own file
}
```

**Detection Criteria**:
- Types are NOT pub (private or pub(crate))
- Each helper <30 LOC
- Helpers only used by main type

---

### Exception 4: DTO Families

**Pattern Recognition**:
```rust
// api/users.rs
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub email: Option<String>,
}

pub struct DeleteUserRequest {
    pub user_id: u64,
}

pub struct UserResponse {
    pub id: u64,
    pub username: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
}
```

**Why Keep Together**:
- All DTOs for same API resource (users)
- All simple data structures (<50 LOC each)
- Often used together in handler functions
- Grouping by API domain is more valuable than splitting

**When to Split**:
```rust
// If any DTO grows complex (>80 LOC):
// api/users/create_request.rs
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    // ... 20 more fields
}

impl CreateUserRequest {
    // 60 LOC of validation logic
}
```

**Detection Criteria**:
- Multiple types with Request/Response/DTO suffixes
- All types <50 LOC
- Located in api/ or dto/ directory
- Same domain (users, posts, comments, etc.)

---

### Exception 5: Typestate Pattern

**Pattern Recognition**:
```rust
// connection.rs
pub struct Connection<S> {
    socket: TcpStream,
    state: PhantomData<S>,
}

// Marker types for typestate
pub struct Connected;
pub struct Disconnected;
pub struct Authenticated;

impl Connection<Disconnected> {
    pub fn connect(self) -> Result<Connection<Connected>> { ... }
}

impl Connection<Connected> {
    pub fn authenticate(self) -> Result<Connection<Authenticated>> { ... }
    pub fn disconnect(self) -> Connection<Disconnected> { ... }
}

impl Connection<Authenticated> {
    pub fn send(&self, data: &[u8]) -> Result<()> { ... }
}
```

**Why Keep Together**:
- Marker types are zero-sized, exist only for type system
- Typestate pattern requires all states in scope
- Splitting would require complex re-exports
- States are meaningless without the main type

**Detection Criteria**:
- Generic type with PhantomData
- Multiple zero-sized marker structs
- Typestate pattern (different impl blocks for each state)

---

### Exception 6: Newtype Wrappers Collection

**Pattern Recognition**:
```rust
// ids.rs
pub struct UserId(pub u64);
pub struct SessionId(pub String);
pub struct TeamId(pub u64);
pub struct ProjectId(pub u64);

impl UserId {
    pub fn new(id: u64) -> Self { UserId(id) }
}
// Similar trivial impls for others
```

**Why Keep Together**:
- All types are simple newtypes (<10 LOC each)
- Conceptually a collection of "ID types"
- Splitting would create excessive boilerplate (4 files for 40 LOC total)
- Common pattern: grouping similar wrapper types

**When to Split**:
```rust
// If one newtype becomes complex:
// user_id.rs
pub struct UserId(u64);

impl UserId {
    pub fn validate(&self) -> Result<()> { ... }
    pub fn from_username(username: &str) -> Result<Self> { ... }
    // ... 50 LOC of logic
}

// Now UserId deserves its own file
```

**Detection Criteria**:
- All types are single-field tuple structs or similar wrappers
- Each type <10 LOC (struct + trivial impl)
- Types serve similar purpose (IDs, units, validated strings, etc.)

---

## Decision Matrix

Use this matrix to decide whether to split or keep types together:

| File LOC | Type Count | Public | Related | Action | Reason |
|----------|------------|--------|---------|--------|---------|
| <100 | 2-3 | Any | Yes | **KEEP** | Small, cohesive |
| <200 | 2 | Both pub | Yes | **KEEP** | Check exceptions |
| <200 | 2 | Both pub | No | **SPLIT** | Unrelated concerns |
| 200-400 | 2 | Both pub | Yes | **CONSIDER** | Evaluate coupling |
| 200-400 | 2 | Both pub | No | **SPLIT** | Different domains |
| >400 | 2+ | Any pub | Any | **SPLIT** | File too large |
| >1000 | 2+ | Any | Any | **MUST SPLIT** | Critical threshold |
| Any | 3+ | All pub | Yes | **SPLIT** | Check DTO exception |
| Any | 3+ | All pub | No | **SPLIT** | Multiple concerns |
| Any | 1 pub + helpers | Helpers private, <30 LOC | Yes | **KEEP** | Helper pattern |

**Exception Override**: If file matches any exception pattern (Error+ErrorKind, Builder, etc.), prefer **KEEP** regardless of size (up to ~200 LOC).

---

## Real-World Examples

### Example 1: TermIDE Editor Module (✅ Good)

**Current Structure**:
```
src/panels/editor/
├── mod.rs                  (19 LOC - re-exports)
├── core.rs              (1,634 LOC - main Editor struct)
├── config.rs               (49 LOC - EditorConfig)
├── cursor/                 (425 LOC total)
│   ├── mod.rs              (12 LOC)
│   ├── physical.rs         (91 LOC - PhysicalCursor)
│   ├── visual.rs          (300 LOC - VisualCursor)
│   └── jump.rs             (22 LOC - JumpList)
├── selection.rs           (124 LOC - Selection struct)
├── clipboard.rs           (107 LOC - clipboard operations)
└── rendering/           (1,510 LOC total)
    ├── context.rs         (122 LOC - RenderContext)
    ├── line_rendering.rs  (340 LOC - line rendering)
    └── ...
```

**Analysis**:
- ✅ **EditorConfig** in separate file (Exception 3 doesn't apply - it's public >40 LOC)
- ✅ **Cursor types** split (PhysicalCursor vs VisualCursor are different concerns)
- ✅ **Rendering** has own module with RenderContext as separate type
- ⚠️ **core.rs still 1,634 LOC** - candidate for further decomposition

**Recommendation**: Continue decomposition of core.rs, extract more types.

---

### Example 2: Hypothetical Error Module (✅ Good Exception)

```rust
// src/error.rs (120 LOC)
#[derive(Debug, Clone, Copy)]
pub enum ErrorKind {
    Io,
    Parse,
    Config,
    Network,
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    location: Location,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self { ... }
    pub fn kind(&self) -> ErrorKind { ... }
}

impl Display for Error { ... }
impl std::error::Error for Error { ... }

pub type Result<T> = std::result::Result<T, Error>;
```

**Analysis**:
- ✅ **Exception 1 applies**: Error + ErrorKind pattern
- ✅ **Size acceptable**: 120 LOC total, well under 200 LOC threshold
- ✅ **Tightly coupled**: ErrorKind is meaningless without Error

**Decision**: **KEEP TOGETHER** ✅

---

### Example 3: Bad Multi-Type File (❌ Bad)

```rust
// src/app_state.rs (650 LOC)

// User management (200 LOC)
pub struct User {
    id: UserId,
    name: String,
    // ... 15 fields
}
impl User {
    // 150 LOC of user methods
}

// Configuration (180 LOC)
pub struct AppConfig {
    theme: Theme,
    keybindings: KeyBindings,
    // ... 10 fields
}
impl AppConfig {
    // 130 LOC of config methods
}

// Logging (120 LOC)
pub struct Logger {
    level: LogLevel,
    output: Output,
}
impl Logger {
    // 100 LOC of logging logic
}

// Session (150 LOC)
pub struct Session {
    user: User,
    token: String,
}
impl Session {
    // 100 LOC of session methods
}
```

**Problems**:
- ❌ **4 unrelated public types** (user, config, logging, session)
- ❌ **Each >100 LOC** (substantial types)
- ❌ **Different domains** (authentication, configuration, logging, session management)
- ❌ **File >600 LOC** with multiple concerns
- ❌ **No exception applies**

**Solution**: Split into separate files
```
src/
├── user.rs (User)
├── config.rs (AppConfig)
├── logger.rs (Logger)
└── session.rs (Session)
```

---

### Example 4: DTO Family (✅ Good Exception)

```rust
// src/api/posts.rs (180 LOC)

#[derive(Serialize, Deserialize)]
pub struct CreatePostRequest {
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct UpdatePostRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct PostResponse {
    pub id: u64,
    pub title: String,
    pub content: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub tags: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct PostListResponse {
    pub posts: Vec<PostResponse>,
    pub total: u64,
    pub page: u32,
}
```

**Analysis**:
- ✅ **Exception 4 applies**: DTO family pattern
- ✅ **Same domain**: All post-related API types
- ✅ **All small**: Each type <50 LOC
- ✅ **Used together**: Often in same API handlers

**Decision**: **KEEP TOGETHER** ✅

---

## Migration Strategies

### Strategy 1: Extract Single Type

For simple cases with 2-3 types:

```bash
# 1. Create new file
touch src/user_settings.rs

# 2. Copy UserSettings type and impl to new file
# (manually or with editor)

# 3. Add pub mod in parent
# In src/lib.rs or src/user_management/mod.rs:
pub mod user_settings;

# 4. Update imports
# Change: use crate::user_management::UserSettings;
# To: use crate::user_settings::UserSettings;

# 5. Remove old code from original file

# 6. Verify
cargo check
cargo test
```

### Strategy 2: Create Subdirectory Module

For multiple related types (3+ types):

```bash
# 1. Create module directory
mkdir -p src/user/

# 2. Create mod.rs
cat > src/user/mod.rs << 'EOF'
pub mod user;
pub mod user_settings;
pub mod user_profile;

pub use user::User;
pub use user_settings::UserSettings;
pub use user_profile::UserProfile;
EOF

# 3. Extract each type to its own file
# src/user/user.rs
# src/user/user_settings.rs
# src/user/user_profile.rs

# 4. Update parent module
# In src/lib.rs:
pub mod user;

# 5. Update imports throughout codebase
# Old: use crate::user_management::{User, UserSettings};
# New: use crate::user::{User, UserSettings};

# 6. Verify
cargo check
cargo test

# 7. Remove old user_management.rs
rm src/user_management.rs
```

### Strategy 3: Incremental Migration

For large, risky refactors:

```bash
# Phase 1: Create parallel structure
mkdir -p src/editor_new/
# Extract types one by one to editor_new/

# Phase 2: Add feature flag
# Cargo.toml
[features]
new_editor = []

# src/lib.rs
#[cfg(feature = "new_editor")]
pub mod editor_new;
#[cfg(not(feature = "new_editor"))]
pub mod editor;

# Phase 3: Test with feature flag
cargo test --features new_editor

# Phase 4: Switch default
# Once stable, make new_editor the default
# Cargo.toml
[features]
default = ["new_editor"]
old_editor = []

# Phase 5: Remove old code
# After confidence period, delete old implementation
```

---

## Common Mistakes

### Mistake 1: Splitting Enum Variants ❌

**Wrong**:
```
// ❌ DON'T DO THIS
src/command/
├── move_up.rs      // Contains: MoveUp variant
├── move_down.rs    // Contains: MoveDown variant
├── save.rs         // Contains: Save variant
└── quit.rs         // Contains: Quit variant
```

**Why Wrong**: Enums are SINGLE types. Variants are not independent types.

**Correct**:
```rust
// src/command.rs
pub enum Command {
    MoveUp,
    MoveDown,
    Save,
    Quit,
}

impl Command {
    pub fn execute(&self) { ... }
}
```

**Exception**: If enum has VERY large variant-specific logic:
```rust
// src/command.rs (enum definition)
pub enum Command {
    MoveUp,
    MoveDown,
    Save,
    Complex(ComplexCommand),
}

// src/command/complex.rs (complex variant logic)
pub struct ComplexCommand {
    // 200 LOC of state
}
impl ComplexCommand {
    // 300 LOC of methods
}
```

---

### Mistake 2: Over-Splitting Small Helpers ❌

**Wrong**:
```
// ❌ Excessive splitting
src/editor/
├── editor.rs (Editor struct, 200 LOC)
├── mode.rs (5 LOC enum)
├── state.rs (8 LOC struct)
├── direction.rs (6 LOC enum)
```

**Why Wrong**: Creates excessive file navigation for tiny types.

**Correct**:
```rust
// src/editor.rs
pub struct Editor {
    mode: Mode,
    state: State,
}

// Private helpers - keep in same file
enum Mode { Normal, Insert }
struct State { modified: bool }
enum Direction { Up, Down, Left, Right }

impl Editor { ... }
```

---

### Mistake 3: Ignoring Exceptions ❌

**Wrong**:
```
// ❌ Forcing split despite Error+ErrorKind pattern
src/error/
├── error.rs       // Just Error struct
├── error_kind.rs  // Just ErrorKind enum
└── mod.rs
```

**Why Wrong**: Breaks natural coupling, requires awkward imports.

**Correct**:
```rust
// src/error.rs
pub enum ErrorKind { ... }
pub struct Error { kind: ErrorKind, ... }
// Both together - they're one conceptual unit
```

---

### Mistake 4: Creating Circular Dependencies ❌

**Wrong**:
```rust
// src/user.rs
use crate::session::Session;
pub struct User {
    active_session: Option<Session>,
}

// src/session.rs
use crate::user::User;
pub struct Session {
    user: User,
}

// ❌ Circular dependency!
```

**Solution Options**:

**Option A**: Keep types together (they're tightly coupled)
```rust
// src/auth.rs
pub struct User { ... }
pub struct Session {
    user_id: UserId,  // Reference by ID, not owned User
}
```

**Option B**: Extract shared types
```rust
// src/types.rs
pub struct UserId(u64);

// src/user.rs
use crate::types::UserId;
pub struct User {
    id: UserId,
    active_session_id: Option<SessionId>,
}

// src/session.rs
use crate::types::UserId;
pub struct Session {
    user_id: UserId,
}
```

---

### Mistake 5: Not Updating Tests ❌

**Problem**: After splitting types, tests import old paths.

**Wrong**:
```rust
// tests/user_test.rs
use crate::user_management::User;  // ❌ Old path

#[test]
fn test_user_creation() { ... }
```

**Correct**:
```rust
// tests/user_test.rs
use crate::user::User;  // ✅ Updated path

#[test]
fn test_user_creation() { ... }
```

**Best Practice**: Search-and-replace import paths across entire codebase:
```bash
# Find all imports of old path
rg "use.*user_management" --type rust

# Replace (carefully!)
sed -i 's/user_management::User/user::User/g' **/*.rs
```

---

## Summary

### Apply "1 File = 1 Structure" When:
- ✅ Multiple large public types (>80 LOC each)
- ✅ File >1000 LOC with multiple types
- ✅ Types from different domains
- ✅ 3+ public types (check exceptions first)

### Exceptions (Keep Together):
- ✅ Error + ErrorKind pattern
- ✅ Builder pattern
- ✅ Small private helpers (<30 LOC)
- ✅ DTO families (<50 LOC each, same domain)
- ✅ Typestate pattern
- ✅ Newtype collections (<10 LOC each)

### Don't Split:
- ❌ Enum variants
- ❌ Generated code
- ❌ Test helper utilities
- ❌ Types <100 LOC total that are tightly coupled

### Migration Checklist:
1. ☐ Identify types to extract
2. ☐ Check for exceptions
3. ☐ Create new file(s)
4. ☐ Move type definition + impl
5. ☐ Add pub mod declaration
6. ☐ Update imports
7. ☐ Run cargo check
8. ☐ Run cargo test
9. ☐ Update test imports
10. ☐ Remove old code
11. ☐ Final verification

---

**See Also**:
- `analyzers/file-decomposition-analyzer.md` for automated analysis
- `phases/5-execution.md` for decomposition execution workflow
