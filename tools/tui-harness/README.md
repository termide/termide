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
- **`--no-lsp`**, so no language server starts or downloads anything.
- **Mock agents** on request: `mock-llm` starts
  `tools/screenshots/mock-llm/server.py` on a free port and points the
  `local` connection at it; it plays a fixed task (read
  `inventory/report.py`, edit it, run the tests, answer), or, asked to
  "delegate the stock scan", hands a task to a subagent in the background
  and quotes its report. It logs the model every call asks for. `mock-acp codex`
  (or `claude_code`, `gemini_cli`) makes that ACP agent the connection and
  puts the stand-in adapters of `tools/screenshots/mock-acp` first on
  `PATH`. Neither touches the network.

## Scenarios

One step per line, words split as a shell does; `#` starts a comment.
Timeouts are in seconds and default to 10.

| Step | What it does |
|---|---|
| `size 120x36` | Screen size (before `start`; the default) |
| `project demo` \| `empty` | The project termide starts in (before `start`) |
| `config FILE` | Use FILE (relative to the scenario) instead of `config.toml`; `{mock_llm_port}` in it is filled in |
| `mock-llm [flavor]` | Start the mock model: `default`, `llamacpp`, `omlx` or `openrouter` |
| `mock-acp PROVIDER` | Connect the `claude_code`, `codex` or `gemini_cli` stand-in |
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
| `mock-acp-got "TEXT" [T]` | Wait until the ACP stand-in's log holds TEXT: the Claude Code one logs each `session/new` (with `plan-in-system-prompt` when its system prompt holds plan mode's instructions) and each prompt block as `prompt: …` |
| `mock-acp-count "TEXT" N` | Check that the ACP stand-in's log holds TEXT exactly N times |
| `run "COMMAND"` | Run a shell command in the project, outside termide |
| `snapshot NAME` | Save the screen as text |
| `sleep S` | Let termide run for S seconds |
| `quit [--force]` | `Alt+Q` and wait for termide to exit; `--force` confirms a "Quit anyway?" |

A scenario that does not `quit` is killed at its end. The examples:
`smoke` (start and quit), `editor` (file manager → editor → save),
`terminal` (a command in a terminal panel), `terminal-keys` (Alt, Esc and Ctrl+F reaching the shell and less, the title following them), `git-diff-staging` (a hunk and lines staged from the diff panel, a discard confirmed and undone), `agent` (the mock model's task
with a permission card), `acp-codex` (the same task through Codex over
ACP), `delegate` (a background subagent's row and report, on the model
`[ai.subagents]` names), `goal` (`/goal` with its judge on the model
`[ai.goal_judge]` names), `plan` (plan mode's carry-out card going back
to the mode worked in, past the ones `Shift+Tab` passes through) and
`acp-plan` (plan mode on Claude Code: the instructions as a note, never in
its system prompt, and a clean-context carry-out in a new session).

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
  keyboard query unanswered, so chords that need the kitty protocol
  (`Ctrl+Shift+…`, `Ctrl+.`) cannot be sent; `key` refuses `Ctrl` with a
  key that has no legacy encoding.

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
