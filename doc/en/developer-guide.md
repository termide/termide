# Developer Guide

This guide is for developers who want to contribute to TermIDE or understand its codebase.

## Development Setup

### Prerequisites

- **Rust** — the version is pinned in `rust-toolchain.toml`; rustup installs
  and selects it automatically on the first `cargo` command in the checkout
- **Git** for version control
- **A C compiler** for the tree-sitter grammars, which `build.rs` compiles:
  gcc or clang on Linux, the Xcode Command Line Tools
  (`xcode-select --install`) on macOS, MSVC on Windows
- **Optional:** Nix with flakes enabled for reproducible builds

Nothing else. The workspace is pure Rust end-to-end — FTPS uses rustls with
webpki-roots and SFTP uses russh, so there is no OpenSSL or libssh2 to install
— which is also what makes the fully static musl build possible.

### Getting the Source Code

```bash
git clone https://github.com/termide/termide.git
cd termide

# Activate the versioned git hooks (once per clone)
git config core.hooksPath .githooks
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

The dev shell reads its compiler version from `rust-toolchain.toml`, the same
file rustup and CI use, so a Nix and a non-Nix checkout build with the same
compiler.

#### Platform notes

CI runs the fmt / check / clippy / test gates on Linux and macOS; Windows and
the cross-compiled targets are built at release time. Code that reaches for
platform facilities needs a branch per platform — the existing examples are
process introspection (`/proc` on Linux, libproc on macOS, ToolHelp snapshots
on Windows), the mount table (`/proc/mounts` vs `getmntinfo`), and the
keyboard enhancement flags.

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

TermIDE uses a Cargo workspace with modular crates:

```
termide/
├── src/                       # Binary entry point
│   ├── main.rs               # App initialization, terminal setup
│   └── ui.rs                 # Top-level rendering bridge
├── crates/
│   ├── app/                  # Application core, event handling, panel management
│   ├── app-core/             # Core application traits (LayoutController, PanelProvider)
│   ├── app-modal/            # Modal dialog handling
│   ├── app-panel/            # Panel management operations
│   ├── app-session/          # Session save/restore logic
│   ├── app-watcher/          # File system watcher integration
│   ├── buffer/               # Text buffer implementation (ropey-based)
│   ├── clipboard/            # System clipboard integration
│   ├── config/               # Configuration management (TOML)
│   ├── core/                 # Core Panel trait and shared types
│   ├── file-ops/             # File operations (copy, move, delete, upload, download)
│   ├── git/                  # Git integration (status, diff, log)
│   ├── highlight/            # Syntax highlighting (tree-sitter, 22 languages)
│   ├── i18n/                 # Internationalization (15 languages)
│   ├── keyboard/             # Keyboard handling and layout translation
│   ├── layout/               # Panel groups, split layout, fullscreen preset
│   ├── logger/               # Logging system
│   ├── lsp/                  # Language Server Protocol client
│   ├── modal/                # Modal dialog implementations
│   ├── panel-diagnostics/    # LSP diagnostics panel
│   ├── panel-editor/         # Text editor panel
│   ├── panel-file-manager/   # File manager panel
│   ├── panel-git-diff/       # Git diff viewer panel
│   ├── panel-git-log/        # Git log panel
│   ├── panel-git-status/     # Git status panel
│   ├── panel-image/          # Image viewer panel
│   ├── panel-misc/           # Help, Journal, and References panels
│   ├── panel-operations/     # Background operations panel
│   ├── panel-terminal/       # Terminal emulator panel (PTY)
│   ├── session/              # Session persistence
│   ├── state/                # Application state (batch, layout, operations, ui)
│   ├── system-monitor/       # CPU/RAM/Disk monitoring
│   ├── theme/                # Theme system and 38 built-in themes
│   ├── ui/                   # UI utilities and path formatting
│   ├── ui-render/            # UI rendering (menu, status bar, panels)
│   ├── unicode-width-fix/    # Unicode width corrections for East Asian characters
│   ├── vfs/                  # Virtual filesystem (SFTP, FTP, SMB)
│   └── watcher/              # File system event watcher
├── doc/                       # Documentation
│   ├── en/                   # English documentation
│   ├── ru/                   # Russian documentation
│   └── zh/                   # Chinese documentation
└── packaging/                 # Distribution packaging (deb, rpm, AUR, Homebrew, Nix)
```

## Key Components

### 1. LayoutManager (`crates/layout/src/`)

Owns the split-layout state for the whole window:
- Horizontal arrangement of `Vec<PanelGroup>` (each group is a column)
- Focus navigation between groups (Alt+Left/Right)
- Smart stacking/unstacking of panels between adjacent groups (Alt+Backspace, F11)
- Proportional width redistribution on terminal resize
- Width-adaptive default layout via `setup_default_layout()`

### 2. PanelGroup (`crates/layout/src/panel_group.rs`)

A column of vertically stacked panels with adjustable heights:
- `expanded_index: usize` — focused panel (active border + the one boosted by the fullscreen preset, when active)
- `width: Option<u16>` — column width (None = auto-distribute)
- `split_heights: Option<Vec<u16>>` — cached per-panel heights, rescaled proportionally on resize
- `fullscreen_cache: Option<Vec<u16>>` — heights to restore when the fullscreen preset toggles off (`Alt+F11`)
- `next_panel`/`prev_panel`/`set_expanded` move focus and re-apply the preset when active
- `grow_focused`/`shrink_focused` add or remove rows from the focused panel (1-row step), cascading through neighbours
- `resize_panel_divider` applies a delta to the divider above a given panel (used by the mouse drag handler)

### 3. Panel Trait (`crates/core/src/lib.rs`)

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

### 4. Event Handling (`crates/app/src/app/`)

**Flow:**
1. `EventHandler` polls for terminal events
2. Events dispatched to appropriate handler:
   - `key_handler.rs` for keyboard
   - `mouse_handler.rs` for mouse
   - `modal_handler.rs` for modals
3. Handlers update `LayoutManager` and panel states
4. UI re-renders on next frame

### 5. State Management (`crates/state/src/`)

Split into modules: `batch.rs`, `layout.rs`, `operations.rs`, `pending_action.rs`, `ui.rs`.

`AppState` contains:
- Theme configuration
- Terminal dimensions
- File system watcher
- Batch operations state
- Modal state
- UI state (menu, submenus, drag)

### 6. Internationalisation (`crates/i18n/`)

15 locales are supported (en, ru, de, es, fr, pt, ja, ko, zh, bn, hi, id, th, tr, vi); dictionaries are TOML files under `crates/i18n/i18n/*.toml`. The `Translation` trait and its `RuntimeTranslation` implementation expose type-safe access to strings.

**English fallback.** If a key is missing from the current locale, `RuntimeTranslation` transparently falls back to the English value and logs a warning:

```
WARN Missing translation key: my_key (using English fallback)
```

This keeps the UI from rendering empty strings when translations lag behind. For non-English locales the English dictionary is always loaded alongside the primary one.

**Adding a key:**
1. Add the method to the `Translation` trait (`crates/i18n/src/lib.rs`).
2. Add the implementation to `RuntimeTranslation` (`crates/i18n/src/runtime.rs`) — usually `self.get_string("key")` or `self.format("key", &[...])` for placeholder-bearing formats.
3. Add an entry to every one of the 15 `*.toml` files (missing entries will use the fallback, but it's preferable to translate up front).

**Removing a key:** remove the trait method, the runtime implementation, and the strings from all 15 dictionaries. Synchronisation is verified by comparing keys in `en.toml` against the other locales.

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
- Linux: `~/.cache/termide/termide.log`
- macOS: `~/Library/Caches/termide/termide.log`
- Windows: `%LOCALAPPDATA%\\termide\\cache\\termide.log`

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
