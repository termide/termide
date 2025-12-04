# Contributing to TermIDE

Thank you for your interest in contributing to TermIDE! We welcome contributions from everyone, no matter how small. Whether you're fixing a typo, adding a feature, or improving documentation, your help is appreciated.

This document provides guidelines for contributing to TermIDE. Please read through it before submitting your contribution.

## Code of Conduct

This project adheres to the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code. Please report unacceptable behavior to the project maintainers.

## Ways to Contribute

There are many ways to contribute to TermIDE:

- 🐛 **Bug Reports** - Help us identify and fix issues
- 💡 **Feature Requests** - Suggest new features or improvements
- 📝 **Documentation** - Improve or translate documentation
- 🌍 **Translations** - Add support for new languages (we support English and Russian)
- 🎨 **Themes** - Create and share new color themes
- 🔧 **Code Contributions** - Fix bugs or implement new features
- 📢 **Spread the Word** - Star the project, share it with others

## Getting Started

### Prerequisites

- **Rust** 1.70 or later (stable toolchain)
- **Git** for version control
- A terminal emulator (ideally with true color support)

### Setup

Clone the repository:

```bash
git clone https://github.com/termide/termide.git
cd termide
```

Choose your preferred build method:

**Option 1: Standard (Cargo)**
```bash
cargo build
cargo run
```

**Option 2: Reproducible (Nix)**
```bash
nix develop  # Enter development environment
cargo build
```

For detailed setup instructions, architecture overview, and development guidelines, see the [Developer Guide](doc/en/developer-guide.md).

## Development Workflow

### Building and Testing

```bash
# Build the project
cargo build

# Run the application
cargo run

# Run tests
cargo test

# Run specific test
cargo test test_name

# Build for release
cargo build --release
```

### Pre-commit Hook

The project includes a pre-commit hook that automatically runs before each commit:

- ✅ Code formatting check (`cargo fmt --check`)
- ✅ Compilation check (`cargo check`)
- ✅ Clippy lints (`cargo clippy -- -D warnings`)
- ✅ Test suite (`cargo test`)

The hook is located at `.git/hooks/pre-commit` and is automatically configured when you clone the repository.

## Code Standards

### Formatting

We use `rustfmt` for consistent code formatting:

```bash
# Check formatting
cargo fmt --check

# Apply formatting
cargo fmt
```

All code must be formatted before committing. The pre-commit hook will enforce this.

### Linting

We use Clippy in **strict mode** to maintain code quality:

```bash
# Run clippy
cargo clippy -- -D warnings
```

**Important:**
- All warnings must be fixed before submitting a PR
- Use `#[allow(...)]` attributes only when absolutely necessary and document why
- Your PR will be rejected by CI if there are any clippy warnings

### Testing

All new features and bug fixes must include tests:

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run tests for a specific module
cargo test git::diff
```

**Test Requirements:**
- **Unit tests**: Add tests in the same file using `#[cfg(test)]` modules
- **Integration tests**: Use the `tests/` directory for integration tests
- **Bug fixes**: Include regression tests to prevent the issue from recurring
- **Doc tests**: Examples in rustdoc comments are automatically tested

### Documentation

- Public API functions must have rustdoc comments (`///`)
- Include examples in documentation when helpful
- Update user documentation in `doc/en/` and `doc/ru/` for user-facing changes
- Keep documentation clear, concise, and accurate

## Commit Message Guidelines

We follow the [Conventional Commits](https://www.conventionalcommits.org/) specification.

### Format

```
<type>: <description>

[optional body]

[optional footer]
```

### Types

- `feat:` - A new feature
- `fix:` - A bug fix
- `docs:` - Documentation changes
- `chore:` - Routine tasks (CI, dependencies, etc.)
- `refactor:` - Code refactoring without functional changes
- `test:` - Adding or updating tests
- `perf:` - Performance improvements
- `style:` - Code style changes (formatting, whitespace)

### Rules

- First line: ≤ 50 characters
- Use lowercase after the colon
- Use imperative mood ("add" not "added")
- No period at the end of the first line
- Wrap body at 72 characters if you add one

### Examples

```
feat: add Japanese language support

Implement i18n module for Japanese with all UI strings translated.
Includes proper pluralization rules and date formatting.

fix: resolve cursor position bug in multi-byte characters

The cursor was incorrectly positioned when editing lines containing
multi-byte UTF-8 characters (emoji, CJK, etc.). This fixes the issue
by using character indices instead of byte offsets.

Fixes: #123

docs: update installation guide for ARM64 Linux

Add instructions for installing on Raspberry Pi and ARM servers.
```

## Pull Request Process

### Before Submitting

1. **Ensure code quality:**
   ```bash
   cargo fmt
   cargo clippy -- -D warnings
   cargo test
   ```

2. **Update documentation** if your changes affect user-facing behavior

3. **Add tests** for new features or bug fixes

4. **Test manually** on different terminal sizes if you made UI changes

5. **Check cross-platform compatibility** if possible (Linux, macOS, WSL)

### Creating a Pull Request

1. **Fork** the repository
2. **Create a feature branch** from `main`:
   ```bash
   git checkout -b feat/amazing-feature
   ```
3. **Make your changes** with clear, descriptive commits
4. **Push** to your fork:
   ```bash
   git push origin feat/amazing-feature
   ```
5. **Open a Pull Request** with a clear description:
   - **What** changed and **why**
   - **How** to test the changes
   - Screenshots or GIFs for UI changes
   - Link to related issues (e.g., "Fixes #123")

### CI/CD Pipeline

All pull requests are automatically checked by our CI pipeline:

- ✅ **Formatting** - `cargo fmt --check`
- ✅ **Compilation** - `cargo check`
- ✅ **Clippy** - `cargo clippy -- -D warnings` (strict mode)
- ✅ **Tests** - `cargo test`

Your PR must pass all checks before it can be merged.

## Code Review Expectations

### For Contributors

- Be open to feedback and suggestions
- Respond to review comments in a timely manner
- Small improvements (nits) are not blockers - incremental improvement is valued
- Ask questions if something is unclear

### For Reviewers

- Focus on helping contributors succeed
- Clearly distinguish between blocking issues and suggestions (nits)
- Check in this order:
  1. Does the change provide value?
  2. Are there any significant issues?
  3. Does it follow code standards?
  4. Are tests adequate?
  5. Is documentation updated (if needed)?
  6. Are commit messages clear?

## Reporting Issues

### Bug Reports

Use the [Bug Report template](.github/ISSUE_TEMPLATE/bug_report.md) when reporting bugs.

**Include:**
- Operating system and version (Linux/macOS/WSL)
- TermIDE version (`termide --version`)
- Terminal emulator and size
- Steps to reproduce
- Expected vs. actual behavior
- Screenshots or error messages
- Relevant logs if available

**Log locations:**
- Linux: `~/.config/termide/termide.log`
- macOS: `~/Library/Application Support/termide/termide.log`
- Windows (WSL): `~/.config/termide/termide.log`

### Feature Requests

Use the [Feature Request template](.github/ISSUE_TEMPLATE/feature_request.md) when suggesting new features.

**Describe:**
- The problem your feature would solve
- Your proposed solution
- Alternative solutions you've considered
- Potential drawbacks or challenges

## Communication

- **GitHub Issues** - For bug reports, feature requests, and technical discussions
- **Pull Requests** - For code review and implementation discussions

**Languages:** We welcome communication in both **English** and **Russian**.

## Internationalization (i18n)

TermIDE supports multiple languages. Currently, English and Russian are fully supported.

### Adding a New Language

To add support for a new language:

1. Create a new module in `src/i18n/` (e.g., `ja.rs` for Japanese)
2. Implement the `Translation` trait with all strings from `src/i18n/en.rs`
3. Register your language in `src/i18n/mod.rs`
4. Update `src/config.rs` to support the new language code
5. Create documentation in `doc/<lang>/README.md` (optional but appreciated)
6. Test thoroughly, especially:
   - UI text rendering
   - Keyboard shortcuts (if your language uses different script)
   - Date/time formatting
   - Pluralization rules

### Translation Guidelines

- Maintain consistent terminology throughout the UI
- Keep translations concise - UI space is limited
- Use natural, idiomatic language
- Document any special pluralization rules in comments
- Test on actual terminals to ensure text fits properly

## Theme Contributions

TermIDE has a flexible theming system. You can create and contribute new themes.

### Creating a New Theme

1. Create a file in `themes/your-theme-name.toml`
2. Use existing themes as examples (see `themes/dracula.toml` or `themes/nord.toml`)
3. Test your theme with different panels:
   - File Manager
   - Text Editor
   - Terminal
   - Debug panel
4. Ensure good contrast and readability
5. Test in both light and dark terminal backgrounds

### Theme Structure

```toml
[colors]
background = "#1e1e2e"
foreground = "#cdd6f4"
cursor = "#f5e0dc"
# ... more color definitions

[syntax]
keyword = "#cba6f7"
string = "#a6e3a1"
# ... syntax highlighting colors
```

### Submitting a Theme

1. Add your theme file to `themes/` directory
2. Test it thoroughly
3. (Optional) Add a screenshot to `assets/screenshots/your-theme.png`
4. Update the theme list in `README.md`
5. Submit a pull request with the title: `feat: add [theme name] theme`

## Development Resources

- **Developer Guide**: [doc/en/developer-guide.md](doc/en/developer-guide.md) - Detailed technical documentation
- **Architecture**: [doc/en/architecture.md](doc/en/architecture.md) - System architecture overview
- **Code of Conduct**: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) - Community guidelines
- **Security Policy**: [SECURITY.md](SECURITY.md) - Security reporting and supported versions

## License

By contributing to TermIDE, you agree that your contributions will be licensed under the [MIT License](LICENSE).

---

## Thank You! ❤️

Every contribution, no matter how small, makes TermIDE better. We appreciate your time and effort in helping improve this project.

If you have questions or need help, don't hesitate to ask in GitHub Issues or Discussions. We're here to help!

Happy coding! 🚀
