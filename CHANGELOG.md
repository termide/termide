# Changelog

All notable changes to TermIDE will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] - 2025-12-13

### Added
- Editor Tab key insertion and block indent/unindent (Tab/Shift+Tab)
- External file change detection with reload/close dialog
- Log viewer panel with Editor-based rendering (cursor, selection support)
- Mouse scroll based on cursor position, not panel focus
- Recursive file watching for git repositories

### Changed
- **Major architecture refactoring**: extracted 31 workspace crates from monolithic src/
  - Modular crate structure: app, app-core, app-event, app-modal, app-panel, etc.
  - Improved build times with incremental compilation
  - Better code organization and separation of concerns
- Type-safe panel communication architecture
  - `PanelCommand` enum replacing unsafe `dyn Any` downcasting
  - `CommandResult` enum for type-safe command responses
  - `handle_command()` method on Panel trait
- Modal dialog code consolidation
  - Shared frame rendering with [X] close button
  - Shared input field rendering with cursor
  - Reduced code duplication in search/replace modals
- Config restructured to nested TOML sections
- Async git diff computation to prevent UI freeze
- Gitignore-aware filesystem updates

### Fixed
- File manager cursor position preserved after file deletion
- Copy/paste operations no longer cause unwanted scroll
- Local timezone display for file modification times
- File sizes rounded to whole units and right-aligned
- Git diff deduplication to prevent UI freeze
- FS watcher feedback loops and deleted files display

### Performance
- Async git diff and gitignore-aware FS updates
- Cache `find_repo_root()` and throttle spinner
- Conditional redraw to reduce idle CPU usage
- Memory optimization: removed redundant clones

### Tests
- PanelCommand and CommandResult unit tests (7 tests in core)
- Editor handle_command integration tests (9 tests)
- FileManager handle_command integration tests (8 tests)
- Modal base module tests (4 tests)
- Large file handling tests (7 tests)
  - 10K+ line file loading and navigation
  - Scroll performance benchmark (50K lines in <100ms)

### Code Quality
- Zero TODO/FIXME comments in production code
- All 5 unsafe blocks documented with SAFETY comments
- Replaced 21+ critical `unwrap()` calls with `expect()` + context messages
- Fixed error swallowing in Editor `handle_key()` - errors now shown to user
- Removed dead code: unused `RequestDirSize` event and `PanelProvider` trait methods
- Optimized `get_selected_text()` - no longer copies entire buffer for large selections

## [0.4.0] - 2025-12-07

### Added
- Double-click word selection in editor
  - Select word between nearest delimiters with double-click
  - Proper handling of alphanumeric word boundaries
- Smart word wrapping with word boundary detection
  - Breaks lines at word boundaries when possible
  - Falls back to hard break for words wider than viewport
- Visual line navigation for word-wrapped text
  - Cursor Up/Down moves through visual lines, not buffer lines
  - Preserves preferred column across visual line movements
- Proper Unicode rendering for CJK and combining characters
  - Chinese/Japanese/Korean characters display with correct 2-column width
  - Hindi and other scripts with combining characters render correctly
  - Uses grapheme clusters for proper text segmentation
- Localization for 7 new languages
  - German (de), Spanish (es), French (fr), Hindi (hi)
  - Japanese (ja), Korean (ko), Portuguese (pt), Thai (th), Chinese (zh)
- Panel reordering hotkeys
  - Alt+[ and Alt+] to reorder panels within current group
  - Alt+PageUp/PageDown context-aware (reorder or switch)
- Kitty keyboard protocol support for proper Alt+Cyrillic handling

### Changed
- Major editor architecture decomposition
  - Extracted cursor movement to dedicated modules (physical, visual)
  - Separated rendering into focused modules (line, wrap, cursor, highlights)
  - Created RenderContext for shared rendering state
  - Keyboard handling with Command Pattern
- Translations migrated from Rust code to TOML files
  - Easier to add/update translations
  - Cleaner separation of concerns
- Terminal VT100 parser extracted to dedicated module
- Extensive code cleanup and DRY refactoring
  - Extracted TextInputHandler for modal inputs
  - Created path_utils module for path resolution
  - Added panel downcast helpers (PanelExt trait)

### Fixed
- Cursor Up/Down navigation with word wrap
- Git status tracking in subdirectories
- Editor navigation and viewport issues with word wrap
- App initialization with correct terminal size

### Performance
- Critical hot path optimizations (100-270x faster)
  - Vec pre-allocation when size is known
  - Eliminated unnecessary string allocations
  - Optimized terminal character shift with copy_within

## [0.3.0] - 2025-12-04

### Added
- Release management Claude Code skill for automated release workflow
  - Pre-release quality checks (fmt, clippy, test, build)
  - Multi-source change analysis (uncommitted, commits, file states)
  - Interactive version selection with validation
  - Automated version updates across 12+ files
  - Auto-generated CHANGELOG entries from git history
  - Post-update quality verification
  - Conventional commit generation and git tag creation
- XDG Base Directory Specification support
  - Config: `~/.config/termide/` (or `$XDG_CONFIG_HOME/termide/`)
  - Data: `~/.local/share/termide/` (or `$XDG_DATA_HOME/termide/`)
  - Cache: `~/.cache/termide/` (or `$XDG_CACHE_HOME/termide/`)
  - Proper cross-platform paths (Linux, macOS, Windows)
- Automatic session persistence with configurable retention
  - Sessions save automatically on focus loss (debounced)
  - Auto-cleanup of sessions older than configured retention period (default: 30 days)
  - Per-project session storage
  - Unsaved buffer persistence across sessions
- Comprehensive project documentation
  - CHANGELOG.md with full version history (0.1.0 to 0.2.0)
  - CONTRIBUTING.md with development guidelines
  - Updated issue templates
  - Revised security policy
  - Contributor Covenant Code of Conduct

### Changed
- **BREAKING**: FileManager is now a regular closable panel
  - Removed special fixed left panel handling
  - FileManager can be closed, resized, and moved between groups
  - Default initialization with 2 FileManager panels (50/50 layout)
  - Simplified architecture (-350 lines of code)
  - All panels are now first-class citizens with identical capabilities
  - Existing sessions will load with default layout

## [0.2.0] - 2025-12-04

### Added
- Comprehensive logging system with configurable levels (debug, info, warn, error)
- Real-time git diff visualization in editor line numbers
  - Display uncommitted changes with color-coded line numbers
  - Show deletion markers with count on horizontal lines
  - In-memory diff computation with debounced updates (300ms)
  - Localized deletion marker text (English/Russian)
- Configurable word wrap option in editor (enabled by default)
- Per-project session storage with automatic unsaved buffer persistence
- Automatic cleanup of old sessions (configurable retention period, default 30 days)
- New configuration options:
  - `word_wrap` - Enable/disable word wrap in editor
  - `min_log_level` - Minimum logging level (debug, info, warn, error)
  - `session_retention_days` - Session cleanup retention period
  - `show_git_diff` - Toggle git diff visualization
  - `fm_extended_view_width` - Minimum width for file manager extended view

### Changed
- Rewrite clipboard system using arboard library
  - Support both CLIPBOARD and PRIMARY selections on Linux
  - More reliable cross-platform clipboard operations
- Improve cursor rendering across all panels
  - Use inverse colors instead of theme selection colors
  - Better visual contrast and cursor visibility
  - Handle reverse attribute correctly in terminal panels
- Improve config auto-completion with hash-based detection
  - Automatically detect ALL missing config keys
  - No manual maintenance of required_keys array
  - Correctly handle optional fields
  - Normalize config file format

### Fixed
- Word wrap rendering bug causing single-line display
- Cursor going off-screen with git diff deletion markers
- Viewport calculations now account for virtual lines (deletion markers)
- Empty line rendering in word wrap mode
- Modified flag after undo/redo operations
- File manager navigation now remembers directory when going up
- Old API tests marked as ignored to fix CI

## [0.1.5] - 2025-12-02

### Fixed
- Package build issues for .deb and .rpm on ARM64 architecture
  - Removed aarch64 from package build matrices to avoid cross-compilation issues
  - Binary tarballs still support all platforms including ARM64

### Added
- Local package build testing script (`scripts/test-packages.sh`)

## [0.1.4] - 2025-12-02

### Added
- Package manager distribution support:
  - Debian/Ubuntu packages (.deb)
  - Fedora/RHEL/CentOS packages (.rpm)
  - Arch Linux AUR packages (source and binary variants)
  - Homebrew formula for macOS/Linux
- Enhanced Nix Flake support:
  - Add `packages.default` output with `rustPlatform.buildRustPackage`
  - Add `apps.default` for `nix run` support
  - Add `overlays.default` for nixpkgs integration
  - Install help files and themes in postInstall phase
- Automatic config file completion for new configuration keys
- Comprehensive installation documentation for all package managers

### Changed
- Reorganize README installation section with collapsible details blocks
- Use emoji icons for better visual navigation in documentation
- Update GitHub Actions workflow to build .deb and .rpm packages automatically

### Fixed
- GitHub Actions workflow dependencies (crates.io publication now requires all builds to succeed)
- Binary paths in cargo-deb and cargo-generate-rpm for cross-compilation
- Fail-fast strategy in build matrices to see all failures

## [0.1.3] - 2025-12-02

### Added
- **Accordion panel system** - Major architectural improvement
  - Smart panel stacking based on terminal width
  - Vertical accordion layout within horizontal groups
  - One expanded panel per group, others collapse to title bar
  - Configurable minimum panel width threshold (80 characters)
- New navigation hotkeys:
  - `Alt+Up/Down` - Navigate panels within group
  - `Alt+PgUp/PgDn` - Move panel to previous/next group
  - `Alt+Home/End` - Move panel to first/last group
  - `Alt+Plus/Minus` - Increase/decrease active group width
  - `Alt+Backspace` - Toggle panel stacking (merge/unstack)
- Developer documentation (`doc/en/architecture.md`, `doc/en/developer-guide.md`)

### Changed
- Complete panel layout system redesign
  - New `LayoutManager` for centralized panel group management
  - New `PanelGroup` for vertical panel stacking
  - Separate panel rendering logic
- Panel width management improvements
  - Fix redistribute after group deletion (8 locations)
  - Add zero-sum balance correction for resize operations
  - Fix auto-stacking calculation to use average width
  - Add proportional width redistribution across all groups

### Removed
- **BREAKING**: Removed `LayoutMode` (SimplePanel/MultiPanel) in favor of dynamic groups
- **BREAKING**: Changed panel navigation model from flat to hierarchical (groups + panels)

## [0.1.2] - 2025-11-29

### Added
- Duplicate line/selection feature (`Ctrl+D`)
- Replace operation feedback ("Replaced N occurrence(s)" message)
- File size validation before opening in editor (100 MB limit)
- Configurable tab size support (reads from `config.toml`)
- Crates.io publishing in release workflow

### Changed
- Migrate to semantic versioning tags without 'v' prefix (e.g., `0.1.2` instead of `v0.1.2`)
- Update license in Cargo.toml from dual (MIT OR Apache-2.0) to MIT only
- Improve error handling across the application
  - Replace panic with graceful error handling in theme parsing
  - Falls back to hardcoded default theme on parse errors
  - Better mutex error handling in terminal (22 `.lock().unwrap()` replaced with `.expect()`)
- Add clear error messages for oversized files

### Fixed
- Application crashes from invalid theme files
- Editor tab size now respects user configuration

## [0.1.1] - 2025-11-25

### Added
- Interactive search modal (`Ctrl+F`) with live preview and match counter
- Interactive replace modal (`Ctrl+H`) with dual input fields
- Tab/Shift+Tab navigation in search mode
- State preservation for search/replace queries
- `[X]` close button on editor panels
- Arrow key navigation between fields in replace modal

### Fixed
- Replace operation skipping matches on same line
- Inconsistent cursor positioning (now always at end of match with selection)
- Prev/Next navigation buttons resetting search state
- Escape key behavior (closes search first, then panel)
- Enter key in replace modal (now replaces instead of deleting)

### Changed
- Update match positions after replacement operations
- Standardize cursor positioning across all search/replace operations

## [0.1.0] - 2025-11-25

### Added
- Initial TermIDE release with complete feature set
- Terminal-based IDE with syntax highlighting for 15+ programming languages
  - Rust, Python, JavaScript, TypeScript, Go, C/C++, Java, Ruby, PHP
  - Haskell, Nix, HTML, CSS, JSON, TOML, YAML, Bash, Markdown
- Smart file manager with intuitive TUI interface
  - File type icons with attributes column
  - Symlink and executable file detection
  - Advanced keyboard and mouse selection controls
  - Recursive git status for directories
- Integrated virtual terminal with full PTY support
  - Ctrl+Shift+V paste with bracketed paste mode
  - 24 FPS rendering
  - Scrollback buffer and ANSI color support
- Multi-panel layout system
- Git integration
  - Background git status monitoring with file watching
  - Automatic updates on repository changes
  - Color-coded status indicators
  - Dimmed styling for gitignored files
  - Support for repository subdirectories
- 12 built-in themes
  - Dark: Default, Midnight, Dracula, OneDark, Monokai, Nord, Solarized Dark
  - Light: Atom One Light, Ayu Light, GitHub Light, Material Lighter, Solarized Light
  - Custom theme support from config directory
- System resource monitoring
  - Real-time CPU and RAM usage
  - Color-coded alerts
- Multi-language support (English, Russian)
  - Full Cyrillic keyboard layout support
  - Case-preserving hotkey translation
- Mouse support for all panels and UI elements
- Clipboard system for cut/copy/paste operations
- Batch file operations (copy, move, delete)
- Quit confirmation for unsaved changes and running processes
- Robust error handling and file size limits
- Automatic directory refresh on filesystem changes
- Cross-platform support (Linux, macOS, Windows via WSL)
- Multi-architecture builds (x86_64, ARM64)

### Technical
- Built with Rust using ratatui TUI framework
- Crossterm for cross-platform terminal manipulation
- Portable-pty for PTY implementation
- Tree-sitter for syntax highlighting
- Ropey for text buffer management
- Sysinfo for system resource monitoring
- Pre-commit hooks for code quality
- Comprehensive test suite

[0.5.0]: https://github.com/termide/termide/releases/tag/0.5.0
[0.4.0]: https://github.com/termide/termide/releases/tag/0.4.0
[0.3.0]: https://github.com/termide/termide/releases/tag/0.3.0
[0.2.0]: https://github.com/termide/termide/releases/tag/0.2.0
[0.1.5]: https://github.com/termide/termide/releases/tag/0.1.5
[0.1.4]: https://github.com/termide/termide/releases/tag/0.1.4
[0.1.3]: https://github.com/termide/termide/releases/tag/0.1.3
[0.1.2]: https://github.com/termide/termide/releases/tag/0.1.2
[0.1.1]: https://github.com/termide/termide/releases/tag/0.1.1
[0.1.0]: https://github.com/termide/termide/releases/tag/0.1.0
