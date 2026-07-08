# Architecture

This document describes the technical architecture of TermIDE.

## High-Level Overview

TermIDE is a terminal-based IDE built with Rust using the `ratatui` TUI framework. It features an adaptive **split panel layout** that resizes to terminal width and gives every stacked panel an independently adjustable height — with a one-key fullscreen preset for the focused panel.

```
┌─────────────────────────────────────────────────────────┐
│ Menu Bar     [CPU] [RAM] [Clock]                        │
├───────────────────┬─────────────────────────────────────┤
│ ┌[≡] 📁 Files ──┐ │ ┌[≡] 📝 Editor: main.rs ──────────┐│
│ │ src/          │ │ │                                  ││
│ │ tests/        │ │ │  fn main() {                     ││
│ │ Cargo.toml    │ │ │      // code here                ││
│ │               │ │ │  }                               ││
│ ├[≡] 💻 Terminal┤ │ │                                  ││
│ │ $ cargo build │ │ │                                  ││
│ │ Compiling...  │ │ │                                  ││
│ └───────────────┘ │ └──────────────────────────────────┘│
├───────────────────┴─────────────────────────────────────┤
│ Status: file.rs:42  Ln 10, Col 5        Disk: 83%      │
└─────────────────────────────────────────────────────────┘
```

The left column above shows two stacked panels sharing the column with their own heights; on the right, a single panel takes the full column. Pressing `Alt+F11` collapses every non-focused panel in the active column to its title row (one-row preset), pressing it again restores the previous heights.

## Core Architectural Components

### 1. Layout System

#### 1.1 LayoutManager

**Location:** `crates/layout/src/lib.rs`

The `LayoutManager` owns the split-layout state. It manages:

**Components:**
- `panel_groups: Vec<PanelGroup>` - Horizontal arrangement of panel groups
- `focus: usize` - Current focus (index of active panel group)

**Key Responsibilities:**
- Adding panels with automatic stacking based on width threshold
- Managing horizontal navigation (Alt+Left/Right)
- Managing vertical focus within a group (Alt+Up/Down)
- Smart panel stacking/unstacking between groups (Alt+Backspace, F11)
- Proportional width redistribution when the terminal resizes
- Closing panels and cleaning up empty groups

**Focus Management:**
Focus is a simple `usize` index indicating which panel group is currently active. The focused group receives keyboard/mouse input and is highlighted in the UI.

#### 1.2 PanelGroup

**Location:** `crates/layout/src/panel_group.rs`

A `PanelGroup` represents a vertical split of panels sharing one column.

**Structure:**
```rust
pub struct PanelGroup {
    panels: Vec<Box<dyn Panel>>,         // Panels in this group
    expanded_index: usize,               // Focused panel (active border)
    pub width: Option<u16>,              // Column width (None = auto-distribute)
    split_heights: Option<Vec<u16>>,     // Cached per-panel heights
    fullscreen_cache: Option<Vec<u16>>,  // Saved heights from before
                                         // entering the fullscreen preset
}
```

**Layout behaviour:**
- Every panel in the group is visible. The minimum height per panel is one row (the title bar).
- `split_heights` is the per-panel height cache. `None` means "no cache — derive equal distribution on first use". Heights are rescaled proportionally whenever the column height changes.
- Pressing `Alt+F11` (or the bound `toggle_fullscreen_panel` action) toggles a fullscreen preset: the focused panel takes the column's full height, the others collapse to one row each. The pre-toggle heights are stashed in `fullscreen_cache` so a second press restores them.
- While the preset is active, switching focus with `Alt+Up` / `Alt+Down` (`prev_panel` / `next_panel`) re-applies the preset to the new focus — visually identical to a classic accordion view.
- `Alt+Shift+=` / `Alt+Shift+-` (`panel_grow_vertical` / `panel_shrink_vertical`) grow / shrink the focused panel by 1 row, taking from / giving to a neighbour with available room (cascade).

**Key Operations:**
- `add_panel()` / `insert_panel()` / `remove_panel()` - mutate panels and rebalance the height caches.
- `set_expanded()` / `next_panel()` / `prev_panel()` - move focus; re-apply the preset when active.
- `toggle_fullscreen()` - turn the preset on or off.
- `grow_focused()` / `shrink_focused()` - adjust the focused panel's height.
- `resize_panel_divider()` - apply a delta to the divider above a given panel (used by the mouse drag handler).

#### 1.3 Automatic Stacking

When adding a new panel via `LayoutManager::add_panel()`:

```rust
let new_width_if_split = available_width / (num_groups + 1);

if new_width_if_split < config.min_panel_width {
    // Stack vertically in the active group (split with cached heights)
    active_group.add_panel(panel);
} else {
    // Create a new horizontal group
    let new_group = PanelGroup::new(panel);
    panel_groups.push(new_group);
}
```

**Default threshold:** `min_panel_width = 80` characters

This ensures panels always have enough space to be usable.

### 2. Panel System

#### 2.1 Panel Trait

**Location:** `crates/core/src/lib.rs`

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

**FileManager** (`crates/panel-file-manager/src/lib.rs`)
- Browse files and directories
- File operations (create, delete, copy, move)
- Git status integration
- Clipboard support
- Batch operations
- Drag-and-drop selection

**Editor** (`crates/panel-editor/src/lib.rs`)
- Text editing with undo/redo
- Syntax highlighting via tree-sitter (22 languages)
- Search and replace with inline find bars
- Line numbers, cursor position, word wrap
- Word navigation (Ctrl+Left/Right), paragraph/symbol navigation (Ctrl+Up/Down)
- Auto-indentation with split-bracket indent
- Auto-close brackets and quotes
- Git diff visualization in line numbers
- LSP integration (completion, hover, go to definition)
- File saving with Save As and executable checkbox

**Terminal** (`crates/panel-terminal/src/lib.rs`)
- Full PTY (pseudo-terminal) support
- Shell integration
- Scrollback buffer
- Text search across scrollback and visible buffer (`Searchable` trait)
- ANSI color support
- Resize handling

**Journal** (`crates/panel-misc/src/journal.rs`)
- Application log viewer
- Panel information
- System resource monitoring

**GitStatus** (`crates/panel-git-status/src/lib.rs`)
- Repository status overview
- File staging/unstaging
- Branch switching
- Commit creation

**GitLog** (`crates/panel-git-log/src/lib.rs`)
- Commit history with ASCII graph
- Diff viewing
- Commit hash copying

**GitDiff** (`crates/panel-git-diff/src/lib.rs`)
- Side-by-side or inline diff view
- Syntax-highlighted diffs

**Diagnostics** (`crates/panel-diagnostics/src/lib.rs`)
- LSP diagnostics display
- Error/warning navigation

**Operations** (`crates/panel-operations/src/lib.rs`)
- Background file operation tracking
- Progress display for copy/move/delete

**Outline** (`crates/panel-outline/src/lib.rs`)
- Structural code navigation with tree-sitter queries
- Symbol list synced with active editor
- Navigate to symbol on Enter
- Cursor tracking and live updates

**Image** (`crates/panel-image/src/lib.rs`)
- Native image rendering (Kitty, iTerm2, Sixel protocols)
- Fallback to Unicode block characters

**Help** (`crates/panel-misc/src/help.rs`)
- Dynamic help content generated from keybindings config
- Pseudo-graphic tables with full-width layout
- Scrollable with keyboard and mouse
- Auto-closes when other panel opens

### 3. Event Handling

#### 3.1 Event Loop

**Location:** `crates/app/src/app/mod.rs`

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

**Location:** `crates/app/src/app/key_handler.rs`

Handles keyboard input with priority:

1. **Modal captures input first** (if open)
2. **Global hotkeys** (Alt+M, Alt+H, Alt+Q, etc.)
3. **Panel management** (Alt+Left/Right, Alt+Up/Down, Alt+X, etc.)
4. **Active panel** (via `panel.handle_key()`)

**Cyrillic Support:**
Keyboard layout translation via `termide_keyboard::translate_hotkey()` allows hotkeys to work with Russian keyboard layout.

#### 3.3 Mouse Handler

**Location:** `crates/app/src/app/mouse_handler.rs`

Handles mouse input:

**Panel Title Bar:**
- Click `[≡]` button → Open panel action context menu (Close / Split / Merge / Move)
- Click title area → Activate panel (double-click on file manager → directory picker)
- **Drag title area** → Two-mode gesture:
  - Drop inside the source group's column → vertical resize (the divider above the dragged panel snaps to the cursor).
  - Drop in another column or between columns → panel move: ghost follows cursor, drop zone is highlighted; release over another panel's header inserts into that group, between groups creates a new one. `Escape` cancels.

**Panel Content:**
- Clicks forwarded to `panel.handle_mouse()`
- Each panel handles its own mouse interactions

**Menu Bar:**
- Click menu items to activate

#### 3.4 Modal Handler

**Location:** `crates/app/src/app/modal_handler.rs` and `crates/modal/src/`

Handles interactive modal dialogs:

**Modal Types** (crate `termide-modal`):
- **Input** — text input (file name, directory name, etc.)
- **Confirm** — Yes/No confirmation
- **Select** / **EditableSelect** — choose from options (with optional editing)
- **Choice** — horizontal choice buttons
- **Info** — informational display with **scrollable content** (script reports, system info); scrollbar on the right border, `↑↓/PageUp/PageDown/Home/End` and mouse-wheel
- **InfoAction** — info window with extra action buttons
- **Settings** — full-screen configuration modal with **sidebar layout**. Split into submodules under `crates/modal/src/settings/`:
  - `settings.rs` — `SettingsModal` struct, rendering, key/mouse handling
  - `settings/fields.rs` — declarative field data (`FieldType`, `FieldDescriptor`, `ContentRow`, helpers `fields_for_tab`, `get_field_value`, `toggle_field`, `cycle_enum_*`)
  - `settings/kb.rs` — keybinding tables and macros (`kb_get!`/`kb_set!`, `KB_SECTIONS`, `kb_binding_names`, `get/set_kb_value`, `format_key_event`)
- **Progress** — progress bar for long-running operations
- **Commit** / **Conflict** / **RenamePattern** / **Sessions** / **DirectoryPicker** / **SaveAs** / **BookmarkAdd** / **Calendar** / **CommandPalette** / **ScriptCreate** — specialised dialogs for individual operations

Shared helpers live in `crates/modal/src/base.rs` (`render_modal_block`, `render_modal_frame`, `button_style`, the `CursorNavigation` trait).

**Input Capture:**
When modal is open, keyboard input goes to modal first. Escape closes modal.

### 4. Rendering Pipeline

#### 4.1 Main Rendering

**Location:** `crates/ui-render/src/layout.rs`

Rendering flow:

```rust
fn render_main_area(frame, layout_manager, state) {
    // 1. Compute column widths (proportional, with min_panel_width floor).
    let horizontal_chunks = calculate_horizontal_layout();

    // 2. For each column, drive vertical constraints from the group's
    //    cached split heights (or equal distribution as a fallback).
    for group in groups {
        let vertical_chunks =
            termide_layout::compute_vertical_constraints(group, area_height);

        // 3. Render each panel. Panels with height >= 2 render their
        //    full content + complete border (top, sides, AND bottom);
        //    height==1 panels fall back to header-only rendering.
        let mut prev_was_accordion = false;
        for (idx, panel) in group.panels().enumerate() {
            let area = vertical_chunks[idx];
            let omit_bottom_border = area.height < 2;
            if !omit_bottom_border {
                render_expanded_panel(panel, area, omit_bottom_border, ...);
            } else {
                render_collapsed_panel(panel, area, ...);
            }
            // Top-row corners chosen by context: └┘ for the last
            // panel when collapsed (group closes off visually),
            // ├┤ when both this and the previous panel are
            // accordions (continuity), otherwise ┌┐.
            patch_top_corners(area, idx, prev_was_accordion, &group);
            prev_was_accordion = area.height < 2;
        }
    }

    // 4. Render modal (if open).
    if let Some(modal) = state.active_modal {
        render_modal(modal, ...);
    }
}
```

Every panel with `height >= 2` draws its own complete border; two adjacent panels show two consecutive border rows (bottom of upper + top of lower) so the focused panel is fully framed in its accent colour on all four sides. Accordion-collapsed `height == 1` panels keep top-only borders, with their top corners switching between `┌┐`, `├┤`, and `└┘` depending on what sits above and whether they are the last panel in the group.

#### 4.2 Panel Rendering

**Location:** `crates/ui-render/src/panel.rs`

**Full panel (height ≥ 2 rows):**
- Border with the `[≡]` action button and emoji + title (e.g. `[≡] 📁 Files`)
- Full content area
- Scrollable if content exceeds area
- The bottom border is always rendered for `height >= 2` panels; between two adjacent panels you see two consecutive border rows so the focused panel is fully framed in its accent colour on all four sides

**Collapsed panel (height = 1 row, only when shrunk to the minimum):**
- Title bar only: `─[≡] 📁 Files ─────`
- Takes one row
- Click the title to focus the panel; press `Alt+F11` or `Alt+Shift+=` to grow it

**Icon Mode:**
Panel titles show emoji icons based on panel type (📁 file manager, 💻 terminal, 📝 editor, 🔄 operations, 🚧 diagnostics, 📑 outline, 🎨 image, etc.). All icons are chosen to be reliably 2-cells wide in modern terminals so the title alignment after the icon stays stable. Icon mode is configured via `icon_mode` in `[general]` settings:
- `auto` (default) — emoji if terminal supports it, plain `[≡]` otherwise
- `emoji` — always show emoji icons
- `unicode` — no icons, no arrows, just `[≡]`

**Drag Overlay:**
When a panel is being dragged by its top border, `render_drag_overlay()` (in `src/ui.rs`) runs after the main panel render and before dropdowns/modals. The intent (`PanelDragIntent::ResizeAbove`, `PanelDragIntent::Move { target: IntoGroup, drop_y }`, or `PanelDragIntent::Move { target: NewGroup, .. }`) drives what is drawn: a thick `━` line at `drop_y` for `ResizeAbove` and `IntoGroup` (the prospective divider / split row), or a thick `┃` line at the gutter column for `NewGroup`. No ghost icon — the user reads the operation from the line shape and position. Hit-testing reuses `calculate_panel_rects` / `classify_panel_drag` / `compute_drop_target` from `termide_layout` so the mouse handler and the renderer agree on geometry. A separate `render_v_divider_ghost` overlay handles the in-group bottom-border drag (`Vertical­Divider­Drag­State`), drawing a `━` line across the affected group's width.

**Border Rendering:**
Borders and buttons are drawn by `panel_rendering.rs`, then panel's `render()` method draws content in the inner area.

### 5. State Management

#### 5.1 AppState

**Location:** `crates/state/src/` (split into `batch.rs`, `layout.rs`, `operations.rs`, `pending_action.rs`, `ui.rs`)

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

**Location:** `crates/config/src/lib.rs`

User configuration loaded from TOML:

```rust
pub struct Config {
    pub general: GeneralSettings,         // Theme, language, icon_mode, vim_mode, keybindings
    pub editor: EditorSettings,           // Tab size, word wrap, git diff, auto-indent
    pub file_manager: FileManagerSettings, // Extended view width, keybindings
    pub git_status: GitStatusSettings,    // Keybindings
    pub terminal: TerminalSettings,       // Keybindings
    pub lsp: LspSettings,                // LSP servers, completion, hover
    pub logging: LoggingSettings,         // Log level, resource monitor interval
    pub vfs: VfsSettings,                // VFS connection timeout
}
```

**Default Locations:**
- Linux: `~/.config/termide/config.toml`
- macOS: `~/Library/Application Support/termide/config.toml`
- Windows: `%APPDATA%\\termide\\config.toml`

### 6. Theme System

**Location:** `crates/theme/src/lib.rs`

**Built-in Themes:** 38 themes (Dracula, Nord, Monokai, Matrix, Pip-Boy, etc.)

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

**Location:** `crates/i18n/`

Language support via TOML-based translation files loaded at compile time:

```
crates/i18n/
├── src/
│   ├── lib.rs      # Translation trait and runtime
│   └── runtime.rs  # Language detection and loading
└── i18n/           # Translation files
    ├── bn.toml     # Bengali
    ├── de.toml     # German
    ├── en.toml     # English
    ├── es.toml     # Spanish
    ├── fr.toml     # French
    ├── hi.toml     # Hindi
    ├── id.toml     # Indonesian
    ├── ja.toml     # Japanese
    ├── ko.toml     # Korean
    ├── pt.toml     # Portuguese
    ├── ru.toml     # Russian
    ├── th.toml     # Thai
    ├── tr.toml     # Turkish
    ├── vi.toml     # Vietnamese
    └── zh.toml     # Chinese
```

**Languages:** 15 supported (Bengali, Chinese, English, French, German, Hindi, Indonesian, Japanese, Korean, Portuguese, Russian, Spanish, Thai, Turkish, Vietnamese)

**Detection:**
1. `config.language` setting
2. `LANG` / `LC_ALL` system variables
3. Default to English

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
- Parser generators for 22 languages
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

### Why Split Layout with a Fullscreen Preset?

**Problem:** Terminal space is limited and multi-panel IDEs often feel cramped, but binary "one-expanded, rest collapsed" layouts also throw away cases where the user wants to see two or three panels at adjusted heights at once.

**Solution:** Adjustable split layout per group, plus a one-key fullscreen preset:
- Every panel in a group is visible by default; the user picks heights via mouse drag (the panel's bottom border or its title bar), `Alt+Shift+=` / `Alt+Shift+-`, or any combination thereof.
- `Alt+F11` toggles a "fullscreen current panel" preset that mirrors the legacy accordion view (one panel takes the full column, others collapse to one row), with the previous heights stashed for instant restore.
- Heights are cached and rescaled proportionally when the terminal resizes.
- Automatic stacking still kicks in when the terminal is too narrow (`min_panel_width`).

### Why Dynamic Panels?

**Benefit:** Users can open as many panels as needed:
- Multiple editors for different files
- Multiple terminals for different tasks
- Multiple file managers for different directories

**Challenge:** Managing many panels efficiently
- The fullscreen preset gives a clutter-free single-panel view on demand
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

**Rendering:** O(n) where n = number of panels in visible groups
- Panels with height ≥ 2 render their full content; panels collapsed to one row render only the title bar
- Each `height >= 2` panel draws its own bottom border so adjacent panels show two consecutive border rows — the focused panel is framed in its accent colour on all four sides

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

### Async Pipelines

Several startup and hot-path operations that used to block the
render loop now run on worker threads, polled from each panel's
`tick()`. The pattern is the same in every case: spawn a worker,
park a `mpsc::Receiver` on the panel, swap the result in when
`try_recv()` returns. References below point to where each pipeline
lives.

| Pipeline                              | Worker location                                                 | Polled in                                                                          |
|---------------------------------------|-----------------------------------------------------------------|------------------------------------------------------------------------------------|
| FileManager initial directory read    | `crates/panel-file-manager/src/lib.rs` (`start_async_reload`)   | `check_async_reload` from app's per-tick `check_background_panel_updates`          |
| FileManager subtree expand            | `crates/panel-file-manager/src/lib.rs` (`start_listing`)        | `poll_pending_expansions` from same path; placeholder rows show `…` until resolved |
| FileManager per-entry git status      | `crates/panel-file-manager/src/git_status.rs`                   | `check_git_status_async`; `apply_git_statuses` reapplies if dir read raced ahead   |
| Git status / log panel refresh        | `crates/panel-git-status/src/lib.rs`, `panel-git-log/src/lib.rs`| `poll_refresh` in each panel's `tick`                                              |
| Git submodule discovery (RepoManager) | `crates/git/src/repo_manager.rs` (`spawn_submodule_walk`)       | `RepoManager::poll` from git panel `tick`                                          |
| Session restore — panels in parallel  | `crates/app/src/layout_session.rs` (`construct_panel` per panel)| Joined synchronously after spawn so the slowest panel still gates the first frame  |
| Watcher repo registration             | `crates/watcher/src/lib.rs` (`watch_repository`)                | `poll_pending` in app main loop; inotify installs chunked at `INSTALL_CHUNK`/tick  |
| Directory size walk (wide-view)       | `crates/panel-file-manager/src/utils.rs` (`shared_dir_size_cache`) | Per-frame `try_recv` against shared cache; budget enforced per walk             |

The SFTP/FTP backend uses a different pattern — a dedicated tokio
runtime owns the connection and a chunk-as-command actor (see
`crates/vfs/src/sftp.rs`). The sync worker drives the chunk loop and
polls pause/cancel flags between dispatches, so a paused transfer
leaves the actor free to serve other panels' metadata requests.

### 8. Session Management

**Location:** `crates/session/src/lib.rs`

Session persistence allows saving and restoring panel layouts:

**Storage Location:**
- Linux: `~/.local/share/termide/sessions/<project_path>/session.toml`
- macOS: `~/Library/Application Support/termide/sessions/<project_path>/session.toml`

**Features:**
- Automatic session save on exit
- Panel layout restoration on startup
- Session switching via menu (switch between different projects)
- Session retention with automatic cleanup of old sessions

**Session File Format:**
```toml
focused_group = 0

[[panel_groups]]
expanded_index = 1            # focused panel within the group
width = 80                    # column width (None = auto-distribute)
split_heights = [12, 6]       # per-panel heights (omitted when uncached)
fullscreen_cache = [10, 8]    # heights to restore on Alt+F11 toggle-off

[[panel_groups.panels]]
type = "file_manager"
path_or_url = "/home/user/project"

[[panel_groups.panels]]
type = "editor"
path = "/home/user/project/main.rs"
```

A legacy `mode = "accordion"` field is still accepted on read and triggers a one-time migration to the fullscreen preset (current code never writes it).

### 9. VFS (Remote Filesystems)

**Location:** `crates/vfs/src/`

A pure-Rust VFS layer makes remote servers look like local
directories to the rest of the app. No native OpenSSL or libssh —
SFTP runs on `russh` + `russh-sftp`, FTPS on `rustls`. Builds work
statically on Alpine / musl.

**Supported protocols:** `sftp://`, `ftp://`, `ftps://` (URL parsing
also recognises `smb://` / `nfs://` but no provider is shipped yet).

**Key components:**
- **`VfsProvider` trait** (`crates/vfs/src/traits.rs`) — abstract
  filesystem API used by FileManager, file-ops, editor: `list_dir`,
  `read_file`, `write_file`, `delete`, `upload`,
  `upload_with_progress`, etc. Every call returns a `VfsOperation<T>`
  whose receiver the caller polls — there is no blocking variant.
- **`VfsManager`** (`crates/vfs/src/lib.rs`) — provider cache keyed
  by `(scheme, host, port, user)`; evicts entries whose actor died.
- **SFTP actor** (`crates/vfs/src/sftp.rs`) — a single tokio task
  owns the `russh-sftp` session and handles small atomic commands
  (`OpenRead` / `ReadChunk` / `WriteChunk` / `CloseHandle` / `Stat`
  / `ListDir` / `MkdirRecursive` / …). Chunk loops live on the sync
  worker side, polling pause/cancel between dispatches.
- **URL parsing** (`crates/vfs/src/url.rs`) — UTF-8 round-trip with
  percent-decoded paths so non-ASCII filenames survive.
- **Authentication** — SSH agent → `~/.ssh/config` `IdentityFile`
  → default keys (`id_ed25519` / `id_rsa` / `id_ecdsa` / `id_dsa`)
  → password, with all four selectable as explicit `AuthMethod`
  variants for users who don't want the auto chain.

**Cancel safety:** transfers cancel between chunks; partial files
get a "Delete partial upload?" modal so the server doesn't keep
stranded bytes. Same-connection renames stay server-side
(no download-then-upload). See `doc/en/vfs.md` for the user view
and `doc/en/operations.md` for the cancel flow.

## Future Architecture Considerations

**Potential Improvements:**

1. **Async Panels**
   - Long-running operations (search, compile) don't block UI
   - Background tasks with progress indicators

2. **Plugin System**
   - Load panels dynamically
   - User-defined panel types
   - Script integration (Lua, Python)

3. **Network Panels**
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
- **Efficiency** - Adjustable split layout with a one-key fullscreen preset maximises usable space without giving up multi-panel views
- **Extensibility** - Trait-based design allows easy additions
- **Robustness** - Defensive programming prevents crashes
- **Performance** - Efficient rendering and event handling

The split-with-fullscreen layout system is the key innovation that differentiates TermIDE from traditional multi-panel terminal applications.
