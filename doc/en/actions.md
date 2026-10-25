# Custom Commands

> **Note:** this document covers **user-defined commands** in the `Commands` menu. For keyboard shortcuts and UI actions, see [ui.md](ui.md#keyboard-navigation-and-panel-management).

A command is a named shell command line that Termide can run from the `Commands` menu or by its own hotkey: in a new terminal panel, silently in the background, or in the background with its output shown in a window when it finishes. Commands are defined in `commands.toml` files, either by hand or through the `Add command...` form.

## Where commands live

There are two files, and the menu shows both:

| Scope | File |
|-------|------|
| Global (all projects) | `commands.toml` in the Termide configuration directory |
| Project | `<project>/.termide/commands.toml` |

The configuration directory depends on the platform:

| Platform | Global file |
|----------|-------------|
| Linux | `~/.config/termide/commands.toml` (or `$XDG_CONFIG_HOME/termide/commands.toml`) |
| macOS | `~/Library/Application Support/termide/commands.toml` |
| Windows | `%APPDATA%\termide\commands.toml` |

The project file belongs to the current project root, so it can be committed and shared with everyone working on the repository.

The two files do not override each other: a project command and a global command with the same identifier are two separate menu entries. Project commands are listed first and drawn in bold.

Since the project file comes with the repository, a project command runs only once you have seen it. The first time one is started — from the menu, the palette, its hotkey or `F5` — a window shows what it runs (its `build`, then the command) and asks; agreeing runs it and approves it as it is, in termide's data directory under the project (`trust/<project path>/approved.toml`). If the repository changes the command, its mode, `build`, debugger or key, it asks again. Until a project command is approved, its hotkey asks only if it is held with `Ctrl` or `Alt` or is an F-key: pressing it shows the same window as the menu. A key text is typed with — a plain letter, `Shift`+letter, `Enter` — is bound only once the command is approved: start it once from the menu, and the key works from then on. A command you write in the `Add command...` or **Set up debugging** form is approved as you save it.

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
| `cwd` | no | `panel` (default) runs in the working directory of the focused panel; `project` runs in the project root |
| `group` | no | Puts the command into a submenu with this name |
| `key` | no | Hotkey that runs the command, e.g. `Ctrl+Shift+D`, see [Hotkeys](#hotkeys) |
| `params` | no | Values asked for before the command runs, see [Parameters](#parameters) |
| `debugger` | no | `lldb-dap`, `gdb-dap`, `debugpy`, `dlv-dap`, `lldb`, `gdb`, `dlv` or `pdb`: the command runs under this debugger, see [Debugging](#debugging) |
| `build` | no | With `debugger`: a shell command run first; the debugger starts only if it succeeds |
| `console` | no | With a debug adapter: `terminal` (the default) runs the program in a terminal panel, where it reads the keyboard; `panel` shows its output in the debug panel |

An unknown `mode` falls back to `terminal`. If a file cannot be parsed, none of its commands appear; the reason is written to the log.

## The Commands menu

The `Commands` menu in the menu bar lists, from top to bottom:

1. `Add command...` — opens the form for a new command.
2. Project commands without a group, then project groups (bold).
3. Global commands without a group, then global groups.

Within each part, commands and groups are sorted by identifier and group name. A group opens a submenu with its commands; clicking the group header again closes it. A command's hotkey is shown in the shortcut column. When the terminal supports emoji, each label is prefixed with its mode: 💻 terminal, ⚙ background, 📋 report, 🐞 a command with a `debugger`.

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
| `Run from the project folder` | `cwd`: ticked runs the command in the project root, left empty in the working directory of the focused panel |
| `Hotkey:` | Optional hotkey (`key`) |
| `Project command` | Checked: the command is saved to the project's `.termide/commands.toml`; unchecked: to the global file |

A new command's identifier is derived from `Menu item:`, or from `Command:` when the label is empty; the characters `/ \ : * ? " < > | .` are replaced with `-`. If a command with that identifier already exists, the new one gets a free variant (`build-2`) instead of replacing it. Emptying `Menu item:` while editing removes the label, so the menu shows the identifier. Toggling `Project command` while editing moves the command to the other file. The `Run from the project folder` box is `cwd`: ticked writes `cwd = "project"`, left empty the key is dropped and the command follows the focused panel. The form does not edit `params`; it is kept as it is in the file. Renaming (`F2`) onto the identifier of another command is refused.

Saving from the form, renaming or deleting edits `commands.toml` in place: the other commands, their order and the comments around them stay as they are, only the fields that changed are rewritten, and a new command is added at the end. A renamed command moves to the end of the file.

## Execution modes

A command with a `debugger` runs in the project root, in the debug panel or a terminal panel, whatever its `mode`, see [Debugging](#debugging). Every other command runs in the working directory of the focused panel (for example, the directory shown in a file manager or the current directory of a terminal), or in the project root if the panel has none. With `cwd = "project"` a command runs in the project root whatever panel is focused: that is the static point its relative paths are written against, the folder its own `commands.toml` sits in. Where a command runs is part of what an approval covers — changing `cwd` asks for the project's command again.

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

A command can declare parameters. Before it runs, Termide shows a form titled `Parameters: <id>` with one field per parameter and the buttons `Run` and `Cancel`. Each value goes into the command line where its [placeholder](#placeholders) `{name}` stands, and is also passed as an environment variable `TERMIDE_PARAM_<NAME>`: the name upper-cased, with `-` replaced by `_`.

```toml
[deploy]
name = "Deploy"
command = "./deploy.sh {target} {dry-run}"
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
| `name` | Required; the placeholder is `{name}`, the variable `TERMIDE_PARAM_<NAME>` |
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

## Placeholders

`{name}` in `command` (and in `build`) is replaced before the command runs: by the value of the parameter `name`, or by something of the focused panel's or the project's. A parameter of the same name takes precedence.

| Placeholder | Value |
|-------------|-------|
| `{file}` | The editor's file; in a file manager, the entry under the cursor |
| `{files}` | The files selected in a file manager, each a word of its own; the entry under the cursor when none is selected; in an editor, its file |
| `{line}`, `{col}` | The editor's cursor, 1-based |
| `{dir}` | The working directory of the focused panel |
| `{project}` | The project root |

Paths are absolute. Without a file to give — a terminal focused, say, or a file on a remote server — a command using `{file}`, `{files}`, `{line}` or `{col}` does not run, and a message says so; it says so before the parameter form opens.

```toml
[p4-edit]
name = "p4 edit"
command = "p4 edit {file}"
key = "Alt+E"
```

Each value is quoted for whatever reads the line, so a path with spaces, quotes or `;` stays one argument and is never run as a command:

- `background` and `report` commands run through `sh -c`: POSIX quoting.
- `terminal` commands are quoted for the terminal's shell: POSIX for sh, bash, zsh and the like, fish's own rules for fish, `'…'` for PowerShell, `"…"` for cmd. cmd has no literal quoting, so a `%name%` in a value is still expanded there.
- A debug command's line is split into words first and the values put into them, each one word (`{files}` as many); its `build` gets POSIX quoting.

A placeholder may stand anywhere: as a word of its own (`p4 edit {file}`), in quotes (`"{file}"`) or inside a longer quoted string (`"at {file}"`, `'at {file}'`). Its value is quoted to match where it stands, so it is taken literally in each case: a file named `$(rm -rf ~).c` is only a name. A `{files}` in quotes gives one word, as anything in quotes does. A placeholder escaped for the shell (`\{file}` in sh, `` `{file} `` in PowerShell, `^{file}` in cmd) is left as written. Only the names above and the command's parameters are replaced: other braces stay as written, so `awk '{print $1}'`, `docker inspect -f '{{.Name}}'`, brace expansion and `${VAR}` are left alone.

## Hotkeys

`key` uses the same notation as the keybindings in `config.toml` (see [keybindings.md](keybindings.md)), e.g. `Ctrl+Shift+D` or `Alt+F6`. `Ctrl+Shift+<letter>` reaches Termide only in terminals with the Kitty keyboard protocol (see [Universal vs Enhanced bindings](keybindings.md#universal-vs-enhanced-bindings)). A command hotkey is checked together with the global Termide shortcuts, so it works whichever panel has focus. A project command and a global command with the same identifier keep separate hotkeys.

Commands with a hotkey are also listed in the command palette (`Ctrl+P`) as `Run command: <label>`.

The form's `Hotkey:` field accepts `Ctrl`, `Alt` and `Shift` combined with a letter, a digit, `F1`–`F12` or a named key (`Enter`, `Tab`, `Space`, `Home`, `PageUp`, arrows and so on), and it rejects a hotkey that another command or a global Termide shortcut already uses (`Hotkey is already in use`). A hotkey written into the file by hand is not checked: if it matches a global shortcut, the shortcut wins and the command never runs from the keyboard. It takes effect the next time the `Commands` menu is opened or the file is saved from Termide.

## Debugging

A command with `debugger` debugs a program with the breakpoints placed in the editor (see [Breakpoints](editor.md#breakpoints-and-debugging)) set, and runs it to the first one. `command` is then what to debug. Two kinds of debugger are supported:

- a **debug adapter**, driven by Termide over the Debug Adapter Protocol and shown in the [debug panel](#the-debug-panel): `lldb-dap`, `gdb-dap`, `debugpy`, `dlv-dap`;
- a **command-line debugger** started in a new terminal panel, where you step, print and continue with its own commands: `lldb`, `gdb`, `dlv`, `pdb`.

| `debugger` | `command` is | Runs as |
|------------|--------------|---------|
| `lldb-dap` | the program and its arguments | LLVM's `lldb-dap` (on macOS also found through `xcrun`; `lldb-vscode` in older LLVM): Rust, C, C++ |
| `gdb-dap` | the program and its arguments | `gdb -i dap`, GDB 14 or later |
| `debugpy` | the script and its arguments | `python3 -m debugpy.adapter`; needs `pip install debugpy` |
| `dlv-dap` | the package to build and its arguments, e.g. `./cmd/app --flag` | `dlv dap`, Delve's adapter, reached over a local TCP port; where `/proc/net/tcp` cannot be read (Termux on Android), with `--only-same-user=false`, as Delve's same-user check would refuse every connection there |
| `lldb` | the program and its arguments | `lldb -s <script> -- <command>` |
| `gdb` | the program and its arguments | `gdb -q -x <script> --args <command>` |
| `dlv` | Delve's subcommand and its arguments, e.g. `debug ./cmd/app` | `dlv --init <script> <command>` |
| `pdb` | the script and its arguments | `python3 -m pdb -c 'break <file>:<line>' … -c continue <command>` |

```toml
[debug-app]
name = "Debug app"
command = "target/debug/app --verbose"
debugger = "lldb-dap"
build = "cargo build"
key = "Ctrl+Alt+D"
```

`debugger_args` gives the debugger flags of its own, as an array of words, added to those Termide starts it with:

```toml
[debug-go]
command = "./cmd/app"
debugger = "dlv-dap"
debugger_args = ["--log", "--log-output=dap"]
```

They go where the debugger reads its options, before the program: an adapter gets them after its own arguments (`dlv dap --listen=… --log …`, `gdb -i dap …`), `lldb` and `gdb` before `--` and `--args`, `dlv` before its subcommand, `pdb` before its `-c`. The program's arguments stay in `command`.

For a debug adapter, `command` is split into words as a shell would split it (quotes and backslashes work) but no shell runs it: variables, `~` and globs are not expanded. `build` runs first, its output going to the debug panel; the adapter starts only if it succeeds. A command-line debugger reads its breakpoints from a script — a `breakpoint set` or `break` for each, then `run` (lldb, gdb) or `continue` (Delve) — written to `termide-debug` in the system's temporary directory; breakpoints set while it runs reach it only on the next launch, so set them in the debugger itself meanwhile.

Only breakpoints in files under the project root go in, each to the debugger for its language: Python files to debugpy and pdb, Go files to Delve and dlv-dap, the rest to lldb and gdb. Debug commands run in the project root, so a relative program path such as `target/debug/app` means the same wherever the focus is. Parameters work as for other commands; for a debug adapter they reach the build and the adapter as environment variables.

`F5` in the editor (`start_debug`), or **Start debugging** in the command palette, starts debugging: with one launch available it starts at once, with several it offers them as a list. The launches are the debug commands of the project and the global file and, when the focused editor holds a Python, Go or Rust file, that file itself, which needs no command:

- a Python script runs under debugpy in the debug panel when debugpy is installed and under pdb in a terminal otherwise;
- the package in a Go file's directory runs under `dlv dap` in the debug panel when Delve is installed and under `dlv debug` in a terminal otherwise;
- a Rust file in a Cargo project offers the binaries found with `cargo metadata`: the one whose main file it is, or else those of its package, or else every binary of the workspace. Each is built with `cargo build -p <package> --bin <name>` and run from the workspace root under `lldb-dap`, or GDB's adapter, or `lldb` in a terminal, whichever is installed first.

When a file could be debugged but its debugger is missing (no LLDB or GDB for Rust, no Delve for Go), a window says what to install. When there is nothing to launch at all, the **Set up debugging** form opens instead (see below). While a debug session is active, `F5` continues the program instead.

**Set up debugging…**, in the editor's `[≡]` menu and the command palette, opens a form for a launch of your own: its name, the program and its arguments, the command to build it first, the debugger and its flags (split as a shell would), filled in for the file in the editor — for a Rust binary its path under `target/debug` and its `cargo build`. **Run** saves it as a debug command in the project's `.termide/commands.toml` and starts it; from then on `F5` offers it, and it can be edited there or in the Commands menu, for example to add arguments.

### Debugger settings

How each debugger starts is set in `config.toml`, apart from what is debugged. A project's `.termide/config.toml` may set `native` and `console`; the debuggers' `command` and `args` come from the global file only, since they name the program termide starts:

```toml
[debug]
native = ["lldb-dap", "gdb-dap", "lldb"]  # Rust, C, C++ without a command: the first installed
console = "terminal"                      # or "panel": where an adapter runs the program by default

[debug.debuggers.lldb-dap]
command = "/opt/homebrew/opt/llvm/bin/lldb-dap"  # in place of the one on PATH (or xcrun)

[debug.debuggers.dlv-dap]
args = ["--log"]                                 # flags for every launch
```

- `command` names the debugger's program: a path (`~/` stands for the home directory), or a name looked up on `PATH`. For `debugpy` and `pdb` it is the Python to run. A path that does not exist makes the debugger count as not installed rather than falling back to another.
- `args` are the debugger's flags for every launch, in place of its default ones; a launch's `debugger_args` come after them. Unset keeps the defaults, `[]` drops them. The only default today is `--only-same-user=false` for `dlv-dap` where `/proc/net/tcp` cannot be read (Termux on Android): Delve's same-user check would refuse every connection there. Setting `args` for `dlv-dap` in such a system replaces it, so list it again if you still need it.
- The arguments Termide needs to talk to an adapter (`dap --listen=…` for Delve, `-i dap` for GDB, `-m debugpy.adapter`) and its scripts for command-line debuggers are always given and cannot be changed.
- `native` is tried in order for a Rust file debugged without a command; it takes `lldb-dap`, `gdb-dap`, `lldb` and `gdb`.
- `console` applies to launches that do not set their own `console`.

The **Debug** tab of the settings window edits the same: the order and the console, then every debugger Termide knows with its program and flags, the defaults shown as such. A debugger opens on a page of its own; its flags are typed as a shell would split them (quotes keep a value whole, commas are part of a flag), emptying them means no flags, and **Reset** brings back the program on `PATH` and the default flags — in a project's settings, the global ones.

### The debug panel

A launch through a debug adapter opens the debug panel (or reuses the open one). Its top line shows the session and where it stands — building, starting, running, stopped and why, exited with its code, or failed with the reason — and the keys. Below it are three sections:

- **Call stack** — the frames of the thread that stopped, innermost first, each with its file and line. `Enter` or a click on a frame shows its variables and its line in an editor.
- **Variables** — the scopes of the selected frame; the first one that is cheap to fetch opens by itself. `Enter`, `Space` or `→` opens a variable with members (fetched the first time), `←` closes it or goes to its parent; a second click on a row toggles it.
- **Output** — the program's standard output and error, the adapter's messages, the build's output and evaluated expressions with their values. It follows new output; `↑`/`PgUp` scroll back and `Enter` returns to the end.

| Key | Action |
|-----|--------|
| `c` / `F5` | Continue |
| `n` | Step over |
| `s` | Step into |
| `o` | Step out |
| `p` | Pause the running program |
| `x` | Stop: end the session and the program |
| `r` | Restart: run the launch again — its `build` first, with the same parameters — ending the session if it still runs |
| `e` | Evaluate an expression in the selected frame |
| `Tab` / `Shift+Tab` | Next / previous section |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | Move in the section |

When the program stops, the editor that has the file moves its cursor to the line, scrolling only as far as needed, and the gutter marks it with `→` on a highlighted cell; a file not open yet opens in a new editor, the focus staying on the panel. Breakpoints set or removed while the session runs reach the adapter at once. Closing the panel or starting another launch ends the session and its program.

### The program's terminal

By default (`console = "terminal"`) the program runs in a terminal panel of its own, opened beside the others: it reads the keyboard there — focus that panel to type to it — and its output, colours and full-screen interfaces show there as in any terminal, while the debug panel keeps the adapter's messages. debugpy and Delve ask Termide to start the program in that panel; lldb-dap starts it itself and is told the panel's terminal device through lldb's `target.input-path`, `output-path` and `error-path` settings, which every lldb-dap version takes. `Ctrl+C` in a terminal lldb-dap uses is not passed to the program: pause it with `p` in the debug panel. GDB's adapter has no such mode, so its programs always write to the debug panel and read nothing; so do the sessions an agent starts, whose output it reads from the session. `console = "panel"` asks for that for any adapter, for a program that does not read input.

## Tips

- Commands run through `sh -c` (or the terminal's shell), so pipes, `&&`, redirections and environment variables work as in a shell. On Windows, `background` and `report` commands need `sh` on `PATH`.
- Use `report` for short checks whose result you want to read (`git status`, a linter), `background` for processes whose output you don't need, and `terminal` for anything interactive or long-running that you want to watch.
- Keep project-specific commands in `.termide/commands.toml` in the repository and personal ones in the global file.
