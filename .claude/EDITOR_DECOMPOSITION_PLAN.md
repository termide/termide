# Editor Decomposition Plan - Updated 2025-12-05

## Executive Summary

**Status:** Phases 1-3 COMPLETED ✅ | Phase 4 PARTIAL ⚠️ | Phase 5 NOT STARTED ❌

### Current Metrics (2025-12-05)

**editor/core.rs: 2,584 LOC** ⚠️ STILL A MONSTERFILE!

Critical problem areas:
- `render_content()`: **732 LOC** (28% of file!) 🔴 URGENT
- `handle_key()`: **345 LOC** (13% of file!) 🔴 HIGH PRIORITY
- **Total: 1,077 LOC in 2 methods = 42% of entire file!**

### Original Metrics (Before Decomposition)

- `editor.rs`: 3,374 LOC
- `key_handler.rs`: 995 LOC
- **Total**: 4,369 LOC in 2 files

### Progress So Far

**Extracted:** 790 LOC from core.rs (Phases 1-3)
**Remaining:** 2,584 LOC (65% of original!)

**Next Priority:** Extract rendering (732 LOC) + keyboard (345 LOC) = 1,077 LOC (42% reduction!)

---

## Module Structure: BEFORE vs AFTER Decomposition

### ✅ COMPLETED (Phases 1-3)

```
src/panels/editor/
├── mod.rs                    18 LOC ✅
├── config.rs                 49 LOC ✅ (Phase 1.3)
├── word_wrap.rs             207 LOC ✅ (Phase 1.1)
├── git.rs                   161 LOC ✅ (Phase 1.2)
├── search.rs                142 LOC ✅ (Phase 2.1)
├── selection.rs             124 LOC ✅ (Phase 2.2)
├── clipboard.rs             107 LOC ✅ (Phase 2.3)
├── text_editing.rs          139 LOC ✅ (Phase 2.4)
└── cursor/                  425 LOC ✅ (Phase 3.1)
    ├── mod.rs                12 LOC
    ├── physical.rs           91 LOC
    ├── visual.rs            300 LOC
    └── jump.rs               22 LOC
```

### ⚠️ PARTIAL (Phase 4)

```
└── rendering/                21 LOC ⚠️ (Only constants!)
    └── mod.rs                21 LOC  (LINE_NUMBER_WIDTH, calculate_content_dimensions)
```

**Problem:** 732 lines of render_content() NOT EXTRACTED!

### ❌ NOT STARTED (Phase 5)

```
└── keyboard.rs           NOT CREATED ❌
```

**Problem:** 345 lines of handle_key() still in core.rs!

---

## Implementation Phases - DETAILED STATUS

### PHASE 1: Extract Simple Helper Modules ✅ COMPLETED

| Module | Status | LOC | Effort | Date Completed |
|--------|--------|-----|--------|----------------|
| word_wrap.rs | ✅ DONE | 207 | 2-3h | 2024 |
| git.rs | ✅ DONE | 161 | 2-3h | 2024 |
| config.rs | ✅ DONE | 49 | 1h | 2024 |

**Result:** 417 LOC extracted, tested, working perfectly.

---

### PHASE 2: Extract Feature Modules ✅ COMPLETED

| Module | Status | LOC | Effort | Date Completed |
|--------|--------|-----|--------|----------------|
| search.rs | ✅ DONE | 142 | 4-5h | 2024 |
| selection.rs | ✅ DONE | 124 | 2-3h | 2024 |
| clipboard.rs | ✅ DONE | 107 | 2-3h | 2024 |
| text_editing.rs | ✅ DONE | 139 | 2-3h | 2024 |

**Result:** 512 LOC extracted, all tests passing.

---

### PHASE 3: Extract Cursor Modules ✅ COMPLETED

| Module | Status | LOC | Effort | Date Completed |
|--------|--------|-----|--------|----------------|
| cursor/physical.rs | ✅ DONE | 91 | 2-3h | 2024 |
| cursor/visual.rs | ✅ DONE | 300 | 4-5h | 2024 |
| cursor/jump.rs | ✅ DONE | 22 | 1h | 2024 |
| cursor/mod.rs | ✅ DONE | 12 | - | 2024 |

**Result:** 425 LOC extracted. Visual navigation working with word wrap.

**Note:** cursor/visual.rs (300 LOC) has optimization potential - can reduce by ~50 LOC through deduplication.

---

### PHASE 4: Extract Rendering Module ⚠️ CRITICAL - PARTIAL

**Status:** ONLY STARTED - rendering/mod.rs with constants only!

**Problem:** Main `render_content()` method (732 LOC) is still in core.rs!

**Current:** 21 LOC in rendering/mod.rs
**Target:** 830 LOC across 7 submodules
**Impact:** Will reduce core.rs from 2,584 to ~1,850 LOC (-28%!)

#### Proposed Structure:

```
src/panels/editor/rendering/
├── mod.rs                   ~100 LOC  (orchestrator + re-exports)
├── context.rs                ~80 LOC  (RenderContext + styles)
├── line_rendering.rs        ~200 LOC  (no word wrap mode)
├── wrap_rendering.rs        ~250 LOC  (word wrap mode) ⚠️ MOST COMPLEX
├── highlight_renderer.rs    ~100 LOC  (search/selection highlights)
├── cursor_renderer.rs        ~40 LOC  (cursor inversion)
└── deletion_markers.rs       ~60 LOC  (git deletion markers)

TOTAL: ~830 LOC (extracted from 732 LOC render_content + duplications)
```

#### Detailed Implementation Plan:

**Step 4.1: Create RenderContext (context.rs) - 2-3 hours**

```rust
/// Rendering context with all prepared data
pub struct RenderContext {
    // Dimensions
    pub area: Rect,
    pub content_width: usize,
    pub content_height: usize,
    pub line_number_width: u16,

    // Styles (pre-built from theme)
    pub text_style: Style,
    pub line_number_style: Style,
    pub cursor_line_style: Style,
    pub search_match_style: Style,
    pub current_match_style: Style,
    pub selection_style: Style,

    // Pre-extracted render data
    pub selection_range: Option<(Cursor, Cursor)>,
    pub search_match_map: HashMap<(usize, usize), usize>,  // O(1) lookup
    pub current_match_idx: Option<usize>,
    pub virtual_lines_total: usize,

    // Settings
    pub use_smart_wrap: bool,
    pub word_wrap: bool,
}

impl RenderContext {
    /// Prepare all render data from editor state
    pub fn prepare(
        editor: &Editor,
        area: Rect,
        theme: &Theme,
        config: &Config,
    ) -> Self { ... }

    /// Build search match HashMap for O(1) character lookup
    pub fn build_search_match_map(
        search_state: &Option<SearchState>
    ) -> HashMap<(usize, usize), usize> { ... }
}
```

**Extract from:** render_content lines 886-952 (initialization section)

---

**Step 4.2: Extract Cursor Renderer (cursor_renderer.rs) - 1 hour**

```rust
/// Render cursor with inverted foreground/background
pub fn render_cursor(
    buf: &mut Buffer,
    area: Rect,
    cursor_viewport_pos: Option<(usize, usize)>,
    line_number_width: u16,
    theme: &Theme,
) { ... }
```

**Extract from:** render_content lines ~1585-1608 (cursor inversion logic)

**Simple, isolated, easy win.**

---

**Step 4.3: Extract Deletion Markers (deletion_markers.rs) - 1-2 hours**

```rust
/// Render git deletion marker line: "─── 3 deleted lines ───"
pub fn render_deletion_marker(
    buf: &mut Buffer,
    area: Rect,
    visual_row: usize,
    deletion_count: usize,
    content_width: usize,
    line_number_width: u16,
    theme: &Theme,
) { ... }
```

**Extract from:**
- Word wrap mode: lines ~1280-1310
- No wrap mode: lines ~1550-1582

**Reused in both rendering modes - good candidate for extraction.**

---

**Step 4.4: Extract Highlight Renderer (highlight_renderer.rs) - 2-3 hours**

```rust
/// Determine final style for a character considering all highlights
pub fn apply_highlights(
    ch: char,
    base_style: Style,
    line_idx: usize,
    col_idx: usize,
    is_cursor_line: bool,
    ctx: &RenderContext,
) -> Style {
    // Priority: search match > selection > cursor line > syntax
    // Returns final style with all layers applied
}

/// Get syntax highlight segments for a line
pub fn get_line_segments(
    highlight_cache: &HighlightCache,
    line_idx: usize,
    line_text: &str,
    syntax_highlighting: bool,
    base_style: Style,
) -> Vec<(String, Style)> { ... }
```

**Extract from:**
- Word wrap: lines ~1125-1163 (character styling)
- No wrap: lines ~1390-1510 (line segments + character styling)

**Medium complexity - central to visual appearance.**

---

**Step 4.5: Extract Line Rendering (line_rendering.rs) - 5-6 hours**

```rust
/// Render editor content without word wrap
pub fn render_without_word_wrap(
    editor: &Editor,
    buf: &mut Buffer,
    ctx: &RenderContext,
    theme: &Theme,
    config: &Config,
) -> Option<(usize, usize)> {
    // Returns cursor_viewport_pos

    // 1. Build virtual lines (buffer lines + deletion markers)
    let virtual_lines = git::build_virtual_lines(...);

    // 2. Find start virtual index
    let start_idx = ...;

    // 3. Render loop
    for row in 0..ctx.content_height {
        match virtual_lines[...] {
            VirtualLine::Real(line_idx) => {
                render_real_line(...);
            }
            VirtualLine::Deletion(count) => {
                deletion_markers::render_deletion_marker(...);
            }
        }
    }

    // 4. Calculate cursor position
    cursor_viewport_pos
}

/// Render a single real line (no wrap)
fn render_real_line(
    buf: &mut Buffer,
    area: Rect,
    row: usize,
    line_idx: usize,
    editor: &Editor,
    ctx: &RenderContext,
    theme: &Theme,
) {
    // Line number + git marker
    // Get syntax segments
    // Render characters with horizontal scroll
    // Apply search/selection highlights
}
```

**Extract from:** render_content lines 1321-1608 (else branch, no wrap mode)

**High complexity - but cleanly separable.**

---

**Step 4.6: Extract Wrap Rendering (wrap_rendering.rs) - 6-8 hours** ⚠️ **MOST COMPLEX**

```rust
/// Render editor content with word wrap
pub fn render_with_word_wrap(
    editor: &mut Editor,
    buf: &mut Buffer,
    ctx: &RenderContext,
    theme: &Theme,
    config: &Config,
) -> Option<(usize, usize)> {
    // Returns cursor_viewport_pos

    let mut cursor_viewport_pos = None;
    let mut visual_row = 0;
    let mut line_idx = editor.viewport.top_line;

    while visual_row < ctx.content_height && line_idx < editor.buffer.line_count() {
        let line_text = editor.buffer.line(line_idx)?;

        if line_text.is_empty() {
            // Render empty line (single visual row)
            render_empty_line_wrapped(...);
            visual_row += 1;
        } else {
            // Render line with wrapping (multiple visual rows)
            visual_row = render_line_wrapped(
                buf, area, visual_row, line_idx, line_text,
                editor, ctx, theme, &mut cursor_viewport_pos
            );
        }

        // Render deletion markers for this line
        if config.show_git_diff {
            visual_row = render_line_deletion_markers(...);
        }

        line_idx += 1;
    }

    cursor_viewport_pos
}

/// Render empty line in word wrap mode
fn render_empty_line_wrapped(...) {
    // Git info + line number
    // Fill with spaces
    // Track cursor position
}

/// Render non-empty line with word wrapping
fn render_line_wrapped(
    buf: &mut Buffer,
    area: Rect,
    mut visual_row: usize,
    line_idx: usize,
    line_text: &str,
    editor: &Editor,
    ctx: &RenderContext,
    theme: &Theme,
    cursor_viewport_pos: &mut Option<(usize, usize)>,
) -> usize {
    let chars: Vec<char> = line_text.chars().collect();
    let mut char_offset = 0;

    while char_offset < chars.len() && visual_row < ctx.content_height {
        // Calculate chunk_end (word wrap point)
        let chunk_end = calculate_wrap_point(...);

        // Render line number (only for first visual row)
        if char_offset == 0 {
            render_line_number_with_git(...);
        } else {
            render_continuation_marker(...);
        }

        // Get syntax segments for this chunk
        let segments = get_chunk_segments(...);

        // Render characters
        for (col, ch) in chunk.chars().enumerate() {
            let style = highlight_renderer::apply_highlights(...);
            render_char(...);

            // Track cursor
            if is_cursor_position(...) {
                *cursor_viewport_pos = Some((visual_row, col));
            }
        }

        // Fill rest of row
        fill_row_remainder(...);

        char_offset = chunk_end;
        visual_row += 1;
    }

    visual_row
}
```

**Extract from:** render_content lines 957-1320 (if word_wrap branch)

**Very high complexity:**
- ~400 LOC of intricate logic
- Smart vs simple wrap
- Multiple nested loops
- Cursor tracking across visual rows
- Git markers interleaved

**Requires careful extraction and testing!**

---

**Step 4.7: Create Orchestrator (rendering/mod.rs) - 3-4 hours**

```rust
// Re-exports
pub mod context;
pub mod cursor_renderer;
pub mod deletion_markers;
pub mod highlight_renderer;
pub mod line_rendering;
pub mod wrap_rendering;

pub use context::RenderContext;

// Constants (already exist)
pub const LINE_NUMBER_WIDTH: usize = 6;

pub fn calculate_content_dimensions(
    area_width: u16,
    area_height: u16,
) -> (usize, usize) {
    let content_width = (area_width as usize).saturating_sub(LINE_NUMBER_WIDTH);
    let content_height = area_height as usize;
    (content_width, content_height)
}

/// Main rendering entry point
pub fn render_editor_content(
    editor: &mut Editor,
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    config: &Config,
) {
    // 1. Prepare render context
    let mut ctx = RenderContext::prepare(editor, area, theme, config);

    // 2. Update viewport and caches
    editor.viewport.resize(ctx.content_width, ctx.content_height);
    editor.cached_virtual_line_count = ctx.virtual_lines_total;
    editor.viewport.ensure_cursor_visible(&editor.cursor, ctx.virtual_lines_total);

    // 3. Cache settings for navigation
    editor.cached_content_width = if editor.config.word_wrap {
        ctx.content_width
    } else {
        0
    };
    editor.cached_use_smart_wrap = ctx.use_smart_wrap;

    // 4. Choose rendering mode
    let cursor_viewport_pos = if ctx.word_wrap {
        wrap_rendering::render_with_word_wrap(editor, buf, &ctx, theme, config)
    } else {
        line_rendering::render_without_word_wrap(editor, buf, &ctx, theme, config)
    };

    // 5. Render cursor
    cursor_renderer::render_cursor(
        buf,
        area,
        cursor_viewport_pos,
        ctx.line_number_width,
        theme,
    );
}
```

**In Editor::render_content (core.rs), replace 732 lines with:**

```rust
fn render_content(
    &mut self,
    area: Rect,
    buf: &mut Buffer,
    theme: &crate::theme::Theme,
    config: &crate::config::Config,
) {
    rendering::render_editor_content(self, area, buf, theme, config);
}
```

**This is the final integration step.**

---

#### Phase 4 Effort Summary:

| Step | Module | Hours | Complexity | Priority |
|------|--------|-------|-----------|----------|
| 4.1 | context.rs | 2-3 | LOW | START HERE |
| 4.2 | cursor_renderer.rs | 1 | LOW | EASY WIN |
| 4.3 | deletion_markers.rs | 1-2 | LOW | REUSABLE |
| 4.4 | highlight_renderer.rs | 2-3 | MEDIUM | CORE LOGIC |
| 4.5 | line_rendering.rs | 5-6 | HIGH | COMPLEX |
| 4.6 | wrap_rendering.rs | 6-8 | VERY HIGH | HARDEST |
| 4.7 | mod.rs orchestrator | 3-4 | MEDIUM | INTEGRATION |
| Testing | Full render testing | 4-6 | - | CRITICAL |

**TOTAL: 24-33 hours**

**Result:** core.rs will shrink from 2,584 to ~1,850 LOC (-28%)

---

### PHASE 5: Extract Keyboard Module ❌ NOT STARTED

**Status:** 345 lines of handle_key() still in core.rs!

**Approach:** Command Pattern

#### Proposed Structure:

```
src/panels/editor/keyboard.rs  (~400 LOC)
```

#### Implementation Plan:

**Step 5.1: Create EditorCommand enum - 4-5 hours**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum EditorCommand {
    // Navigation
    MoveCursorUp,
    MoveCursorDown,
    MoveCursorLeft,
    MoveCursorRight,
    MoveToLineStart,
    MoveToLineEnd,
    MoveToVisualLineStart,
    MoveToVisualLineEnd,
    PageUp,
    PageDown,
    MoveToDocumentStart,
    MoveToDocumentEnd,

    // Navigation with selection (Shift)
    MoveCursorUpWithSelection,
    MoveCursorDownWithSelection,
    MoveCursorLeftWithSelection,
    MoveCursorRightWithSelection,
    MoveToLineStartWithSelection,
    MoveToLineEndWithSelection,
    PageUpWithSelection,
    PageDownWithSelection,
    MoveToDocumentStartWithSelection,
    MoveToDocumentEndWithSelection,

    // Editing
    InsertChar(char),
    InsertNewline,
    Backspace,
    Delete,

    // Special commands
    Undo,
    Redo,
    Save,
    SelectAll,
    Copy,
    Cut,
    Paste,
    DuplicateLine,

    // Search
    SearchNext,
    SearchPrev,
    StartSearch,
    CloseSearch,
    StartReplace,
    ReplaceNext,
    ReplaceAll,

    // No operation
    None,
}

impl EditorCommand {
    /// Parse KeyEvent into EditorCommand
    pub fn from_key_event(
        key: KeyEvent,
        read_only: bool,
        has_search: bool,
    ) -> Self {
        // All 345 lines of match logic from handle_key
        // But just returns command, doesn't execute

        match (key.code, key.modifiers) {
            (KeyCode::Up, KeyModifiers::NONE) => Self::MoveCursorUp,
            (KeyCode::Up, KeyModifiers::SHIFT) => Self::MoveCursorUpWithSelection,
            (KeyCode::Char('s'), KeyModifiers::CONTROL) if !read_only => Self::Save,
            // ... etc
            _ => Self::None,
        }
    }
}
```

**Extract from:** handle_key lines 2020-2364 (all key mappings)

---

**Step 5.2: Implement EditorCommand::execute - 3-4 hours**

```rust
impl EditorCommand {
    /// Execute command on editor
    pub fn execute(self, editor: &mut Editor) -> Result<()> {
        match self {
            // Navigation - use existing helper methods
            Self::MoveCursorUp => {
                editor.navigate(
                    Editor::move_cursor_up_visual,
                    Editor::move_cursor_up,
                );
                Ok(())
            }
            Self::MoveCursorDown => {
                editor.navigate(
                    Editor::move_cursor_down_visual,
                    Editor::move_cursor_down,
                );
                Ok(())
            }

            // Navigation with selection
            Self::MoveCursorUpWithSelection => {
                editor.navigate_with_selection(
                    Editor::move_cursor_up_visual,
                    Editor::move_cursor_up,
                );
                Ok(())
            }

            // Editing
            Self::InsertChar(ch) => editor.insert_char(ch),
            Self::InsertNewline => editor.insert_newline(),
            Self::Backspace => editor.handle_delete_key(|e| e.backspace()),
            Self::Delete => editor.handle_delete_key(|e| e.delete()),

            // Special
            Self::Undo => editor.handle_undo_redo(|buf| buf.undo()),
            Self::Redo => editor.handle_undo_redo(|buf| buf.redo()),
            Self::Save => editor.save(),
            Self::SelectAll => {
                editor.select_all();
                Ok(())
            }
            Self::Copy => editor.copy_to_clipboard(),
            Self::Cut => editor.cut_to_clipboard(),
            Self::Paste => editor.paste_from_clipboard(),
            Self::DuplicateLine => editor.duplicate_line(),

            // Search
            Self::SearchNext => {
                editor.search_next_or_open();
                Ok(())
            }
            Self::SearchPrev => {
                editor.search_prev_or_open();
                Ok(())
            }
            Self::StartSearch => {
                editor.open_search_modal(true);
                Ok(())
            }
            Self::CloseSearch => {
                if editor.search_state.is_some() {
                    editor.close_search();
                }
                Ok(())
            }

            // Special cases requiring modal handling
            Self::StartReplace | Self::ReplaceNext | Self::ReplaceAll => {
                // These need more complex logic with modals
                // Keep in Editor for now, or extract to separate handler
                todo!("Modal-based commands")
            }

            Self::None => Ok(()),
        }
    }
}
```

---

**Step 5.3: Simplify Editor::handle_key - 1 hour**

**In core.rs, replace 345 lines with:**

```rust
fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
    // Translate Cyrillic to Latin for hotkeys
    let key = crate::keyboard::translate_hotkey(key);

    // Parse key to command
    let cmd = keyboard::EditorCommand::from_key_event(
        key,
        self.config.read_only,
        self.search_state.is_some(),
    );

    // Execute command
    cmd.execute(self)?;

    Ok(())
}
```

**Only ~10 lines!** Down from 345!

---

#### Phase 5 Effort Summary:

| Step | Task | Hours | Complexity |
|------|------|-------|-----------|
| 5.1 | EditorCommand enum + from_key_event | 4-5 | MEDIUM |
| 5.2 | EditorCommand::execute | 3-4 | MEDIUM |
| 5.3 | Simplify handle_key | 1 | LOW |
| Testing | Test all keybindings | 2-3 | - |

**TOTAL: 10-13 hours**

**Result:** core.rs will shrink from ~1,850 to ~1,500 LOC (-19%)

---

### PHASE 6: App Key Handler Decomposition (Per Original Plan)

**Target:** app/key_handler.rs (995 LOC → ~200 LOC)

This is already well-documented in lines 359-442 of original plan.

**Effort:** 9-12 hours total:
- hotkeys.rs: 3-4h
- panel_mgmt.rs: 4-5h
- menu_handler.rs: 2-3h

---

## Final Structure After FULL Decomposition

```
src/panels/editor/
├── mod.rs                        18 LOC ✅
├── core.rs                   ~1,100 LOC ⚠️ (TARGET, from 2,584)
│   ├── Editor struct + fields
│   ├── Constructor methods (new, open_file, from_text)
│   ├── Save methods
│   ├── Helper methods (navigate, prepare_for_navigation, etc.)
│   ├── State methods (get_editor_info, virtual_line_count, etc.)
│   ├── Delegating methods (calls to modules)
│   └── Panel trait impl (now tiny!)
│
├── config.rs                     49 LOC ✅
├── word_wrap.rs                 207 LOC ✅
├── git.rs                       161 LOC ✅
├── search.rs                    142 LOC ✅
├── selection.rs                 124 LOC ✅
├── clipboard.rs                 107 LOC ✅
├── text_editing.rs              139 LOC ✅
│
├── cursor/                      425 LOC ✅
│   ├── mod.rs                    12 LOC
│   ├── physical.rs               91 LOC
│   ├── visual.rs                300 LOC (optimization potential: -50 LOC)
│   └── jump.rs                   22 LOC
│
├── rendering/                  ~830 LOC 🆕 (PHASE 4 TARGET)
│   ├── mod.rs                   100 LOC (orchestrator)
│   ├── context.rs                80 LOC (RenderContext)
│   ├── line_rendering.rs        200 LOC (no wrap)
│   ├── wrap_rendering.rs        250 LOC (with wrap)
│   ├── highlight_renderer.rs    100 LOC (styling)
│   ├── cursor_renderer.rs        40 LOC (cursor)
│   └── deletion_markers.rs       60 LOC (git markers)
│
└── keyboard.rs                 ~400 LOC 🆕 (PHASE 5 TARGET)
    ├── EditorCommand enum
    ├── from_key_event() parser
    └── execute() dispatcher

TOTAL AFTER FULL DECOMPOSITION: ~3,700 LOC
- Largest file: core.rs at ~1,100 LOC (down from 2,584!)
- Average file size: ~185 LOC
- No file > 400 LOC (except core.rs orchestrator at 1,100)
```

---

## Metrics: Journey from 3,374 to 1,100 LOC

### Original (Before Any Decomposition)
- `editor.rs`: **3,374 LOC** 🔴
- Largest method: render_content (718 LOC)
- Second largest: handle_key (590 LOC)

### After Phases 1-3 (Current State)
- `core.rs`: **2,584 LOC** ⚠️ (-790 LOC, -23%)
- Largest method: render_content (732 LOC) 🔴 GREW!
- Second largest: handle_key (345 LOC) ⚠️

**Why render_content grew:** Better implementation, no optimization yet.

### After Phase 4 (Rendering Extraction)
- `core.rs`: **~1,850 LOC** ⚠️ (-734 LOC, -28%)
- Largest method: handle_key (345 LOC)
- rendering/ module: 830 LOC

### After Phase 5 (Keyboard Extraction)
- `core.rs`: **~1,100 LOC** ✅ (-350 LOC, -19%)
- keyboard.rs: 400 LOC
- **Total reduction: 1,484 LOC from original!** (-44%)

### Deduplication Savings
Through smart extraction and deduplication:
- Original: 3,374 LOC
- After phases 1-5: 3,700 LOC across modules
- Net addition: +326 LOC (infrastructure)
- But core.rs reduced by: -2,274 LOC (-67%!)

**The goal is not just to move code, but to improve structure!**

---

## Implementation Timeline

### ✅ COMPLETED (Phases 1-3)
- **Weeks 1-3:** All easy and medium complexity modules extracted
- **Result:** 790 LOC extracted, 15 helper methods created
- **Status:** Tested, working, 69 tests passing

### 🔄 IN PROGRESS (Recommended Order)

#### Week 4-5: Rendering (PRIORITY 1) ⚠️ URGENT
- **Day 1-2:** context.rs + cursor_renderer.rs + deletion_markers.rs (4-6h)
- **Day 3-4:** highlight_renderer.rs (2-3h)
- **Day 5-7:** line_rendering.rs (5-6h)
- **Day 8-10:** wrap_rendering.rs (6-8h) ⚠️ HARDEST
- **Day 11:** mod.rs orchestrator (3-4h)
- **Day 12-14:** Testing, bug fixing, optimization (6-8h)

**TOTAL:** 26-35 hours (2-3 weeks at 10-15h/week)

#### Week 6-7: Keyboard (PRIORITY 2)
- **Day 1-3:** EditorCommand enum + from_key_event (4-5h)
- **Day 4-5:** EditorCommand::execute (3-4h)
- **Day 6:** Simplify handle_key (1h)
- **Day 7:** Testing all keybindings (2-3h)

**TOTAL:** 10-13 hours (1 week at 10-13h)

#### Week 8-9: App Key Handler (OPTIONAL)
- Follow original plan (lines 359-442)
- **TOTAL:** 9-12 hours

---

## Critical Success Factors

### Must-Have:
1. ✅ All 69 existing tests pass after each phase
2. ✅ No new clippy warnings
3. ✅ Manual testing of critical features
4. ✅ Small, atomic git commits

### Should-Have:
1. 🎯 Performance within 5% of baseline (render ~60fps)
2. 🎯 New unit tests for extracted modules
3. 🎯 Code coverage maintained
4. 🎯 Clear module documentation

### Nice-to-Have:
1. 💡 Benchmark rendering performance
2. 💡 Integration tests for complex scenarios
3. 💡 Profiling before/after

---

## Risks & Mitigation (Updated)

### Risk 1: Rendering Extraction Complexity 🔴 HIGH

**Why risky:**
- 732 LOC of intricate, tightly-coupled code
- Touches all editor state (buffer, cursor, selection, search, viewport, git, highlights)
- Word wrap logic is complex (smart vs simple)
- Critical path - any bug = broken editor

**Mitigation:**
1. Extract in order: simple → complex
2. Start with isolated pieces (cursor_renderer, deletion_markers)
3. Test after EACH extraction (not at the end!)
4. Keep old code until new code is proven
5. Manual testing checklist:
   - Word wrap on/off
   - Smart wrap with syntax highlighting
   - Long lines (>1000 chars)
   - Unicode content (emojis, Cyrillic, etc.)
   - Selection + search simultaneously
   - Git diff markers
   - Cursor positioning
   - Horizontal scrolling
6. Commit after each working submodule
7. If blocked, rollback and retry

### Risk 2: Performance Regression 🟡 MEDIUM

**Why risky:**
- Rendering is called every frame (~60 fps)
- Extra function calls might slow down
- HashMap lookups might add overhead

**Mitigation:**
1. Profile BEFORE extraction (baseline)
2. Profile AFTER each extraction
3. Use `#[inline]` for hot functions
4. Keep search_match_map (O(1) lookup)
5. Avoid unnecessary allocations
6. Benchmark critical sections:
   ```bash
   cargo bench --bench render_benchmark
   ```
7. Target: <5% slowdown (acceptable)
8. If >10% slowdown: optimize or reconsider

### Risk 3: State Management Issues 🟡 MEDIUM

**Why risky:**
- Rendering needs mutable access to editor.cached_* fields
- Borrow checker might complain
- Need careful API design

**Mitigation:**
1. Pass &mut Editor to render functions
2. Use RenderContext for read-only data
3. Update cached_* fields in orchestrator
4. Consider splitting Editor state if needed
5. Prototype API before full extraction

### Risk 4: Testing Overhead 🟢 LOW

**Why risky:**
- Manual testing is time-consuming
- Hard to test all edge cases

**Mitigation:**
1. Automated tests where possible:
   - Unit tests for RenderContext
   - Unit tests for highlight_renderer
   - Unit tests for deletion_markers
2. Visual regression testing (screenshots?)
3. Test matrix:
   - OS: Linux, macOS, Windows
   - Terminals: gnome-terminal, alacritty, konsole
   - Content: ASCII, Unicode, long lines, empty files
   - Features: word wrap, git diff, syntax, search

---

## Next Steps (Action Items)

### IMMEDIATE (This Session) ✅
1. ✅ Update this plan document
2. ✅ Commit updated plan

### NEXT SESSION (Phase 4.1)
1. Create `src/panels/editor/rendering/context.rs`
2. Implement RenderContext struct
3. Implement RenderContext::prepare()
4. Implement build_search_match_map()
5. Test compilation
6. Commit

### THEN (Phase 4.2-4.3)
1. Extract cursor_renderer.rs (easy win!)
2. Extract deletion_markers.rs (reusable!)
3. Test both
4. Commit

### CONTINUE (Phase 4.4-4.7)
Follow detailed plan above, one step at a time.

---

## Questions to Consider

### Technical Decisions:

1. **RenderContext ownership:**
   - Pass by reference? `&RenderContext`
   - Or pass fields individually?
   - **Recommendation:** Pass &RenderContext - cleaner API

2. **Mutable vs immutable rendering:**
   - Can we make rendering fully immutable?
   - Or must we update editor.cached_* fields?
   - **Current:** Need &mut for cached fields, but could refactor

3. **Error handling:**
   - Rendering currently doesn't return Result
   - Should we add Result<()> for robustness?
   - **Recommendation:** Keep as-is (rendering can't fail)

4. **Testing strategy:**
   - Unit tests for each module?
   - Integration tests for full rendering?
   - Visual regression tests?
   - **Recommendation:** All three!

### Project Management:

1. **Pacing:**
   - Do phases 4-5 back-to-back? (5-6 weeks)
   - Or take breaks between phases?
   - **Recommendation:** Phase 4 first, evaluate, then Phase 5

2. **Code review:**
   - Review after each submodule?
   - Or after complete phase?
   - **Recommendation:** Review after wrap_rendering (hardest part)

3. **Documentation:**
   - Write docs during extraction?
   - Or after completion?
   - **Recommendation:** Write as you go

---

## Conclusion

### Current State Summary:
- ✅ Phases 1-3 DONE: 790 LOC extracted, 15 modules created
- ⚠️ Phase 4 PARTIAL: Only constants extracted, 732 LOC remain!
- ❌ Phase 5 NOT STARTED: 345 LOC still in handle_key

### Critical Problem:
**core.rs is STILL a monsterfile at 2,584 LOC!**
- 42% of the file is just 2 methods (render_content + handle_key)
- This is unsustainable and blocks further development

### The Path Forward:
1. **PHASE 4 (Rendering):** 26-35 hours → core.rs to 1,850 LOC
2. **PHASE 5 (Keyboard):** 10-13 hours → core.rs to 1,100 LOC
3. **TOTAL:** 36-48 hours of focused work

### Expected Outcome:
- core.rs: 3,374 → 1,100 LOC (-67%!)
- Well-structured rendering module (~830 LOC)
- Clean keyboard command system (~400 LOC)
- Maintainable, testable, documented code

### Why This Matters:
- **Maintainability:** Easier to find and fix bugs
- **Performance:** Easier to optimize specific parts
- **Features:** Easier to add new rendering modes or commands
- **Testing:** More unit testable components
- **Onboarding:** New contributors can understand modules faster

**Let's finish what we started! Phase 4 rendering decomposition is the priority.**

---

*Last updated: 2025-12-05*
*Next milestone: Phase 4.1 (RenderContext extraction)*
