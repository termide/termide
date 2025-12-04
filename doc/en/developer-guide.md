# Developer Guide

This guide is for developers who want to contribute to TermIDE or understand its codebase.

## Development Setup

### Prerequisites

- **Rust 1.70+** (stable toolchain)
- **Git** for version control
- **Optional:** Nix with flakes enabled for reproducible builds

### Getting the Source Code

```bash
git clone https://github.com/termide/termide.git
cd termide
```

### Building

#### With Cargo (Standard)

```bash
# Development build
cargo build

# Release build with optimizations
cargo build --release

# Run in development mode
cargo run

# Run in release mode
cargo run --release
```

#### With Nix (Reproducible)

```bash
# Enter development shell with all dependencies
nix develop

# Build with Nix
nix build

# Run checks
nix flake check
```

### Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_name
```

### Code Quality Checks

```bash
# Check for compilation errors
cargo check

# Run clippy linter
cargo clippy

# Format code
cargo fmt

# Check formatting without modifying files
cargo fmt --check
```

## Project Structure

```
termide/
├── src/
│   ├── app/                    # Application core and event handling
│   │   ├── mod.rs             # Main app structure
│   │   ├── key_handler.rs     # Keyboard input handling
│   │   ├── mouse_handler.rs   # Mouse input handling
│   │   ├── modal_handler.rs   # Modal dialog handling
│   │   └── modal/             # Modal implementations
│   ├── config.rs              # Configuration management
│   ├── constants.rs           # Application constants
│   ├── i18n/                  # Internationalization
│   │   ├── en.rs             # English strings
│   │   └── ru.rs             # Russian strings
│   ├── layout_manager.rs      # Panel layout and accordion system
│   ├── panels/                # Panel implementations
│   │   ├── mod.rs            # Panel trait definition
│   │   ├── panel_group.rs    # Vertical accordion group
│   │   ├── file_manager/     # File manager panel
│   │   ├── editor.rs         # Text editor panel
│   │   ├── terminal_pty.rs   # Terminal panel with PTY
│   │   ├── debug.rs          # Log panel
│   │   └── welcome.rs        # Welcome screen panel
│   ├── state.rs               # Application state management
│   ├── system_monitor.rs      # CPU/RAM monitoring
│   ├── theme.rs               # Theme system
│   └── ui/                    # UI components
│       ├── layout.rs         # Main layout rendering
│       ├── panel_rendering.rs # Panel rendering utilities
│       ├── menu.rs           # Menu bar
│       ├── modal.rs          # Modal dialogs
│       ├── status_bar.rs     # Status bar
│       └── dropdown.rs       # Dropdown menus
├── themes/                    # Built-in theme definitions (TOML)
├── doc/                       # Documentation
│   ├── en/                   # English documentation
│   └── ru/                   # Russian documentation
└── help/                      # In-app help files
    ├── en.txt                # English help
    └── ru.txt                # Russian help
```

## Key Components

### 1. LayoutManager (`src/layout_manager.rs`)

Manages the accordion panel layout system:
- Tracks FileManager (static left panel)
- Manages horizontal panel groups
- Handles focus navigation
- Implements smart panel stacking/unstacking

**Key Types:**
- `FocusTarget` - Either FileManager or Group(usize)
- `LayoutManager` - Main layout coordinator

### 2. PanelGroup (`src/panels/panel_group.rs`)

Represents a vertical stack of panels (accordion):
- One expanded panel, others collapsed to title bar
- Maintains expanded_index
- Provides navigation within group

### 3. Panel Trait (`src/panels/mod.rs`)

All panels implement this trait:
```rust
pub trait Panel {
    fn render(&mut self, area: Rect, buf: &mut Buffer, is_focused: bool, panel_index: usize, state: &AppState);
    fn handle_key(&mut self, key: KeyEvent) -> Result<()>;
    fn handle_mouse(&mut self, mouse: MouseEvent, panel_area: Rect) -> Result<()>;
    fn title(&self) -> String;
    fn is_welcome_panel(&self) -> bool { false }
    // ... other methods
}
```

### 4. Event Handling

**Flow:**
1. `EventHandler` polls for terminal events
2. Events dispatched to appropriate handler:
   - `key_handler.rs` for keyboard
   - `mouse_handler.rs` for mouse
   - `modal_handler.rs` for modals
3. Handlers update `LayoutManager` and panel states
4. UI re-renders on next frame

### 5. State Management (`src/state.rs`)

`AppState` contains:
- Theme configuration
- Terminal dimensions
- File system watcher
- Batch operations state
- Modal state
- Error messages

## Coding Conventions

### Style

- Follow Rust standard style (enforced by `cargo fmt`)
- Use meaningful variable names
- Keep functions focused and small
- Add comments for complex logic

### Error Handling

- Use `anyhow::Result` for error propagation
- Use `.context()` or `.with_context()` to add error context
- Avoid `.unwrap()` - use `.expect()` with descriptive message or proper error handling
- Log errors to `state.log_error()` for debugging

### UI Code

- Use `ratatui` widgets for rendering
- Keep rendering logic separate from business logic
- Calculate dimensions carefully (account for borders, padding)
- Test UI at different terminal sizes

### Panel Implementation

When creating a new panel:

1. Implement the `Panel` trait
2. Handle keyboard input in `handle_key()`
3. Handle mouse input in `handle_mouse()`
4. Implement proper rendering in `render()`
5. Return meaningful `title()` for panel header
6. Add to panel creation in `app/mod.rs` or menu

## Testing

### Manual Testing Checklist

When making changes, test:
- [ ] Different terminal sizes (resize during operation)
- [ ] Keyboard navigation (all hotkeys)
- [ ] Mouse interactions (clicks, scrolling)
- [ ] Modal dialogs (open, close, interact)
- [ ] Panel management (open, close, stack, unstack)
- [ ] Theme switching
- [ ] Both English and Russian UI

### Common Issues

**Panel rendering glitches:**
- Check border calculations
- Verify area.width/height account for borders (subtract 2)
- Test at minimum width (80 chars)

**Focus issues:**
- Verify FocusTarget is updated correctly
- Check focus handling in event handlers
- Test navigation with empty groups

**Memory leaks:**
- Ensure panels are properly dropped when closed
- Check for circular references
- Monitor with `cargo clippy`

## Contribution Workflow

1. **Fork** the repository
2. **Create a branch** for your feature/fix
3. **Make changes** following coding conventions
4. **Test thoroughly** (see checklist above)
5. **Run code quality checks:**
   ```bash
   cargo fmt
   cargo clippy
   cargo test
   ```
6. **Commit** with clear, descriptive messages
7. **Push** to your fork
8. **Open a Pull Request** with:
   - Clear description of changes
   - Why the change is needed
   - Test results
   - Screenshots for UI changes

## Debugging

### Logging

TermIDE writes logs to:
- Linux: `~/.config/termide/termide.log`
- macOS: `~/Library/Application Support/termide/termide.log`
- Windows: `%APPDATA%\\termide\\termide.log`

Use logging in code:
```rust
state.log_info("Info message");
state.log_error(format!("Error: {}", error));
state.log_debug("Debug message");
```

### Log Panel

Open with `Alt+L`:
- Shows application state
- Displays recent log entries
- Shows panel information
- Useful for development

### Common Debugging Tasks

**Panel not rendering:**
1. Check panel is in a group: `layout_manager.panel_groups`
2. Verify focus is correct: `layout_manager.focus`
3. Check rendering area is non-zero

**Keyboard input not working:**
1. Check if modal is open (captures input)
2. Verify panel has focus
3. Check key translation (Cyrillic support)

**Memory usage increasing:**
1. Run with `valgrind` or similar
2. Check for unbounded collections
3. Verify panels are dropped on close

## Performance Considerations

### Rendering

- Minimize expensive operations in `render()`
- Cache computed values when possible
- Use `area` dimensions to limit work
- Profile with `cargo flamegraph` if needed

### File Operations

- Use async operations where appropriate
- Implement debouncing for file system events
- Limit directory traversal depth
- Handle large files gracefully (100 MB limit)

### Terminal Operations

- Batch terminal writes
- Minimize screen redraws
- Use partial updates when possible

## Resources

- **Ratatui:** https://github.com/ratatui-org/ratatui
- **Crossterm:** https://github.com/crossterm-rs/crossterm
- **Tree-sitter:** https://tree-sitter.github.io/
- **Rust Book:** https://doc.rust-lang.org/book/

## Getting Help

- **Issues:** https://github.com/termide/termide/issues
- **Discussions:** Use GitHub Discussions for questions
- **Code Review:** Request review on your PR

## License

TermIDE is licensed under the MIT License. By contributing, you agree to license your contributions under the same terms.
