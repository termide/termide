# TermIDE

[![GitHub Release](https://img.shields.io/github/v/release/termide/termide)](https://github.com/termide/termide/releases)
[![CI](https://github.com/termide/termide/actions/workflows/release.yml/badge.svg)](https://github.com/termide/termide/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

A cross-platform terminal-based IDE, file manager, and virtual terminal written in Rust.

<p align="center">
  <img src="assets/termide.jpg" alt="TermIDE Screenshot" width="800">
</p>

## Features

- **Terminal-based IDE** - Edit files directly in your terminal with syntax highlighting for 15+ programming languages (Rust, Python, JavaScript, TypeScript, Go, C/C++, Java, Ruby, PHP, Haskell, Nix, HTML, CSS, JSON, TOML, YAML, Bash, Markdown)
- **Smart File Manager** - Navigate and manage files with an intuitive TUI interface
- **Integrated Virtual Terminal** - Run commands without leaving the IDE with full PTY support
- **Multi-panel Layout** - Work with multiple files and terminals simultaneously
- **Cross-platform** - Works on Linux (x86_64, ARM64), macOS (Intel, Apple Silicon), and Windows (via WSL)
- **Git Integration** - See file status and changes at a glance with color-coded indicators and automatic updates
- **12 Built-in Themes** - Choose from popular themes like Dracula, Nord, Monokai, Solarized, and more
- **Custom Theme Support** - Create and load your own themes from config directory
- **System Resource Monitoring** - Real-time CPU, RAM, and disk usage indicators with device names and color-coded alerts
- **Batch Operations** - Copy, move, and manage multiple files efficiently
- **Search and Replace** - Interactive modals with live search preview, match counter, Tab/Shift+Tab navigation, replace counter feedback, and state preservation
- **Powerful Editing** - Duplicate line/selection (Ctrl+D), configurable tab size, undo/redo support
- **Multi-language Support** - UI localization (English, Russian) with proper keyboard layout support (including Cyrillic)
- **Robust Error Handling** - Graceful fallbacks for theme errors, file size limits (100 MB), and clear error messages
- **Mouse Support** - Full mouse support for all panels and UI elements

## Installation

### Download Pre-built Binary (Recommended)

Download the latest release for your platform from [GitHub Releases](https://github.com/termide/termide/releases):

```bash
# Linux x86_64 (also works in WSL)
wget https://github.com/termide/termide/releases/latest/download/termide-v0.1.2-x86_64-unknown-linux-gnu.tar.gz
tar xzf termide-v0.1.2-x86_64-unknown-linux-gnu.tar.gz
./termide

# macOS Intel (x86_64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-v0.1.2-x86_64-apple-darwin.tar.gz
tar xzf termide-v0.1.2-x86_64-apple-darwin.tar.gz
./termide

# macOS Apple Silicon (ARM64)
curl -LO https://github.com/termide/termide/releases/latest/download/termide-v0.1.2-aarch64-apple-darwin.tar.gz
tar xzf termide-v0.1.2-aarch64-apple-darwin.tar.gz
./termide

# Linux ARM64 (Raspberry Pi, ARM servers)
wget https://github.com/termide/termide/releases/latest/download/termide-v0.1.2-aarch64-unknown-linux-gnu.tar.gz
tar xzf termide-v0.1.2-aarch64-unknown-linux-gnu.tar.gz
./termide
```

**Supported Platforms:**
- Linux x86_64 (also works in WSL/WSL2)
- Linux ARM64 (Raspberry Pi, ARM servers)
- macOS Intel (x86_64)
- macOS Apple Silicon (M1/M2/M3)

### Install from crates.io

```bash
cargo install termide
```

### Build from Source with Nix

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

### Build from Source with Cargo

```bash
# Clone the repository
git clone https://github.com/termide/termide.git
cd termide

# Build and run
cargo run --release
```

## Requirements

- For pre-built binaries: No additional requirements
- For building from source:
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
- `Ctrl+F` - Find text (interactive modal with live preview)
- `Ctrl+H` - Replace text (interactive modal with two fields)
- `F3` / `Shift+F3` - Next/Previous match
- `Tab` / `Shift+Tab` - Navigate matches (when search active)
- `Escape` - Close search/modal first, then close panel
- `Ctrl+C` / `Ctrl+X` / `Ctrl+V` - Copy/Cut/Paste
- Mouse support: Click buttons in modals, `[X]` to close panels

**Panels:**
- `Alt+F` - New file manager
- `Alt+T` - New terminal
- `Alt+E` - New editor
- `Alt+P` - Open configuration file in editor

## Configuration

Configuration file location:
- Linux: `~/.config/termide/config.toml`
- macOS: `~/Library/Application Support/termide/config.toml`
- Windows: `%APPDATA%\termide\config.toml`

### Example Configuration

```toml
# Theme name - choose from built-in themes or use a custom theme from ~/.config/termide/themes/
theme = "default"

# Tab size (number of spaces per tab)
tab_size = 4

# Language (auto, en, ru)
# "auto" detects from environment variables (TERMIDE_LANG, LANG, LC_ALL)
language = "auto"

# System resource monitor update interval in milliseconds (default: 1000)
resource_monitor_interval = 1000

# Optional: Custom log file path
# log_file_path = "/custom/path/to/termide.log"
```

### Available Themes

**Dark Themes:**
- `default` - Default dark theme
- `midnight` - Midnight Commander inspired theme
- `dracula` - Popular Dracula theme
- `onedark` - Atom One Dark theme
- `monokai` - Classic Monokai theme
- `nord` - Nord theme with blue tones
- `solarized-dark` - Dark Solarized theme

**Light Themes:**
- `atom-one-light` - Atom One Light theme
- `ayu-light` - Ayu Light theme
- `github-light` - GitHub Light theme
- `material-lighter` - Material Lighter theme
- `solarized-light` - Light Solarized theme

**Theme Examples:**

<p align="center">
  <img src="assets/screenshots/dracula.png" alt="Dracula Theme" width="600">
  <br>
  <em>Dracula Theme</em>
</p>

<p align="center">
  <img src="assets/screenshots/monokai.png" alt="Monokai Theme" width="600">
  <br>
  <em>Monokai Theme</em>
</p>

### Custom Themes

You can create custom themes by placing TOML files in the themes directory:
- Linux: `~/.config/termide/themes/`
- macOS: `~/Library/Application Support/termide/themes/`
- Windows: `%APPDATA%\termide\themes\`

User themes take priority over built-in themes with the same name. See `themes/` directory in the repository for theme file format examples.

### Language Configuration

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
├── i18n/          # Internationalization (en, ru)
├── panels/        # Panel implementations (file manager, editor, terminal)
├── state.rs       # Application state management
├── system_monitor.rs  # CPU/RAM monitoring
├── theme.rs       # Theme system and built-in themes
└── ui/            # UI components (menus, modals, status bar)

themes/            # Built-in theme definitions (TOML files)
doc/
├── en/            # English documentation
└── ru/            # Russian documentation
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
- [portable-pty](https://github.com/wez/wezterm/tree/main/pty) - PTY implementation
- [tree-sitter](https://github.com/tree-sitter/tree-sitter) - Syntax highlighting
- [ropey](https://github.com/cessen/ropey) - Text buffer
- [sysinfo](https://github.com/GuillaumeGomez/sysinfo) - System resource monitoring
