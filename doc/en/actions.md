# Custom Commands

> **Note:** this document covers **user-defined commands** in the `Commands` menu. For keyboard shortcuts and UI actions, see [ui.md](ui.md#keyboard-navigation-and-panel-management).

A command is a named shell command line that TermIDE can run from the `Commands` menu or by its own hotkey: in a new terminal panel, silently in the background, or in the background with its output shown in a window when it finishes. Commands are defined in `commands.toml` files, either by hand or through the `Add command...` form.

## Where commands live

There are two files, and the menu shows both:

| Scope | File |
|-------|------|
| Global (all projects) | `commands.toml` in the TermIDE configuration directory |
| Project | `<project>/.termide/commands.toml` |

The configuration directory depends on the platform:

| Platform | Global file |
|----------|-------------|
| Linux | `~/.config/termide/commands.toml` (or `$XDG_CONFIG_HOME/termide/commands.toml`) |
| macOS | `~/Library/Application Support/termide/commands.toml` |
| Windows | `%APPDATA%\termide\commands.toml` |

The project file belongs to the current project root, so it can be committed and shared with everyone working on the repository.

The two files do not override each other: a project command and a global command with the same identifier are two separate menu entries. Project commands are listed first and drawn in bold.

## File format

Each TOML table is one command; the table name is the command's identifier.

```toml
[build]
command = "cargo build --release"

[test]
name = "Run tests"
command = "cargo nextest run"
group = "cargo"
key = "Ctrl+Shift+T"

[clippy]
command = "cargo clippy --workspace -- -D warnings"
mode = "report"
group = "cargo"

[dev-server]
name = "Start dev server"
command = "npm run dev > /tmp/dev-server.log 2>&1"
mode = "background"
```

| Field | Required | Description |
|-------|----------|-------------|
| `command` | yes | Shell command line to run |
| `name` | no | Label in the menu; the identifier is shown when it is absent |
| `mode` | no | `terminal` (default), `background` or `report`, see [Execution modes](#execution-modes) |
| `group` | no | Puts the command into a submenu with this name |
| `key` | no | Hotkey that runs the command, e.g. `Ctrl+Shift+D`, see [Hotkeys](#hotkeys) |
| `params` | no | Values asked for before the command runs, see [Parameters](#parameters) |

An unknown `mode` falls back to `terminal`. If a file cannot be parsed, none of its commands appear; the reason is written to the log.

## The Commands menu

The `Commands` menu in the menu bar lists, from top to bottom:

1. `Add command...` — opens the form for a new command.
2. Project commands without a group, then project groups (bold).
3. Global commands without a group, then global groups.

Within each part, commands and groups are sorted by identifier and group name. A group opens a submenu with its commands; clicking the group header again closes it. A command's hotkey is shown in the shortcut column. When the terminal supports emoji, each label is prefixed with its mode: 💻 terminal, ⚙ background, 📋 report.

Keys on a selected command:

| Key | Action |
|-----|--------|
| `Enter` | Run the command |
| `F2` | Rename (change the identifier) |
| `F4` | Edit in the command form |
| `Delete` / `F8` | Delete, after confirmation |

The menu rereads both files every time it opens, so commands added to a file by hand show up without a restart.

## Adding and editing commands

`Add command...` and `F4` open the same form:

| Field | Meaning |
|-------|---------|
| `Group:` | Group name; suggests the existing groups. Empty means the top level of the menu |
| `Menu item:` | Menu label (`name`) |
| `Command:` | Shell command line (`command`); required when creating |
| `Mode:` | `Terminal`, `Background` or `Report`; switch with `←` / `→` or `1`–`3` |
| `Hotkey:` | Optional hotkey (`key`) |
| `Project command` | Checked: the command is saved to the project's `.termide/commands.toml`; unchecked: to the global file |

A new command's identifier is derived from `Menu item:`, or from `Command:` when the label is empty; the characters `/ \ : * ? " < > | .` are replaced with `-`. If a command with that identifier already exists, the new one gets a free variant (`build-2`) instead of replacing it. Emptying `Menu item:` while editing removes the label, so the menu shows the identifier. Toggling `Project command` while editing moves the command to the other file. The form does not edit `params`; they are kept as they are in the file. Renaming (`F2`) onto the identifier of another command is refused.

Saving from the form, renaming or deleting edits `commands.toml` in place: the other commands, their order and the comments around them stay as they are, only the fields that changed are rewritten, and a new command is added at the end. A renamed command moves to the end of the file.

## Execution modes

Every command runs in the working directory of the focused panel (for example, the directory shown in a file manager or the current directory of a terminal), or in the project root if the panel has none.

| Mode | How it runs | Output |
|------|-------------|--------|
| `terminal` | A new terminal panel opens and the command line is typed into its shell | In the terminal; the shell stays open after the command ends |
| `background` | `sh -c "<command>"` without a panel | Discarded |
| `report` | `sh -c "<command>"` without a panel | Captured and shown in a window when the command ends |

Background and report commands appear in the [Operations](operations.md) panel, which opens when such a command starts. The entry disappears when the process exits; cancelling it there kills the process together with its children. A background command gives no other signal on completion, so redirect its output to a file if you need it. On Unix, when `direnv` is installed, background and report commands also get the environment that `direnv export json` returns for the working directory.

### Report window

When a report command finishes, a window titled with the command's label and `✓` (exit code 0) or `✗` (any other exit code) shows its output: standard output first, then standard error. Indentation and blank lines inside the output are kept, tabs are expanded to four spaces, and blank lines before and after each stream are dropped; a command with no output shows `(no output)`. The window opens in whichever project is current; a command started in another project adds that project's path to the title. If several report commands finish at the same moment, only the last one's window is shown.

| Key | Action |
|-----|--------|
| `↑` / `↓` (`k` / `j`) | Scroll one line |
| `PageUp` / `PageDown` | Scroll one page |
| `Home` / `End` | Jump to start / end |
| Mouse wheel | Scroll |
| `Enter` / `Escape` | Close the window |

## Parameters

A command can declare parameters. Before it runs, TermIDE shows a form titled `Parameters: <id>` with one field per parameter and the buttons `Run` and `Cancel`. Each value is passed to the command as an environment variable `TERMIDE_PARAM_<NAME>`: the name upper-cased, with `-` replaced by `_`.

```toml
[deploy]
name = "Deploy"
command = "./deploy.sh \"$TERMIDE_PARAM_TARGET\" \"$TERMIDE_PARAM_DRY_RUN\""
mode = "report"
key = "Ctrl+Shift+D"

[[deploy.params]]
name = "target"
label = "Target environment"
type = "select"
options = ["staging", "production"]
default = "staging"

[[deploy.params]]
name = "dry-run"
label = "Dry run"
type = "bool"
default = true
```

| Field | Description |
|-------|-------------|
| `name` | Required; the variable is `TERMIDE_PARAM_<NAME>` |
| `label` | Field label in the form; defaults to `name` |
| `type` | `text` (default), `number`, `bool` or `select` |
| `options` | Choices for `select` |
| `default` | Initial value: a string, number or boolean |

| Type | Field | Value passed |
|------|-------|--------------|
| `text` | Text input | The text as typed |
| `number` | Text input | The text as typed; it is not checked to be a number |
| `bool` | Checkbox, toggled with `Space` / `Enter` or a click | `true` or `false` |
| `select` | Choice switched with `←` / `→`; starts at `default`, otherwise at the first option | The selected option |

The form is shown however the command is started: from the `Commands` menu, by its hotkey or from the command palette. The variables reach the command in every mode; in `terminal` mode they are set in the new terminal's shell, so they stay there after the command ends.

## Hotkeys

`key` uses the same notation as the keybindings in `config.toml` (see [keybindings.md](keybindings.md)), e.g. `Ctrl+Shift+D` or `Alt+F5`. `Ctrl+Shift+<letter>` reaches TermIDE only in terminals with the Kitty keyboard protocol (see [Universal vs Enhanced bindings](keybindings.md#universal-vs-enhanced-bindings)). A command hotkey is checked together with the global TermIDE shortcuts, so it works whichever panel has focus. A project command and a global command with the same identifier keep separate hotkeys.

Commands with a hotkey are also listed in the command palette (`Ctrl+P`) as `Run command: <label>`.

The form's `Hotkey:` field accepts `Ctrl`, `Alt` and `Shift` combined with a letter, a digit, `F1`–`F12` or a named key (`Enter`, `Tab`, `Space`, `Home`, `PageUp`, arrows and so on), and it rejects a hotkey that another command or a global TermIDE shortcut already uses (`Hotkey is already in use`). A hotkey written into the file by hand is not checked: if it matches a global shortcut, the shortcut wins and the command never runs from the keyboard. It takes effect the next time the `Commands` menu is opened or the file is saved from TermIDE.

## Tips

- Commands run through `sh -c` (or the terminal's shell), so pipes, `&&`, redirections and environment variables work as in a shell. On Windows, `background` and `report` commands need `sh` on `PATH`.
- Use `report` for short checks whose result you want to read (`git status`, a linter), `background` for processes whose output you don't need, and `terminal` for anything interactive or long-running that you want to watch.
- Keep project-specific commands in `.termide/commands.toml` in the repository and personal ones in the global file.
