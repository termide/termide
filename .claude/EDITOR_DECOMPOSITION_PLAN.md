# Editor.rs & Key_handler.rs Decomposition Plan

## Summary of Analysis

### editor.rs (3,374 LOC)
**Problem areas:**
- `render_content()`: 718 LOC - massive rendering function
- `handle_key()` (Panel trait): 590 LOC - huge key dispatcher
- 80+ methods handling 11 distinct responsibilities

**Top extraction candidates:**
1. Rendering (718 LOC) - Extract to `panels/editor/rendering/` module
2. Keyboard handling (590 LOC) - Extract to `panels/editor/keyboard.rs`
3. Word wrap calculations (150 LOC) - Extract to `panels/editor/word_wrap.rs`
4. Search/replace (265 LOC) - Extract to `panels/editor/search.rs`
5. Cursor movement (358 LOC) - Extract to `panels/editor/cursor.rs`

### key_handler.rs (995 LOC)
**Problem areas:**
- `handle_key_event()`: 242 LOC - main dispatcher
- `handle_global_hotkeys()`: 235 LOC - global hotkey handler
- `handle_resize_panel()`: 107 LOC - panel resizing logic

**Top extraction candidates:**
1. Global hotkeys (235 LOC) - Extract to `app/hotkeys.rs`
2. Panel management (107 LOC) - Extract to `app/panel_mgmt.rs`
3. Menu handling (55 LOC) - Extract to `app/menu_handler.rs`

---

## Decomposition Strategy

### Principle: Progressive, Non-Destructive Refactoring

Instead of rewriting everything, we'll:
1. **Extract** helper modules first (word_wrap, search, etc.)
2. **Delegate** methods from Editor to extracted modules
3. **Keep** Editor as orchestrator/facade
4. **Test** after each extraction

### NOT using feature flags

The previous plan suggested parallel module systems with feature flags (editor-v1/v2), but that adds complexity. Instead:
- Extract modules one by one
- Keep Editor as thin wrapper
- Maintain backward compatibility
- Test continuously

---

## Implementation Phases

### PHASE 1: Extract Simple Helper Modules (Low Risk)

These modules have minimal dependencies and can be extracted safely:

#### 1.1 Word Wrap Module
**File:** `src/panels/editor/word_wrap.rs`
**Extract from editor.rs lines:** 2408-2557 (~150 LOC)

**Methods to move:**
- `get_line_wrap_points()`
- `calculate_visual_row_for_cursor()`
- `calculate_total_visual_rows()`
- `visual_row_to_buffer_position()`

**Benefits:**
- Pure geometric calculations
- Easy to unit test
- Reusable across editor features

**Effort:** 2-3 hours

---

#### 1.2 Git Integration Module
**File:** `src/panels/editor/git.rs`
**Extract from editor.rs lines:** 248-336, 1313-1420 (~120 LOC)

**Methods to move:**
- `update_git_diff()`
- `schedule_git_diff_update()`
- `check_pending_git_diff_update()`
- `get_git_line_info()`
- `build_virtual_lines()`

**Types to move:**
- `GitLineInfo` struct
- `VirtualLine` enum

**Benefits:**
- Self-contained git operations
- Clear cache interface
- Minimal coupling

**Effort:** 2-3 hours

---

#### 1.3 Config Module
**File:** `src/panels/editor/config.rs`
**Extract from editor.rs lines:** 20-64 (~45 LOC)

**Types to move:**
- `EditorConfig` struct
- `EditorInfo` struct

**Benefits:**
- Clean separation of configuration
- Easier to test config logic
- Can add config validation

**Effort:** 1 hour

---

### PHASE 2: Extract Feature Modules (Medium Risk)

#### 2.1 Search & Replace Module
**File:** `src/panels/editor/search.rs`
**Extract from editor.rs lines:** 2139-2403 (~265 LOC)

**Methods to move:**
- `start_search()`
- `perform_search()` (private)
- `search_next()`
- `search_prev()`
- `close_search()`
- `get_search_match_info()`
- `start_replace()`
- `update_replace_with()`
- `replace_current()`
- `replace_all()`

**Dependencies:**
- Uses `SearchState` from `crate::editor`
- Calls `delete_selection()` - keep in Editor
- Uses `buffer`, `cursor`, `selection`

**Benefits:**
- Complex search logic isolated
- Easier to add features (regex, case folding)
- Better testability

**Effort:** 4-5 hours

---

#### 2.2 Selection Module
**File:** `src/panels/editor/selection.rs`
**Extract from editor.rs lines:** 954-1052 (~99 LOC)

**Methods to move:**
- `select_all()`
- `start_or_extend_selection()`
- `update_selection_active()`
- `get_selected_text()`
- `delete_selection()`

**Benefits:**
- Selection state management isolated
- Used by many features (clipboard, search, edit)
- Clear API

**Effort:** 2-3 hours

---

#### 2.3 Clipboard Module
**File:** `src/panels/editor/clipboard.rs`
**Extract from editor.rs lines:** 1054-1149 (~96 LOC)

**Methods to move:**
- `copy_to_clipboard()`
- `cut_to_clipboard()`
- `paste_from_clipboard()`

**Dependencies:**
- Uses `selection` module
- Uses `crate::clipboard`
- Updates status messages

**Benefits:**
- System clipboard abstraction
- Easier to mock for testing
- Clean error handling

**Effort:** 2-3 hours

---

#### 2.4 Text Editing Module
**File:** `src/panels/editor/text_editing.rs`
**Extract from editor.rs lines:** 1220-1310 (~91 LOC)

**Methods to move:**
- `insert_char()`
- `insert_newline()`
- `backspace()`
- `delete()`
- `duplicate_line()`

**Dependencies:**
- Uses `buffer`, `cursor`, `highlight_cache`
- Calls `delete_selection()`
- Schedules git diff updates

**Benefits:**
- Core editing logic isolated
- Cache invalidation centralized
- Easier to add smart editing features

**Effort:** 2-3 hours

---

### PHASE 3: Extract Complex Modules (High Risk)

#### 3.1 Cursor Movement Module
**File:** `src/panels/editor/cursor.rs`
**Extract from editor.rs lines:** 498-856 (~358 LOC)

**Submodules:**
- `cursor/physical.rs` - Basic movement (up/down/left/right)
- `cursor/visual.rs` - Wrap-aware movement
- `cursor/jump.rs` - Page/home/end movement

**Methods to move:**
Physical movement:
- `move_cursor_up/down/left/right()`
- `move_to_line_start/end()`
- `move_to_document_start/end()`
- `page_up/down()`
- `clamp_cursor()`

Visual movement (complex):
- `move_cursor_up/down_visual()`
- `move_to_visual_line_start/end()`
- `page_up/down_visual()`

**Challenges:**
- Visual movement has 221 LOC of complex geometry
- Duplicated logic between up/down visual
- Heavy dependency on word_wrap module

**Refactoring opportunity:**
Create `VisualMovement` helper struct to deduplicate up/down logic

**Effort:** 6-8 hours

---

#### 3.2 Rendering Module
**File:** `src/panels/editor/rendering/mod.rs`
**Extract from editor.rs lines:** 1422-2139 (~718 LOC)

**THIS IS THE BIGGEST REFACTORING**

**Submodules:**
- `rendering/context.rs` - RenderContext builder
- `rendering/lines.rs` - Line rendering loop
- `rendering/highlights.rs` - Search/selection highlights
- `rendering/line_numbers.rs` - Line number rendering
- `rendering/git_markers.rs` - Git diff indicators

**Main method:**
- `render_content()` - 718 LOC monolith

**Proposed breakdown:**
```rust
// rendering/mod.rs
pub struct RenderContext {
    area: Rect,
    content_width: usize,
    styles: ThemeStyles,
    virtual_lines: Vec<VirtualLine>,
    // ... other prepared state
}

impl Editor {
    fn render_content(&self, frame, area, state) {
        let ctx = RenderContext::prepare(self, area, state);
        rendering::render_editor(frame, &ctx, self);
    }
}

// rendering/lines.rs
pub fn render_editor(frame, ctx, editor) {
    render_line_numbers(frame, ctx, editor);
    render_text_content(frame, ctx, editor);
    ensure_cursor_visible(frame, ctx, editor);
}
```

**Benefits:**
- Rendering logic separated from editing logic
- Easier to optimize rendering performance
- Testable render components
- Clearer code structure

**Challenges:**
- Tightly coupled to all editor state
- Many dependencies (buffer, cursor, selection, search, viewport, git, highlights)
- Needs careful API design

**Effort:** 10-15 hours

---

#### 3.3 Keyboard Handler Module
**File:** `src/panels/editor/keyboard.rs`
**Extract from editor.rs lines:** 2560-3374 (Panel trait impl, ~590 LOC in handle_key)

**Proposed approach: Command Pattern**

```rust
// keyboard.rs
pub enum EditorCommand {
    // Navigation
    MoveCursorUp,
    MoveCursorDown,
    MoveToLineStart,
    PageUp,
    // ... all other commands
}

impl EditorCommand {
    pub fn from_key_event(key: KeyEvent, editor_state: &EditorState) -> Option<Self> {
        // Map keys to commands
    }

    pub fn execute(self, editor: &mut Editor) -> Result<()> {
        match self {
            Self::MoveCursorUp => editor.move_cursor_up(),
            // ...
        }
    }
}

// In Panel trait impl:
fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
    if let Some(cmd) = EditorCommand::from_key_event(key, &self.state) {
        cmd.execute(self)?;
    }
    // ... modal handling, etc.
}
```

**Benefits:**
- 590 LOC match statement replaced with command enum
- Commands are testable independently
- Can record/replay commands (undo/redo improvement)
- Clearer key mapping

**Effort:** 8-10 hours

---

### PHASE 4: Extract App Key Handler (Medium Risk)

#### 4.1 Global Hotkeys Module
**File:** `src/app/hotkeys.rs`
**Extract from key_handler.rs lines:** 760-994 (~235 LOC)

**Method to move:**
- `handle_global_hotkeys()` - entire 235 LOC method

**Proposed structure:**
```rust
// app/hotkeys.rs
pub struct GlobalHotkeys {
    // Any state needed
}

impl GlobalHotkeys {
    pub fn handle(&self, key: KeyEvent, app: &mut App) -> Result<Option<()>> {
        // All the match logic from handle_global_hotkeys
        match (key.code, key.modifiers) {
            (KeyCode::Char('m'), KeyModifiers::ALT) => {
                app.toggle_menu();
                Ok(Some(()))
            }
            // ... rest of hotkeys
        }
    }
}
```

**Benefits:**
- 235 LOC extracted from key_handler.rs
- Hotkeys centralized
- Easier to document keybindings
- Can generate help from this module

**Effort:** 3-4 hours

---

#### 4.2 Panel Management Module
**File:** `src/app/panel_mgmt.rs`
**Extract from key_handler.rs:** Multiple methods (~300 LOC total)

**Methods to move:**
- `handle_new_terminal()` (21 LOC)
- `handle_new_file_manager()` (15 LOC)
- `handle_new_editor()` (8 LOC)
- `handle_new_debug()` (15 LOC)
- `focus_existing_debug_panel()` (16 LOC)
- `handle_new_help()` (9 LOC)
- `handle_swap_panel_left()` (26 LOC)
- `handle_swap_panel_right()` (26 LOC)
- `handle_resize_panel()` (107 LOC)
- `handle_close_panel_request()` (40 LOC)
- `close_welcome_panels()` (51 LOC)
- `has_panels_requiring_confirmation()` (27 LOC)

**Benefits:**
- Panel lifecycle management centralized
- Cleaner App struct
- Easier to add new panel types

**Effort:** 4-5 hours

---

#### 4.3 Menu Handler Module
**File:** `src/app/menu_handler.rs`
**Extract from key_handler.rs:** ~90 LOC

**Methods to move:**
- `handle_menu_key()` (18 LOC)
- `execute_menu_action()` (55 LOC)
- `open_config_in_editor()` (32 LOC)

**Benefits:**
- Menu logic separated
- Easier to extend menu system
- Cleaner key_handler

**Effort:** 2-3 hours

---

## File Structure After Decomposition

```
src/
├── panels/
│   └── editor/
│       ├── mod.rs               # Main Editor struct (500-700 LOC)
│       ├── config.rs            # EditorConfig, EditorInfo (45 LOC)
│       ├── word_wrap.rs         # Wrap calculations (150 LOC)
│       ├── git.rs               # Git integration (120 LOC)
│       ├── search.rs            # Search & replace (265 LOC)
│       ├── selection.rs         # Selection ops (99 LOC)
│       ├── clipboard.rs         # Clipboard ops (96 LOC)
│       ├── text_editing.rs      # Insert/delete (91 LOC)
│       ├── cursor/
│       │   ├── mod.rs           # Common cursor utilities
│       │   ├── physical.rs      # Basic movement (100 LOC)
│       │   ├── visual.rs        # Wrap-aware movement (200 LOC)
│       │   └── jump.rs          # Page/home/end (58 LOC)
│       ├── rendering/
│       │   ├── mod.rs           # Main render orchestration (100 LOC)
│       │   ├── context.rs       # RenderContext builder (50 LOC)
│       │   ├── lines.rs         # Line rendering (300 LOC)
│       │   ├── highlights.rs    # Highlight application (100 LOC)
│       │   ├── line_numbers.rs  # Line numbers (80 LOC)
│       │   └── git_markers.rs   # Git indicators (50 LOC)
│       └── keyboard.rs          # EditorCommand pattern (600 LOC)
│
└── app/
    ├── mod.rs                   # App struct
    ├── key_handler.rs           # Main dispatcher (200 LOC, down from 995)
    ├── hotkeys.rs               # Global hotkeys (235 LOC)
    ├── panel_mgmt.rs            # Panel lifecycle (300 LOC)
    └── menu_handler.rs          # Menu handling (90 LOC)
```

---

## Metrics

### Before:
- `editor.rs`: 3,374 LOC
- `key_handler.rs`: 995 LOC
- **Total**: 4,369 LOC in 2 files

### After:
- Editor modules: ~2,900 LOC across 16 files (avg 181 LOC/file)
- App modules: ~825 LOC across 4 files (avg 206 LOC/file)
- **Total**: 3,725 LOC (saves ~644 LOC through deduplication)

### Largest remaining files:
- `editor/keyboard.rs`: 600 LOC (down from 590 but with cleaner structure)
- `editor/rendering/lines.rs`: 300 LOC (down from 718)
- `editor/mod.rs`: 500-700 LOC (orchestration)

---

## Implementation Order (Recommended)

### Week 1: Easy Wins
1. ✅ Extract `word_wrap.rs` (3h)
2. ✅ Extract `git.rs` (3h)
3. ✅ Extract `config.rs` (1h)
4. ✅ Test all editor functionality

### Week 2: Feature Modules
5. ✅ Extract `selection.rs` (3h)
6. ✅ Extract `clipboard.rs` (3h)
7. ✅ Extract `text_editing.rs` (3h)
8. ✅ Extract `search.rs` (5h)
9. ✅ Test all features

### Week 3: Complex Editor Modules
10. ✅ Extract `cursor/` modules (8h)
11. ✅ Deduplicate visual movement logic
12. ✅ Test cursor movement extensively

### Week 4: Rendering Refactor
13. ✅ Design RenderContext API
14. ✅ Extract `rendering/` modules (15h)
15. ✅ Test rendering thoroughly

### Week 5: Keyboard Handler
16. ✅ Design EditorCommand enum
17. ✅ Extract `keyboard.rs` (10h)
18. ✅ Test all keybindings

### Week 6: App Key Handler
19. ✅ Extract `hotkeys.rs` (4h)
20. ✅ Extract `panel_mgmt.rs` (5h)
21. ✅ Extract `menu_handler.rs` (3h)
22. ✅ Test app-level functionality

---

## Testing Strategy

After each extraction:
1. **Unit tests** for extracted module (new tests)
2. **Integration tests** for Editor/App (existing tests)
3. **Manual testing** of affected features
4. **Performance check** (rendering, key handling)

Critical test areas:
- Cursor movement (especially visual with word wrap)
- Selection + clipboard operations
- Search and replace
- Rendering with various content (long lines, unicode, git diffs)
- All keybindings

---

## Risks & Mitigation

### Risk 1: Breaking existing functionality
**Mitigation:**
- Extract one module at a time
- Run full test suite after each extraction
- Keep git commits small and focused
- Manual testing checklist

### Risk 2: Performance regression
**Mitigation:**
- Profile before/after major extractions
- Watch for extra allocations
- Keep hot paths (rendering, key handling) optimized
- Benchmark critical operations

### Risk 3: API design mistakes
**Mitigation:**
- Design module APIs carefully (public vs private)
- Keep Editor as facade - don't expose too much
- Iterate on API during extraction
- Document module boundaries

### Risk 4: Complex dependencies
**Mitigation:**
- Start with simple, isolated modules
- Identify circular dependencies early
- Use traits for abstraction where needed
- Keep Editor as state owner

---

## Success Criteria

1. ✅ All existing tests pass
2. ✅ No new clippy warnings
3. ✅ Code coverage maintained or improved
4. ✅ Performance within 5% of baseline
5. ✅ Each module < 600 LOC
6. ✅ Clear module responsibilities
7. ✅ Improved testability (more unit tests possible)

---

## Notes

- This is a **large refactoring** (~40-50 hours estimated)
- Can be done **incrementally** (one module per session)
- **Not a rewrite** - we're extracting, not reimplementing
- Focus on **maintainability** over clever abstractions
- **Test continuously** - don't accumulate untested changes
