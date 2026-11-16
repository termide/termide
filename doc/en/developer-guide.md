# Developer Guide

This guide is for developers who want to contribute to Termide or understand its codebase.

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

The dev shell and `nix build` read the compiler version from
`rust-toolchain.toml`, the same file rustup and CI use, so a Nix and a non-Nix
checkout build with the same compiler.

#### Platform notes

CI runs the fmt / clippy / test gates on Linux and macOS; Windows and
the cross-compiled targets are built at release time. Code that reaches for
platform facilities needs a branch per platform — the existing examples are
process introspection (`/proc` on Linux, libproc on macOS, ToolHelp snapshots
on Windows), the mount table (`/proc/mounts` vs `getmntinfo`), and the
keyboard enhancement flags.

On Linux the build links with the toolchain's default: rust-lld on x86_64,
the system's `ld` on other architectures. Linking the binary takes about a
second with lld and half that with [mold](https://github.com/rui314/mold),
against six with GNU `ld`; to use mold (or `lld`) for your own builds, install
it and add to `~/.cargo/config.toml`:

```toml
[target.aarch64-unknown-linux-gnu]   # or x86_64-unknown-linux-gnu
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

### Running Tests

```bash
# Run all tests (the root crate is a package too: without --workspace
# only its own tests run)
cargo test --workspace

# Run tests with output
cargo test --workspace -- --nocapture

# Run specific test
cargo test --workspace test_name

# Faster, as the pre-commit hook and CI do (cargo install cargo-nextest)
cargo nextest run --workspace
```

UI and agent behaviour is checked live with
[`tools/tui-harness`](../../tools/tui-harness/README.md): it runs the built
binary headlessly in a pty against scenario scripts (keys, waits, checks on
the screen), with a throwaway HOME, the demo project and mock model or ACP
agents.

```bash
cargo build
tools/tui-harness/run.sh           # every scenario
tools/tui-harness/run.sh agent     # one of them
```

### Code Quality Checks

```bash
# Run clippy linter (covers the compiler checks; no separate cargo check)
cargo clippy --workspace --all-targets -- -D warnings

# Format code
cargo fmt --all

# Check formatting without modifying files
cargo fmt --all -- --check

# Find unused dependency declarations (cargo install cargo-machete)
cargo machete
```

These are the gates of the pre-commit hook (`.githooks/pre-commit`, enabled
with `git config core.hooksPath .githooks`): fmt, machete, clippy and the
test suite.

## Project Structure

Termide uses a Cargo workspace with modular crates:

```
termide/
├── src/                       # Binary entry point
│   ├── completions.rs        # Shell completion scripts (--completions)
│   ├── main.rs               # CLI parsing, app initialization, terminal setup
│   └── ui.rs                 # Top-level rendering bridge
├── crates/
│   ├── agent-acp/            # External agent over the Agent Client Protocol (stdio)
│   ├── agent-core/           # Provider-agnostic agent core: messages, tools, agent loop
│   ├── agent-hooks/          # Command hooks: external programs run on tool calls
│   ├── agent-mcp/            # MCP client (tools from servers) and termide's own MCP server
│   ├── agent-providers/      # Model providers (OpenAI-compatible chat completions)
│   ├── agent-recall/         # The recall tool: ranked search over agent sessions, git history and code
│   ├── agent-tools/          # Built-in agent tools: read, edit, write, shell
│   ├── agent-web/            # Web tools: fetch and web_search, over HTTP or a local Chrome
│   ├── app/                  # Application orchestrator, event handling, panel management
│   ├── app-core/             # Core application traits (LayoutController, PanelProvider)
│   ├── buffer/               # Text buffer implementation (ropey-based)
│   ├── clipboard/            # System clipboard integration
│   ├── config/               # Configuration management (TOML)
│   ├── core/                 # Core Panel trait and shared types
│   ├── db/                   # Database access core (SQLite, PostgreSQL, MySQL)
│   ├── debug/                # Breakpoints and command-line debugger launches
│   ├── detach/               # Detached (daemonised) instance hosting
│   ├── fetch/                # Minimal, security-bounded HTTP(S) GET for viewer panels
│   ├── file-ops/             # File operations queue (copy, move, delete, upload, download)
│   ├── fsutil/               # Crash-safe atomic file writes, shared by the editor, agent and project store
│   ├── git/                  # Git integration (status, diff, log)
│   ├── highlight/            # Syntax highlighting (tree-sitter)
│   ├── html/                 # HTML to terminal pseudographics renderer
│   ├── html-markdown/        # HTML to Markdown (agent fetch, HTML viewer export)
│   ├── i18n/                 # Internationalization (15 languages)
│   ├── keyboard/             # Keyboard handling and layout translation
│   ├── layout/               # Panel groups, split layout, fullscreen preset
│   ├── logger/               # Logging system
│   ├── lsp/                  # Language Server Protocol client
│   ├── mermaid/              # Mermaid parsing and text-pseudographics layout
│   ├── modal/                # Modal dialog system
│   ├── panel-agent/          # Coding agent panel (transcript, input, permission prompts)
│   ├── panel-binary/         # Binary file viewer/editor panel (hex/ASCII)
│   ├── panel-csv/            # CSV/TSV table viewer panel
│   ├── panel-db/             # Database viewer panel
│   ├── panel-debug/          # Debug session panel (DAP call stack, variables, output)
│   ├── panel-diagnostics/    # LSP diagnostics panel
│   ├── panel-editor/         # Text editor panel
│   ├── panel-file-manager/   # File manager panel
│   ├── panel-git-diff/       # Git diff viewer panel
│   ├── panel-git-log/        # Git log panel
│   ├── panel-git-status/     # Git status panel
│   ├── panel-html/           # Rendered HTML preview panel
│   ├── panel-image/          # Image viewer panel (native graphics)
│   ├── panel-markdown/       # Rendered Markdown preview panel
│   ├── panel-mermaid/        # Mermaid diagram viewer panel (experimental)
│   ├── panel-misc/           # Journal and References panels
│   ├── panel-operations/     # Background operations panel
│   ├── panel-outline/        # Outline panel
│   ├── panel-plugin-list/    # A plugin's panel drawn as a list of rows with actions
│   ├── panel-terminal/       # Terminal emulator panel (PTY)
│   ├── plugin-perforce/      # Built-in Perforce plugin (`termide --plugin perforce`)
│   ├── plugins/              # Plugins: manifests, the JSON-RPC peer and the process host
│   ├── project/              # Saved project layouts
│   ├── richtext/             # Rich-text layout engine (styled runs to wrapped lines)
│   ├── secrets/              # Master-password encrypted credential vault
│   ├── state/                # Application state (batch, layout, operations, ui)
│   ├── system-monitor/       # CPU/RAM/Disk monitoring
│   ├── theme/                # Theme system and 44 built-in themes
│   ├── ui/                   # UI components and path formatting
│   ├── ui-render/            # UI rendering (menu, status bar, panels)
│   ├── unicode-width-fix/    # Patched unicode-width (East Asian width corrections)
│   ├── vfs/                  # Virtual filesystem (SFTP, FTP, FTPS, SMB, NFS)
│   ├── viewer/               # Viewer core shared by the HTML and Markdown panels
│   ├── walk/                 # Project tree walking that honours .gitignore
│   └── watcher/              # Filesystem and git watcher
├── doc/                       # Documentation
│   ├── en/                   # English documentation
│   ├── ru/                   # Russian documentation
│   └── zh/                   # Chinese documentation
├── packaging/                 # AUR PKGBUILDs, crates.io redirect stub
└── flake.nix                  # Nix flake (package, dev shell)
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
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
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

Each run writes its log to `logs/<date>-<time>.log` in the project's data
directory (`~/.local/share/termide/projects/<name>-<hash>/` on Linux,
`~/Library/Application Support/termide/projects/<name>-<hash>/` on macOS); logs
older than 24 hours are removed at startup. The `[logging]` section changes
this:

```toml
[logging]
file_path = "/tmp/termide.log"  # one fixed file instead; global config only
min_level = "info"              # trace, debug, info, warn or error (default: info)
```

`--log-level` overrides `min_level` for one run without saving it. The same
records show in the journal panel (`Alt+L`).

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

Termide is licensed under the MIT License. By contributing, you agree to license your contributions under the same terms.
