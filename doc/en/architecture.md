# Architecture

This document describes the technical architecture of TermIDE.

## High-Level Overview

TermIDE is a terminal-based IDE built with Rust using the `ratatui` TUI framework. It features an innovative **accordion panel layout system** that adapts to terminal width and allows efficient multi-panel workflows.

```
┌─────────────────────────────────────────────────────────┐
│ Menu Bar     [CPU] [RAM] [Clock]                        │
├───────────────────┬─────────────────────────────────────┤
│ ┌[X][▼] Files ──┐ │ ┌[X][▼] Editor: main.rs ──────────┐│
│ │               │ │ │                                  ││
│ │ src/          │ │ │  fn main() {                     ││
│ │ tests/        │ │ │      // code here                ││
│ │ Cargo.toml    │ │ │  }                               ││
│ │               │ │ │                                  ││
│ └───────────────┘ │ └──────────────────────────────────┘│
│ ─[X][▶] Terminal─ │ ─[X][▶] Log ────────────────────────│
├───────────────────┴─────────────────────────────────────┤
│ Status: file.rs:42  Ln 10, Col 5        Disk: 83%      │
└─────────────────────────────────────────────────────────┘
```

## Core Architectural Components

### 1. Layout System

#### 1.1 LayoutManager

**Location:** `src/layout_manager.rs`

The `LayoutManager` is the heart of the accordion layout system. It manages:

**Components:**
- `panel_groups: Vec<PanelGroup>` - Horizontal arrangement of panel groups
- `focus: usize` - Current focus (index of active panel group)

**Key Responsibilities:**
- Adding panels with automatic stacking based on width threshold
- Managing horizontal navigation (Alt+Left/Right)
- Managing vertical navigation within groups (Alt+Up/Down)
- Smart panel stacking/unstacking (Alt+Backspace)
- Closing panels and cleaning up empty groups

**Focus Management:**
Focus is a simple `usize` index indicating which panel group is currently active. The focused group receives keyboard/mouse input and is highlighted in the UI.

#### 1.2 PanelGroup

**Location:** `src/panels/panel_group.rs`

A `PanelGroup` represents a vertical stack of panels with accordion behavior.

**Structure:**
```rust
pub struct PanelGroup {
    panels: Vec<Box<dyn Panel>>,  // Panels in this group
    expanded_index: usize,         // Which panel is expanded
    horizontal_weight: u16,        // Layout weight (default 100)
}
```

**Accordion Behavior:**
- Exactly one panel is expanded (shows full content)
- Other panels are collapsed to title bar only
- Click title bar [▼]/[▶] button to expand/collapse
- Alt+Up/Down navigates between panels in group

**Key Operations:**
- `add_panel()` - Add panel to group
- `remove_panel()` - Remove panel (resets expanded_index if needed)
- `set_expanded()` - Change which panel is expanded
- `next_panel()` / `prev_panel()` - Cycle through panels

#### 1.3 Automatic Stacking

When adding a new panel via `LayoutManager::add_panel()`:

```rust
let new_width_if_split = available_width / (num_groups + 1);

if new_width_if_split < config.min_panel_width {
    // Stack vertically in current group (accordion)
    active_group.add_panel(panel);
} else {
    // Create new horizontal group
    let new_group = PanelGroup::new(panel);
    panel_groups.push(new_group);
}
```

**Default threshold:** `min_panel_width = 80` characters

This ensures panels always have enough space to be usable.

### 2. Panel System

#### 2.1 Panel Trait

**Location:** `src/panels/mod.rs`

All panels implement the `Panel` trait, which defines the interface for interactive terminal panels:

```rust
pub trait Panel {
    /// Render panel content
    fn render(
        &mut self,
        area: Rect,                // Available rendering area
        buf: &mut Buffer,          // Ratatui buffer
        is_focused: bool,          // Is this panel focused?
        panel_index: usize,        // Panel index for identification
        state: &AppState,          // Shared application state
    );

    /// Handle keyboard input
    fn handle_key(&mut self, key: KeyEvent) -> Result<()>;

    /// Handle mouse input
    fn handle_mouse(&mut self, mouse: MouseEvent, panel_area: Rect) -> Result<()>;

    /// Get panel title (shown in header)
    fn title(&self) -> String;

    /// Check if this is a welcome panel (auto-closes on other panel open)
    fn is_welcome_panel(&self) -> bool { false }

    /// Get file to open (for panels that request file opening)
    fn take_file_to_open(&mut self) -> Option<PathBuf> { None }

    /// Get working directory for new panels
    fn get_working_directory(&self) -> Option<PathBuf> { None }

    /// Get modal request (for panels that open modals)
    fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> { None }
}
```

#### 2.2 Panel Implementations

**FileManager** (`src/panels/file_manager/mod.rs`)
- Browse files and directories
- File operations (create, delete, copy, move)
- Git status integration
- Clipboard support
- Batch operations
- Drag-and-drop selection

**Editor** (`src/panels/editor.rs`)
- Text editing with undo/redo
- Syntax highlighting via tree-sitter (15+ languages)
- Search and replace with interactive modals
- Line numbers, cursor position
- File saving
- Configurable tab size

**TerminalPty** (`src/panels/terminal_pty.rs`)
- Full PTY (pseudo-terminal) support
- Shell integration
- Scrollback buffer
- ANSI color support
- Resize handling

**Log** (`src/panels/debug.rs`)
- Application state inspection
- Log viewer
- Panel information
- System resource monitoring

**Welcome** (`src/panels/welcome.rs`)
- Shows when no panels are open
- Displays help text
- Auto-closes when other panel opens

### 3. Event Handling

#### 3.1 Event Loop

**Location:** `src/app/mod.rs`

Main event loop structure:

```rust
while !state.should_quit {
    match event_handler.next()? {
        Event::Key(key) => self.handle_key_event(key)?,
        Event::Mouse(mouse) => self.handle_mouse_event(mouse)?,
        Event::Resize(w, h) => state.update_terminal_size(w, h),
        Event::Tick => {
            // Periodic updates
            self.update_panels_tick()?;
            self.system_monitor.update(&mut self.state);
        }
    }
    self.render(terminal)?;
}
```

**Event Types:**
- **Key** - Keyboard input (hotkeys, text input)
- **Mouse** - Mouse clicks, drags, scroll
- **Resize** - Terminal size change
- **Tick** - Periodic timer (resource monitoring, panel updates)

#### 3.2 Key Handler

**Location:** `src/app/key_handler.rs`

Handles keyboard input with priority:

1. **Modal captures input first** (if open)
2. **Global hotkeys** (Alt+M, Alt+H, Alt+Q, etc.)
3. **Panel management** (Alt+Left/Right, Alt+Up/Down, Alt+X, etc.)
4. **Active panel** (via `panel.handle_key()`)

**Cyrillic Support:**
Keyboard layout translation via `crate::keyboard::translate_hotkey()` allows hotkeys to work with Russian keyboard layout.

#### 3.3 Mouse Handler

**Location:** `src/app/mouse_handler.rs`

Handles mouse input:

**Panel Title Bar:**
- Click `[X]` button → Close panel
- Click `[▼]` button → Collapse panel (expand next)
- Click `[▶]` button → Expand panel

**Panel Content:**
- Clicks forwarded to `panel.handle_mouse()`
- Each panel handles its own mouse interactions

**Menu Bar:**
- Click menu items to activate

#### 3.4 Modal Handler

**Location:** `src/app/modal_handler.rs` and `src/app/modal/`

Handles interactive modal dialogs:

**Modal Types:**
- **Input** - Text input (file name, directory name, etc.)
- **Confirm** - Yes/No confirmation
- **Select** - Choose from options
- **Batch** - Multi-item operations (copy, move, delete)

**Input Capture:**
When modal is open, keyboard input goes to modal first. Escape closes modal.

### 4. Rendering Pipeline

#### 4.1 Main Rendering

**Location:** `src/ui/layout.rs`

Rendering flow:

```rust
fn render_layout_with_accordion(frame, layout_manager, state) {
    // 1. Calculate horizontal layout for all panel groups
    let horizontal_chunks = calculate_horizontal_layout();

    // 2. Render panel groups
    for group in groups {
        let vertical_chunks = calculate_vertical_layout(group);

        // 3. Render each panel (expanded or collapsed)
        for panel in group {
            if is_expanded {
                render_expanded_panel(panel, area, ...);
            } else {
                render_collapsed_panel(panel, area, ...);
            }
        }
    }

    // 4. Render modal (if open)
    if let Some(modal) = state.active_modal {
        render_modal(modal, ...);
    }
}
```

#### 4.2 Panel Rendering

**Location:** `src/ui/panel_rendering.rs`

**Expanded Panel:**
- Border with `[X][▼]` buttons and title
- Full content area
- Scrollable if content exceeds area

**Collapsed Panel:**
- Title bar only: `─[X][▶] Title ─────`
- Takes minimal vertical space (1 line)
- Clicking expands

**Border Rendering:**
Borders and buttons are drawn by `panel_rendering.rs`, then panel's `render()` method draws content in the inner area.

### 5. State Management

#### 5.1 AppState

**Location:** `src/state.rs`

Central state container:

```rust
pub struct AppState {
    pub theme: Theme,                    // Current theme
    pub terminal: TerminalInfo,          // Width, height
    pub config: Config,                  // User configuration
    pub should_quit: bool,               // Exit flag
    pub batch_operation: Option<BatchOp>, // Pending batch ops
    pub active_modal: Option<ActiveModal>, // Current modal
    pub error_message: Option<String>,   // Error to display
    pub fs_watcher: Option<Watcher>,     // File system watcher
    // ... other fields
}
```

**Thread Safety:**
Most state is single-threaded (TUI runs on main thread). File system watcher uses channels for cross-thread communication.

#### 5.2 Configuration

**Location:** `src/config.rs`

User configuration loaded from TOML:

```rust
pub struct Config {
    pub theme: String,                    // Theme name
    pub tab_size: usize,                  // Editor tab size
    pub language: String,                 // UI language (auto/en/ru)
    pub min_panel_width: u16,             // Stacking threshold
    pub resource_monitor_interval: u64,   // Update interval (ms)
    pub log_file_path: Option<String>,    // Custom log path
}
```

**Default Locations:**
- Linux: `~/.config/termide/config.toml`
- macOS: `~/Library/Application Support/termide/config.toml`
- Windows: `%APPDATA%\\termide\\config.toml`

### 6. Theme System

**Location:** `src/theme.rs`

**Built-in Themes:** 12 themes (Dracula, Nord, Monokai, etc.)

**Custom Themes:** Load from `~/.config/termide/themes/*.toml`

**Theme Structure:**
```rust
pub struct Theme {
    pub fg: Color,                // Foreground
    pub bg: Color,                // Background
    pub accented_fg: Color,       // Focused elements
    pub disabled: Color,          // Disabled/unfocused
    pub selected_bg: Color,       // Selection background
    // ... syntax highlighting colors
}
```

**Loading Priority:**
1. User themes (in config dir)
2. Built-in themes
3. Fallback to default

### 7. Internationalization

**Location:** `src/i18n/`

Language support via compile-time string constants:

```rust
pub struct I18n {
    pub menu_files: &'static str,
    pub menu_terminal: &'static str,
    // ... all UI strings
}
```

**Languages:**
- English (`en.rs`)
- Russian (`ru.rs`)

**Detection:**
1. `config.language` setting
2. `TERMIDE_LANG` environment variable
3. `LANG` / `LC_ALL` system variables
4. Default to English

### 8. Key Dependencies

**Ratatui** - Terminal UI framework
- Widget-based rendering
- Buffer system for efficient updates
- Layout system (Rect, Constraints)

**Crossterm** - Cross-platform terminal manipulation
- Event handling (keyboard, mouse, resize)
- Terminal control (cursor, colors, clear)
- Raw mode management

**Tree-sitter** - Syntax highlighting
- Parser generators for 15+ languages
- Incremental parsing for performance
- Query system for syntax highlighting

**Ropey** - Text buffer
- Efficient line-based text storage
- UTF-8 aware
- Gap buffer internally

**Portable-pty** - PTY implementation
- Cross-platform pseudo-terminal
- Shell integration
- Resize support

**Sysinfo** - System monitoring
- CPU usage
- Memory usage
- Disk space

## Design Decisions

### Why Accordion Layout?

**Problem:** Terminal space is limited, multi-panel IDEs often feel cramped.

**Solution:** Accordion layout maximizes usable space:
- One expanded panel per group gets full vertical space
- Other panels collapse to title bar (1 line)
- Quick access via Alt+Up/Down or mouse click
- Automatic stacking when terminal is too narrow

### Why Dynamic Panels?

**Benefit:** Users can open as many panels as needed:
- Multiple editors for different files
- Multiple terminals for different tasks
- Multiple file managers for different directories

**Challenge:** Managing many panels efficiently
- Accordion prevents clutter
- Hotkeys provide fast navigation
- Welcome screen auto-closes

### Why Trait-Based Panels?

**Flexibility:** New panel types can be added without changing core code
- Implement `Panel` trait
- Add to panel creation logic
- Works with existing layout system

**Polymorphism:** `Box<dyn Panel>` allows heterogeneous collections
- Single `Vec<Box<dyn Panel>>` holds all panel types
- Uniform rendering and event handling
- Dynamic dispatch overhead is negligible for TUI

## Performance Characteristics

**Rendering:** O(n) where n = number of visible panels
- Only expanded panels render full content
- Collapsed panels render single line

**Event Handling:** O(1) for most operations
- Direct index access to focused panel
- Hash map lookups for key bindings

**Memory:** Linear with panel count
- Each panel owns its state
- Shared AppState is small
- No excessive cloning (uses references)

**File Operations:** Asynchronous where possible
- FS watcher uses separate thread
- Debouncing prevents excessive updates

## Future Architecture Considerations

**Potential Improvements:**

1. **Async Panels**
   - Long-running operations (search, compile) don't block UI
   - Background tasks with progress indicators

2. **Plugin System**
   - Load panels dynamically
   - User-defined panel types
   - Script integration (Lua, Python)

3. **Session Management**
   - Save/restore panel layout
   - Project workspaces
   - Recent files/directories

4. **Network Panels**
   - SSH terminal panels
   - Remote file browsers
   - Collaborative editing

## Debugging Architecture

**Log System:**
- All logs written to `termide.log` in config directory
- Levels: INFO, ERROR, DEBUG
- Timestamp and component prefixes
- Rotate logs to prevent unbounded growth

**Debug Panel:**
- Live view of application state
- Recent log entries
- Panel inspection
- Performance metrics

**Panic Handling:**
- Restore terminal on panic
- Write panic info to log
- Show error message to user

## Security Considerations

**Terminal Injection:**
- ANSI escape sequences filtered in terminal panel
- User input sanitized before shell execution

**File Operations:**
- Symlink attacks prevented
- Path traversal checks
- Permission checks before operations

**Resource Limits:**
- File size limit (100 MB) for editor
- Scrollback buffer limit for terminal
- Log rotation to prevent disk exhaustion

## Conclusion

TermIDE's architecture prioritizes:
- **Flexibility** - Dynamic panel system adapts to user needs
- **Efficiency** - Accordion layout maximizes usable space
- **Extensibility** - Trait-based design allows easy additions
- **Robustness** - Defensive programming prevents crashes
- **Performance** - Efficient rendering and event handling

The accordion layout system is the key innovation that differentiates TermIDE from traditional multi-panel terminal applications.
