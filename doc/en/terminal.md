# Terminal

The terminal panel provides a full-featured terminal emulator with pseudoterminal (PTY) support, ensuring compatibility with most console applications such as `bash`, `fish`, `htop`, and `mc`.

## Key Features

- **Interactive Shell**: Launches the default system shell (`fish`, `zsh`, `bash`, etc.) for command execution
- **Compatibility**: Supports `xterm-256color` and most standard ANSI control sequences, ensuring correct display of colors and text styles
- **Modern TUI Compatibility**: Responds to common terminal capability queries and supports negotiated keyboard/focus reporting used by applications such as `vim`, `neovim`, `yazi`, `htop`, and `lazygit`
- **Process Management**: When closing a terminal panel with running processes, the application will request confirmation before terminating them

## Interaction

| Shortcut               | Action                                     |
|------------------------|--------------------------------------------|
| `Ctrl+/`               | Open directory switcher                    |
| `Ctrl+F`               | Open text search in scrollback             |
| `Ctrl+C`               | Copy selection to clipboard **if text is selected**, otherwise sent to the shell as `SIGINT` |
| `Ctrl+V`               | Paste text from clipboard                  |
| `Shift+Enter`          | Insert newline (multi-line input)          |
| `Shift+PageUp`         | Scroll output history up                   |
| `Shift+PageDown`       | Scroll output history down                 |
| `Shift+Home`           | Go to beginning of output history          |
| `Shift+End`            | Go to current line (end of history)        |

**Keyboard Layout Support:**

TermIDE supports Cyrillic keyboard layouts for common shortcuts. When using a Russian/Cyrillic layout, paste (`Ctrl+V`) works without switching to a Latin layout — pressing it with the Cyrillic letter on the same physical key is recognized automatically.

All other key combinations are passed directly to the application running in the terminal.

When an application inside the terminal requests modern keyboard reporting, TermIDE switches from legacy xterm-style key encoding to the negotiated compatibility mode. This helps applications distinguish ambiguous combinations such as `Ctrl+I` vs `Tab`, `Ctrl+M` vs `Enter`, and modified `Esc`/`Backspace`.

**Modified arrow keys and Home/End** are encoded as the standard xterm CSI
`1;{mod}{final}` escape sequence (`{mod}` is the xterm modifier parameter:
`2` = Shift, `3` = Alt, `5` = Ctrl, `6` = Ctrl+Shift, and so on; `{final}`
is `A`/`B`/`C`/`D`/`H`/`F`). In practice this means `Ctrl+Left` / `Ctrl+Right`
trigger `backward-word` / `forward-word` in bash/zsh readline, `Shift+Home` /
`Shift+End` select to line boundaries where the shell supports it, and so on.
Plain arrows keep their existing path, including application-cursor-mode
substitution (`\x1bOA` vs `\x1b[A`). `Alt+Left` / `Alt+Right` remain bound
globally to previous/next panel group and therefore aren't forwarded.

## Text Search

Press `Ctrl+F` to open an inline find bar docked at the top of the panel (the
same UX as the editor and file manager), with a separator below it. The search
works across the entire scrollback buffer and the visible screen:

- **Live preview**: matches are highlighted as you type; the bar shows a match
  counter (e.g. "3 of 12")
- **Toggles**: `[Aa]` case sensitivity and `[.*]` regular-expression matching
  (click, or focus the button row and press `Enter` / `Space`)
- **Navigation**: the `◄ Prev` / `Next ►` buttons, `Enter`, or `F3` /
  `Shift+F3` step between matches; the viewport scrolls to the current match
- **Focus**: `Tab` switches focus between the bar and the terminal grid, so you
  can scroll the grid while the bar stays open
- **Refresh**: `Ctrl+R` re-runs the query against the current scrollback
- **Close**: `Escape`

The search keybinding defaults to `Ctrl+F` (instead of `Ctrl+Shift+F`) because most host terminals intercept `Ctrl+Shift+F` for their own search.

## Shell Selection

You can choose which shell to launch via the **Windows > Terminal** submenu. The submenu lists all shells detected on your system:

- **Linux/macOS**: shells from `/etc/shells`, plus common paths (`/usr/bin/fish`, `/usr/bin/zsh`, `/bin/bash`, `/bin/sh`) and NixOS-specific paths
- **Windows**: Git Bash, PowerShell Core (`pwsh`), Windows PowerShell, Command Prompt (`cmd`), and WSL distributions

The currently configured default shell is marked with **●**. Selecting a shell opens a new terminal with that shell and saves it as the default for future terminals.

You can also set the default shell in `config.toml`:

```toml
[terminal]
default_shell = "/usr/bin/fish"
```

## Mouse Support

- **Text Selection**: In the normal shell/scrollback view, click and drag with the left mouse button to select text, then `Ctrl+C` copies it. When the application inside the terminal enables xterm mouse tracking (e.g. an editor or agent), the mouse belongs to that application (it draws its own selection); hold `Alt+drag` for a local TermIDE selection instead, which `Ctrl+C` then copies
- **Double-click**: Select the word under the cursor; **triple-click**: select the whole line
- **Scroll Wheel**: Scroll through terminal output history until the application inside the terminal enables mouse tracking. After that, the wheel is passed through to the application
- **Ctrl+Click on URL/path**: Open link in browser or file manager
- **Ctrl+Click on hex color**: Show color preview popup (e.g. `#ff0000`, `#abc`) — visible while button is held, disappears on release
- **Application Interaction**: If a console application (e.g., `htop` or `mc`) enables xterm mouse tracking, TermIDE gives it priority for click, drag, move, and wheel events inside the terminal content area

## Application Compatibility Notes

- **Keyboard negotiation**: TermIDE answers the common keyboard capability queries used by modern TUI applications and supports negotiated `CSI u` / `modifyOtherKeys` compatibility modes when requested by the application
- **Focus reporting**: If an application enables xterm focus events (`?1004`), TermIDE forwards host terminal focus gain/loss as `CSI I` / `CSI O`
- **Terminal identity**: The inner PTY keeps the regular `xterm-256color` baseline and adds compatibility through runtime negotiation rather than by pretending to be a different terminal
