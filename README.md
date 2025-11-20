# TermIDE

A cross-platform terminal-based IDE, file manager, and virtual terminal written in Rust.

## Features

- **Terminal-based IDE** - Edit files directly in your terminal with syntax highlighting for multiple programming languages
- **Smart File Manager** - Navigate and manage files with an intuitive TUI interface
- **Integrated Virtual Terminal** - Run commands without leaving the IDE with full PTY support
- **Multi-panel Layout** - Work with multiple files and terminals simultaneously
- **Cross-platform** - Works on Linux, macOS, and Windows (WSL)
- **Git Integration** - See file status and changes at a glance with color-coded indicators
- **Configurable Themes** - Customize the appearance to your preference
- **Batch Operations** - Copy, move, and manage multiple files efficiently
- **Search and Replace** - Find and replace text in editor with case-sensitivity support
- **Multi-language Support** - UI localization (English, Russian)
- **Mouse Support** - Full mouse support for all panels and UI elements

## Installation

### Using Nix (Recommended)

```bash
# Clone the repository
git clone https://github.com/termide/termide.git
cd termide

# Enter development environment
nix develop

# Build the project
cargo build --release

# Run
./target/release/termide
```

### Using Cargo

```bash
# Clone the repository
git clone https://github.com/termide/termide.git
cd termide

# Build and run
cargo run --release
```

## Requirements

- Rust 1.70+ (stable)
- For Nix users: Nix with flakes enabled

## Usage

### Quick Start

After launching TermIDE, you'll see:
- File manager panel on the left
- Welcome panel on the right (when no other panels are open)
- Menu bar at the top
- Status bar at the bottom

Use `Alt+M` to open the menu or `Alt+H` for help.

### Documentation

For detailed documentation, see:
- **English**: [doc/en/README.md](doc/en/README.md)
- **Russian**: [doc/ru/README.md](doc/ru/README.md)

### Keyboard Shortcuts (Quick Reference)

**Global:**
- `Alt+M` - Toggle menu
- `Alt+H` - Show help
- `Alt+Q` / `Alt+Delete` - Quit application
- `Alt+Left` / `Alt+Right` - Switch between panels
- `Alt+1` to `Alt+9` - Go to panel by number
- `Escape` / `Alt+X` - Close current panel

**File Manager:**
- `Enter` - Open file or enter directory
- `Backspace` - Go to parent directory
- `Space` - Show file/directory information
- `Insert` - Toggle file selection
- `Ctrl+A` - Select all files
- `F` - Create new file
- `D` / `F7` - Create new directory
- `C` / `F5` - Copy selected files
- `M` / `F6` - Move/rename files
- `Delete` / `F8` - Delete selected files

**Editor:**
- `Ctrl+S` - Save file
- `Ctrl+Z` / `Ctrl+Y` - Undo/Redo
- `Ctrl+F` - Find text
- `Ctrl+H` - Replace text
- `F3` / `Shift+F3` - Next/Previous match
- `Ctrl+C` / `Ctrl+X` / `Ctrl+V` - Copy/Cut/Paste

**Panels:**
- `Alt+F` - New file manager
- `Alt+T` - New terminal
- `Alt+E` - New editor
- `Alt+P` - Settings

## Configuration

Configuration file location:
- Linux: `~/.config/termide/config.toml`
- macOS: `~/Library/Application Support/termide/config.toml`
- Windows: `%APPDATA%\termide\config.toml`

### Example Configuration

```toml
# Theme name (default, dark, light, monokai, solarized_dark, solarized_light, nord, gruvbox)
theme = "default"

# Language (auto, en, ru)
# "auto" detects from environment variables (TERMIDE_LANG, LANG, LC_ALL)
language = "auto"

# Optional: Custom log file path
# log_file_path = "/custom/path/to/termide.log"
```

You can also set the language via environment variable:
```bash
export TERMIDE_LANG=ru  # Set Russian UI
./termide
```

## Development

### Project Structure

```
src/
├── app/           # Application core and event handling
├── config.rs      # Configuration management
├── constants.rs   # Application constants
├── panels/        # Panel implementations (file manager, editor, terminal)
├── state.rs       # Application state management
├── theme.rs       # Theme definitions
└── ui/            # UI components (menus, modals, status bar)
```

### Building

```bash
# Development build
cargo build

# Release build with optimizations
cargo build --release

# Run tests
cargo test

# Check code quality
cargo clippy
cargo fmt --check
```

### Nix Development

The project includes a Nix flake for reproducible development environments:

```bash
# Enter development shell
nix develop

# Build with Nix
nix build

# Run checks
nix flake check
```

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

This project is dual-licensed under:
- MIT License
- Apache License 2.0

You may choose either license for your use.

## Acknowledgments

Built with:
- [ratatui](https://github.com/ratatui-org/ratatui) - Terminal UI framework
- [crossterm](https://github.com/crossterm-rs/crossterm) - Cross-platform terminal manipulation
- [tui-textarea](https://github.com/rhysd/tui-textarea) - Text editor widget
- [portable-pty](https://github.com/wez/wezterm/tree/main/pty) - PTY implementation
