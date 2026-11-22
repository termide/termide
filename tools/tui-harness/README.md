# TUI harness

Live checks of termide without a terminal window: the built binary runs in
a pty of a fixed size, a [pyte](https://github.com/selectel/pyte) screen
reads what it draws, and a scenario script presses keys, waits for text and
checks the screen. A UI or agent change can be checked end to end by an
agent or in a script, the same way each time.

```sh
cargo build                                # the harness runs target/debug/termide
tools/tui-harness/run.sh                   # every scenario in scenarios/
tools/tui-harness/run.sh agent terminal    # these ones
tools/tui-harness/run.sh -v --keep my.scn  # a scenario file anywhere; keep its scratch dir
```

`run.sh` makes a Python venv with pyte under `target/tui-harness/venv` on
first use (it needs network once), then runs `harness.py`. Each scenario
prints `ok` or `FAIL` with the step that failed and the screen at that
moment; the exit code is non-zero when one failed. Its screens land in
`target/tui-harness/out/<scenario>/`: the `snapshot`s, `final.txt` (the
screen when it ended) and `output.bin` (every byte termide wrote). `--bin`
or `TERMIDE_BIN` picks another binary, e.g. a release build;
`CARGO_TARGET_DIR` is honoured. A run of all of them takes under a
minute.

## What each run gets

- **A throwaway HOME** under `/tmp`, removed afterwards: a fresh config
  (`config.toml` here, close to the defaults), no sessions or layouts from
  earlier runs, a demo git identity, `bash` with the prompt `demo$ `. On
  macOS termide keeps its config in `~/Library/Application Support`, which
  is under that HOME as well, so nothing of the user's is read or written.
- **The demo project** of the screenshots
  (`tools/screenshots/fixtures/make-demo.sh`): a small Python service with
  a git history, a worktree, uncommitted changes and a SQLite database.
  `project empty` gives an empty directory instead.
- **`--no-lsp`**, so no language server starts or downloads anything,
  unless the scenario says `lsp`.
- **Mock agents** on request: `mock-llm` starts
  `tools/screenshots/mock-llm/server.py` on a free port and points the
  `local` connection at it; it plays a fixed task (read
  `inventory/report.py`, edit it, run the tests, answer), or, asked to
  "delegate the stock scan", hands a task to a subagent in the background
  and quotes its report ("… and again" hands it over a second time when the
  first report comes back); asked to "break the count", it makes a number a
  string in a Cargo project's `src/main.rs` and repeats what the edit's
  result said of the language server's errors. It logs the model every call asks for. `mock-acp codex`
  (or `claude_code`, `gemini_cli`) makes that ACP agent the connection and
  puts the stand-in adapters of `tools/screenshots/mock-acp` first on
  `PATH`; `mock-acp claude_code agent` only puts them on `PATH`, for an
  `AGENT.md` whose `command` starts one. Neither touches the network.

## Scenarios

One step per line, words split as a shell does; `#` starts a comment.
Timeouts are in seconds and default to 10.

| Step | What it does |
|---|---|
| `size 120x36` | Screen size (before `start`; the default) |
| `project demo` \| `empty` | The project termide starts in (before `start`) |
| `setup "COMMAND"` | Run a shell command in the project once it is made, before termide starts, as `run` does (before `start`) |
| `config FILE` | Use FILE (relative to the scenario) instead of `config.toml`; `{mock_llm_port}` in it is filled in |
| `mock-llm [flavor]` | Start the mock model: `default`, `llamacpp`, `omlx` or `openrouter` |
| `mock-acp PROVIDER [agent]` | Connect the `claude_code`, `codex` or `gemini_cli` stand-in; with `agent`, only put the stand-ins on `PATH` |
| `lsp` | Let language servers run (no `--no-lsp`); a rustup-installed one keeps the real `RUSTUP_HOME` and `CARGO_HOME` (before `start`) |
| `start [ARGS...]` | Run termide in the project, waiting for its first frame; `{project}` in ARGS is the project's path |
| `key K...` | Press keys: `Enter`, `Esc`, `Tab`, `Alt+T`, `Ctrl+S`, `Shift+Up`, `Alt+F11`, `PgDn`, `x` |
| `type "TEXT"` | Type text, one character at a time |
| `paste "TEXT"` | Paste text as a bracketed paste |
| `wait "TEXT" [T]` | Wait until TEXT is on the screen |
| `wait-re "REGEX" [T]` | Wait until the screen matches REGEX (multi-line mode) |
| `wait-gone "TEXT" [T]` | Wait until TEXT is no longer on the screen |
| `expect "TEXT"` / `expect-not "TEXT"` / `expect-re "REGEX"` | Check the screen now |
| `expect-row N "REGEX"` | Check row N (0 is the top, -1 the bottom) |
| `file-contains PATH "TEXT" [T]` | Wait until the project's file PATH holds TEXT |
| `mock-llm-asked MODEL [T]` | Wait until the mock model has had a call for MODEL: where a side model setting sent it |
| `mock-acp-got "TEXT" [T]` | Wait until the ACP stand-in's log holds TEXT: the Claude Code one logs each `session/new` (with `plan-in-system-prompt` when its system prompt holds plan mode's instructions), each `session/fork` with the session it branches off, and each prompt block as `prompt: …`, and each tool termide's MCP server lists to it as `tool: …` |
| `mock-acp-count "TEXT" N` | Check that the ACP stand-in's log holds TEXT exactly N times |
| `run "COMMAND"` | Run a shell command in the project, outside termide; in the directory that held it once the project has moved away. `$TERMIDE_CONFIG` and `$TERMIDE_DATA` name termide's config and data directories, `$TERMIDE_BIN` the binary under test (for `--prompt`) |
| `snapshot NAME` | Save the screen as text |
| `sleep S` | Let termide run for S seconds |
| `quit [--force]` | `Alt+Q` and wait for termide to exit; `--force` confirms a "Quit anyway?" |

A scenario that does not `quit` is killed at its end. The examples:
`smoke` (start and quit), `editor` (file manager → editor → save),
`terminal` (a command in a terminal panel), `terminal-keys` (Alt, Esc and Ctrl+F reaching the shell and less, the title following them), `terminal-commands` (bash's prompts and commands marked: a failed one on the left border, `Ctrl+Shift+Up`/`Down` from prompt to prompt, the last command sent to the Claude Code stand-in), `debug-env` (a debug command's `env`, a placeholder filled in, reaching a C program under lldb-dap over the variables direnv sets; needs a C compiler, lldb-dap and direnv), `git-diff-staging` (a hunk and lines staged from the diff panel, a discard confirmed and undone, `Ctrl+O` folding every file and unfolding them), `git-status-folds` (`Ctrl+O` in git status folding every folder at every depth and opening them again), `editor-git-changes` (F7 to a change, staged, unstaged and reverted from the editor's menu), `editor-git-crlf` (a file committed with CRLF breaks: after an edit only the edited line offers staging, F7 goes to it), `file-manager-trash` (`Delete` moves a file to the system trash after a prompt on Yes, `Shift+Delete` deletes for good after one on No; on macOS the file lands in the user's own Trash), `file-manager-taken-names` (a new file under a taken name refused with the file kept whole; Save As onto another file asking first, No keeping it and Yes replacing it), `file-manager-copy-conflict` (a copy onto files that exist: `Esc` on the conflict ends the copy, writing nothing and not asking again; Rename All putting every conflicting file under its pattern), `file-manager-copy-conflict-running` (a conflict the running copy raises, a file put in its way while it scans a large folder: Rename asks for the name, `Esc` on that prompt asks about the conflict again, the name typed is where the folder lands), `remote-smb-nfs` (an `nfs://` and an `smb://` address connected to, not refused as unsupported, each saying why the connection failed when no server listens), `editor-vim-hint` (in Vim mode `z` and `2d` show the keys that may follow in a box, the next key or Esc takes it away and Esc leaves the panel open), `editor-folding` (`Alt+,` folds a block by its indentation, the arrows step over it, a search opens it, the menu folds and unfolds everything), `editor-folding-lsp` (rust-analyzer's folding ranges: the run of `use` lines, which the indentation would not fold, and a function body; needs rust-analyzer), `agent` (the mock model's task
with a permission card), `agent-rule-scopes` (the same task's edit card
answered "always", its second step listing the file, its folder and the
whole project, `Esc` back to the first step), `agent-artifact-card` (a Markdown file written,
its block's `Open in Markdown ↗` card), `acp-codex` (the same task through Codex over
ACP), `agent-review` (the Claude Code stand-in's edit reviewed with
`/changes`, a hunk rejected and the agent told of it), `agent-snippets`
(an editor's selection and a terminal's last lines sent from their menus
and with `@term`, as the Claude Code stand-in receives them),
`agent-lsp-errors` and `agent-lsp-new-errors` (rust-analyzer's errors in
the mock model's edit result, for a file no editor has, and the new ones
alone for one an editor has; both need rust-analyzer on `PATH`), `delegate` (a background subagent's row and report, on the model
`[ai.subagents]` names), `agent-message-limit` (past
`max_agent_messages` the card asking whether to allow more, and the task
going on once allowed), `agent-send-message` (a message to another
agent asking every time, its card showing whom it goes to and all it says),
`goal` (`/goal` with its judge on the model
`[ai.goal_judge]` names), `plan` (plan mode's carry-out card going back
to the mode worked in, past the ones `Shift+Tab` passes through),
`acp-fork` (`/fork <focus>` on Claude Code: the copy branches off the agent's session with `session/fork`, told nothing and left whole, with no notice, by a focus the stand-in has no `/compact` for) and
`acp-plan` (plan mode on Claude Code: the instructions as a note, never in
its system prompt, and a clean-context carry-out in a new session),
`acp-agent-file` (Claude Code started by an `AGENT.md` `command` and served
termide's tools as on a connection), `settings-fields` (the settings
window's Option as Alt and git graph fields on General, the terminal session
limit and the bell on the AI page), `settings-live` (settings applying at
once: LSP turned on and off for an open Rust file, emoji icons in the
panel headers, the log going on in a newly named file; needs
rust-analyzer) and
`usage` (the usage window from `F3` and the AI menu: its scopes, a period,
a session opened from its list), `ai-session-locations` (AI → Sessions
listing the directories sessions worked in with their counts, ✗ on one
gone, `Delete` taking a directory's sessions, `Enter` opening an agent
panel there that lists them), `project-store` (layouts in the old tree folded into flat project directories at startup, ✗ on a project whose directory is gone, `F8` deleting a project with or without its agent sessions), `project-move` (`F6` and `F5` on a
project in the Projects menu: the directory moved and reopened with its
layout, a copy opened beside it), `project-button-menu` (a project's button in the
menu bar opening its menu: another project's offering a switch, the
current one's not), `project-relink` (`F6` and `Enter` on a ✗ project
pointing it at where its directory went: the layout and agent sessions
follow, a directory with a project of its own is refused) and `project-move-agent` (a moved
project's agent session found again at the new path, and its git worktree
still linked) and `viewer` (the HTML and Markdown previews from `F3`: a
search with its match count, a same-page link followed, `G` to the last
line, the toggle to the source) and `help` (`F1`'s window on the focused
panel's section, filtered by typing; closing the last panel leaves a file
manager at the project root) and `i18n-status-bars` (in Russian, the
editor's status bar, the find bar and the Markdown preview's status bar
read in the interface language), `agent-checklist` (the `checklist`
tool's headline, the list under its rule in the state strip, and the
reminder of the unfinished list in the next request), `agent-debug-approval`
(an agent's `debug start` of a project command waiting for the user's
approval, the command shown as their own launch shows it), `git-commit`
(a commit from the commit window running behind a slow hook and reporting
the new commit's hash), `headless-acp` (`--prompt` on a `claude_code`
connection, reported as stream-json), `headless-plan` (`--prompt` in plan
mode refusing an edit an allow rule would pass elsewhere), `loop-until`
(`/loop` with a stop condition ending once the judge finds it met) and
`editor-lsp-trust` (a Rust file of a workspace not trusted asking before
rust-analyzer starts, and declined, starting nothing; needs
rust-analyzer).

## Writing one

- **Wait, do not sleep.** `wait` for what the step should bring and
  `wait-gone` for what it should take away; a `sleep` is only for checking
  that something does *not* happen.
- **Wait for text that fits on one row.** The screen is matched row by row
  as drawn, so a sentence wrapped by a narrow panel is not found; wait for
  a shorter part of it, or give the panel room (`key Alt+Home Alt+F11` moves
  the agent into the wide first column at full height).
- **Know when work is over.** The agent's input rule shows `[‖][■]` while a
  run is under way: `wait-gone "[■]"` before `quit`, or quitting asks to
  confirm.
- **Keys are legacy xterm sequences.** The harness leaves termide's kitty
  keyboard query unanswered. Modifiers on a special key (`Ctrl+Shift+Up`,
  `Alt+F11`) go as xterm's modifier codes, but a printable key with `Ctrl`
  needs a legacy control code: `Ctrl+Shift+A` arrives as `Ctrl+A`, and
  `key` refuses a chord with none, such as `Ctrl+.`.

## Limits

- pyte draws no colours into the text snapshots, has no alternate screen
  or images, and counts widths as `wcwidth` does. termide is told with
  `TERMIDE_VS16_WIDE=0` that U+FE0F does not widen an emoji, and the
  harness drops U+FE0F before pyte (which otherwise loses the character
  after it); a width bug that depends on the host terminal is reproduced
  better against the real terminal.
- The clock, the resource figures in the menu bar and the file times of new
  files are live; do not match them.
- Unix only (pty).
