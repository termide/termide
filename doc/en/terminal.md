# Terminal

The terminal panel provides a full-featured terminal emulator with pseudoterminal (PTY) support, ensuring compatibility with most console applications such as `bash`, `fish`, `htop`, and `mc`.

## Key Features

- **Interactive Shell**: Launches the default system shell (`fish`, `zsh`, `bash`, etc.) for command execution
- **Compatibility**: Supports `xterm-256color` and most standard ANSI control sequences, ensuring correct display of colors and text styles
- **Process Management**: When closing a terminal panel with running processes, the application will request confirmation before terminating them

## Interaction

| Shortcut               | Action                                     |
|------------------------|--------------------------------------------|
| `Shift+PageUp`         | Scroll output history up                   |
| `Shift+PageDown`       | Scroll output history down                 |
| `Shift+Home`           | Go to beginning of output history          |
| `Shift+End`            | Go to current line (end of history)        |
| `Ctrl+Shift+V`         | Paste text from clipboard                  |
| `Ctrl+Shift+М`         | Paste text from clipboard (Cyrillic layout)|

**Keyboard Layout Support:**

TermIDE supports Cyrillic keyboard layouts for common shortcuts. When using a Russian/Cyrillic layout, you can use `Ctrl+Shift+М` (where М is the Cyrillic letter corresponding to V) instead of switching to Latin layout. This works for paste operations in the terminal.

All other key combinations are passed directly to the application running in the terminal.

## Mouse Support

- **Text Selection**: Click and hold the left mouse button to select text. Selected text is automatically copied to the clipboard after releasing the button
- **Scroll Wheel**: Scroll through terminal output history
- **Application Interaction**: If a console application (e.g., `htop` or `mc`) supports mouse input, the terminal will pass mouse events to it
