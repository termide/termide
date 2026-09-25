# Coding Agent

TermIDE has a built-in coding agent: a panel where you describe a task in
plain language and a language model carries it out by reading files, editing
them and running shell commands in your project. Every action it takes that
could change something asks for your permission first.

Open it with `Alt+A`, from **Windows → Agent**, or from the command palette
(**Open Agent**).

## Configuring a model

The agent talks to any OpenAI-compatible endpoint, or to Anthropic's Messages
API. The OpenAI protocol covers local servers (llama.cpp, Ollama, vLLM, omlx)
and most hosted gateways, OpenAI and OpenRouter among them. Without a
configured model the panel refuses to open and says so.

```toml
[ai]
provider = "openai_compatible"   # openai_compatible (default), anthropic_compatible, claude_code, codex
base_url = "http://127.0.0.1:10000/v1"
model = "Qwen3.8-Flash-Next-oQ4e-mtp"
# context_window_fallback = 32000   # used only when the server does not report a window
max_tokens_per_turn = 4096          # 0 or negative: no limit, the model decides
prefer_reasoning = false            # send reasoning_effort to models that support it
api_key_env = "OPENAI_API_KEY"   # name of the variable, never the key itself
autofold = true              # fold each block to a preview by default
```

Every one of these lives under an **AI** section in the settings modal too
(the gear, or the command palette), so you can change the provider, model,
context window and the rest without editing the file. The model is a dropdown:
it fills with the endpoint's models (fetched in the background when the modal
opens), and its last entry, "Enter a model id…", lets you type one by hand when
the endpoint cannot list them. The API key is read from
the environment variable named by `api_key_env`, so the configuration file
never holds a secret. Local servers usually need no key at all; leave the
variable unset. `autofold = false` shows every block expanded instead of
folded to a preview. With `max_tokens_per_turn` at zero or below no output
limit is sent and the model decides how long to reply; the Anthropic API
requires one, so there it becomes a generous 32000.

For a hosted OpenAI-compatible endpoint, keep `provider = "openai_compatible"` and point
`base_url` and `api_key_env` at it, for example OpenAI itself
(`https://api.openai.com/v1`, `OPENAI_API_KEY`) or OpenRouter
(`https://openrouter.ai/api/v1`, `OPENROUTER_API_KEY`). For an Anthropic
subscription set `provider = "anthropic_compatible"`, drop `base_url` (the API root is
built in; set it only for a gateway) and point `api_key_env` at your
`ANTHROPIC_API_KEY`; `prefer_reasoning = true` then turns on extended thinking. The
**Model** chip lists the endpoint's models for each.

`provider = "claude_code"` and `provider = "codex"` are different in kind: instead
of the built-in loop talking to a model endpoint, the panel drives that tool's
own CLI as an [external agent](#external-agents) over ACP
(`@zed-industries/claude-code-acp` / `@zed-industries/codex-acp`, run through
`npx`). The CLI owns the endpoint and the sign-in — its own subscription or
API key — so `base_url` and `api_key_env` in `[ai]` do not apply; the settings
modal hides them for these providers and clears them from the file. `model` is
kept: it is the model **pre-selected** on the agent — applied over ACP once the
session starts — and at runtime the **Model** chip lists and switches the
agent's own models. The
tool must be installed and signed in first (`npx` on `PATH`). This is the
built-in shortcut for what an agent's own `[acp]` table does by hand.

## Using the panel

The session fills the panel, the input box sits at the bottom, under a titled
border that carries the agent's name (`─ default ─`) so parallel agent panels
are easy to tell apart. A long line wraps to the panel width and the box grows
to fit — up to half the panel's height — before it starts scrolling. Like a new
terminal, the agent works in the directory of the panel that had focus when
you opened it (a file manager's directory, an editor's file), or in the project
root. The panel title is your first request, so several agent panels stay
apart at a glance; before you ask anything it shows the working directory
instead. Give a session a name of your own through the panel's `[≡]` menu →
**Rename session**, and the title shows that name from then on.

A fresh session greets you with a banner: a small logo on the left and, on the
right, what the agent is set up with — its provider, model, agent name and the
directory it works in. The model and the agent are shown underlined in the
accent colour: a click on either opens the same picker its status-bar chip
does, so you can switch before you start. The banner gives way to the
conversation as soon as you send your first message.

The panel's `[≡]` menu is kept to the actions with no home elsewhere —
**Session info** (also `F3` and `/usage`), **Rename session** and **Delete
session**. The assembled system prompt opens with `/prompt`. Managing sessions
is on the F-keys and in the AI menu instead: `F7` starts a new session, `F6` opens
the picker of this directory's sessions (newest first, the current one marked
`●`), `F8` deletes the current one after a confirmation, and `F2` renames it.
Switching waits for the current task: stop it with `Esc` first if the agent is
still working.

From the input, `/new` starts a fresh session (keeping the current one), and
`/clear` starts one too but deletes the current one first; `/rename` (or
`/name`) renames it. The model, agent and permission-mode pickers are the
status-bar chips.

A session you never send anything to is discarded when you switch away from it
or close the panel, so opening a panel and closing it — or trying a couple of
new sessions — leaves no empty logs cluttering the list or the disk. A session
you have named or sent even one message to is always kept.

| Key | Action |
|---|---|
| `Enter` | Send. While the agent works, the text is queued for the next turn instead and waits in the state strip above the input |
| `Shift+Enter`, `Alt+Enter`, `Ctrl+J` | New line in the input |
| `Esc` | Stop the running task; with nothing running, clear the input |
| `Ctrl+O` | Expand or collapse every block |
| `Tab` | Move focus between the input and the chat; in the chat, `↑`/`↓` pick a block, `Space`/`Enter` fold or unfold it, `o` opens it in its own panel |
| Click a block | Focus the chat and select that block (the selected block is shown inverted, success and error colours keeping their hue); click it again to fold or unfold it |
| `Ctrl+C` | Copy: the selected prompt text, or — with a block selected in the chat — the block's text |
| `Ctrl+X` / `Ctrl+V` | Cut / paste the prompt selection |
| `Ctrl+A` | Select all the prompt text |
| `Shift+arrows`, `Shift+Home`/`End`, `Ctrl+Shift+arrows` | Extend the prompt selection by character, to the line edges, by word |
| `Ctrl+Left` / `Ctrl+Right` | Word-by-word navigation in the prompt |
| `Ctrl+Z` / `Ctrl+Y`, `Ctrl+Shift+Z` | Undo / redo a prompt edit |
| `Shift+Tab` | Cycle the permission mode: ask → accept-edits → auto → plan |
| `F2` | Rename this session (the same prompt as the `[≡]` menu) |
| `F3` | Open the session-info modal (model, agent, mode, directory, created/last-active times, messages, compactions, tokens, context, how much shell output was cleaned); also `/usage` and the `[≡]` menu |
| `F4` | Roll the session back to before a chosen checkpoint |
| `F6` | Switch session — open the picker of this directory's sessions |
| `F7` | Start a new session (the used one is kept in the list) |
| `F8` | Delete this session (after a confirmation) and start a fresh one |
| `/name args` + `Enter` | Send the prompt template `name` with `args` filled in, or run the command script `name`; `/compact [focus]` summarises the session, `/undo` takes the last request back, `/new` starts a fresh session, `/clear` starts one after discarding the current session, and `/rename [name]` (or `/name`) renames it; `/pause` stops the run after the current step and `/continue` resumes it (or, before the step ends, cancels the pause); `/loop [interval] <prompt>` re-runs a prompt on an interval (or back-to-back), `/loop stop` (or `Esc`) ends it; `/goal <what to achieve>` works autonomously toward a goal until a judge says it is reached, `/goal stop` (or `Esc`) ends it; `/handoff` briefs the unfinished work, then offers to save it to `HANDOFF.md` or start a new session from it; `/usage` opens the session-info modal and `/prompt` opens the assembled system prompt |
| `↑` / `↓` | On the first or last line of the input: take back the messages still queued (`↑`, while any wait), else recall an earlier request of this session, or come back to what you were typing |
| `Tab` | Complete the highlighted `/command` or `@file` while the list is open |
| `Ctrl+↑` / `Ctrl+↓`, `PageUp` / `PageDown` | Scroll the session |
| `Ctrl+Home` / `Ctrl+End` | Jump to the start, or back to following the newest output |

The conversation is a stack of blocks, each opened by an accent-coloured mark:
`› ` for your message and for the agent's answer, `@ ` for its reasoning, `$ `
for a shell call, `< ` for a file read, `> ` for a write, `± ` for an edit,
`# ` for the system prompt. The answer is shown in full;
a user message or the system prompt longer than five lines is
folded to a preview that keeps the first line and the last few, with a
`… N more lines` note between them. A block of five lines or fewer has nothing
worth hiding, so it is shown in full with no fold marker; so is any finished
block that takes a single row unfolded. A finished tool call
folds to a single line: its headline (a shell call's first command line) with
how long it took (`🕒`) at the right end. A single row carries no `✓`; a
failed call shows its headline in the error colour instead, and its unfolded
form ends with the `✗`. Durations grow from seconds to minutes, hours and
days (`2s`, `1m13s`, `2h5m`, `3d4h`). Finished reasoning folds the same
way, to its first line with how long the turn took (`🕒`, prefill and generation
together) at the right end; unfolded, that splits into the `⏫`/`✍️` lines. A block still in progress — a
streaming reasoning, a running tool call with its live output — is always shown
unfolded and folds only once it finishes. Your
message reads as plain text on a faint background; the reasoning, the system
prompt and a tool's output are dim text, except an edit's diff, which is
coloured the way the Git diff panel colours one. Every step opens with its type
glyph and a localized action in the same accent, then its subject: the
reasoning as `@ Thinking` and its text, a shell call as `$ Running` and its
command (dim, wrapped to the width), a file tool with its path
(`< Reading src/main.rs`, `> Writing`, `± Editing`), a web tool with the URL or
the query (`↓ Fetching https://docs.rs`, `? Searching ratatui scrollbar`); any
other tool as its name and a summary. The reasoning is its own block above the
answer, and its text wraps to the width. Only the system prompt
opens with a dim dashed rule that sets it apart from the block before; an
annotation has none, its glyph marks it. A blank line follows your message and another precedes the answer; the
reasoning and tool calls in between stack with no gap. A folded block is marked
with `▸`, an unfolded one with `▾`, right after the step's action
(`@ Thinking ▸`, `$ Running ▸`, `< Reading ▸ src/main.rs`) or the system
prompt's `#` (`# ▸`).

While a run works, a run clock sits right-aligned under its last block: an
animated `✽` and the time since the request was sent. It has its own glyph
rather than `🕒` because the figures under each block describe that block
alone, while this one is the run's total. When the run ends the clock freezes
where it stood and dims, so a resting `✻` never reads as a working one. A run that ends on an answer keeps it in the answer's meta
(`✻ 3m41s`), under the answer's own time and `✓`/`✗` — a failed call's error
included. A run whose end the answer cannot tell (one ended on a tool call, or
aborted after an answer that went fine) keeps it as a closing line after its
last block, with the time it ended and how: `✻ 3m41s · 21:03:41 ✗`.

`/pause` takes effect as soon as the tool call or model reply in progress
finishes — not at the end of the whole step, so a step that fetches several
pages stops after the current one. The calls it leaves unrun wait for
`/continue`, which runs them first; a new request instead closes them as not
run. A run stopped with `/pause` gets a pause line instead, `‖ 1m12s`: it counts
how long the pause has lasted, its `‖` marked while the pause is on, and
keeps that length, dimmed, once the run continues. A continued run's clock goes on from the
original request, so its total includes the pause. The run controls sit at the
right end of the prompt box's top border, in view whatever the transcript's
scroll: `[‖]` pauses the run, like `/pause`, and `[■]` stops it, like `Esc`,
while the agent works; once a pause is asked for, `[▶]` takes the place of
`[‖]` and withdraws it (so does clicking its notice in the state strip, or
`/continue`); while paused, `[▶]` continues the run, as does clicking the pause line or
`/continue`, and `[■]` gives the paused run up (the calls it left unrun are
closed by the next request).

The closing line is one kind of annotation — a line that marks a moment in the
conversation rather than holding content. The others are the panel's notices:
`·` for information (a model switch, a finished compaction), `!` for a warning
(a stopped goal, a busy agent), `✗` for an error outside a block (a failed
compaction, an MCP error). An error inside a turn stays in its answer block.
Annotations never fold. A notice's text wraps to the width under its glyph; the chat cursor can stop on a notice, so an
error can be selected and copied like a block, while it passes over a run's
closing line.

What holds right now rather than what happened lives in the state strip, a few
rows between the conversation and the input that appear only when there is
something to show: a pending pause (`‖ will pause after the current step`) —
once the pause takes effect the transcript's `‖` line takes over — and each
message queued while the agent
works (`› …`, its first line; after three, a count of the rest). Everything queued
goes to the agent at once, as a single message with the pieces joined by a
blank line — messages typed while the agent works are usually one thought
added to — and leaves the strip then, showing up in the conversation as your
message. Until the agent takes it, `↑` on the input's first line takes the
queue back into the input, ahead of anything typed, to edit before it goes.

The system prompt in effect is shown as a folded `# ` block at the start of a
session and again whenever it changes before your next message (switching agent
or mode, for instance), so what the model was told is always in view.

Your message and the agent's answer each end with a dim, right-aligned time and a
`✓`/`✗` status (`18:34:01 ✓`), at the end of the text's last row when it fits
there; the reasoning and tool blocks carry only their
work figures, no wall-clock. A tool call shows how long it took (`🕒 6s`), and its
status once unfolded. A call that waited on a permission question shows that
wait apart, first, as the pause it was: `‖ 12s 🕒 2s`, the `‖` marked and
ticking while the question is up; the `🕒` counts only the call's own run. When a turn reasons, the reasoning block carries the turn's cost —
the prefill phase (`⏫ 6s (↑42k, 7k tok/s)`) and the generation phase
(`✍️ 12s (↓5k, 420 tok/s)`), each with its duration (whole seconds), token count
and average speed; large counts are abbreviated (`40k`, `1.2M`). A turn with no
reasoning shows those on the answer instead.
While a turn is still running, the same right-aligned meta zone shows the live
figures: a `✍️` generation line with the running duration, estimated tokens and
speed, and below it the run clock. The `⏫` prefill line waits for the finished block, since the
input token count is only known once the turn ends. Reopening a
conversation restores each block's time, its reasoning, the turn's
prefill/generation lines and each tool call's duration from the log.

Unfold a block to see all of it: click it, or press `Tab` to move into the
chat and `Space`/`Enter` on the block the `↑`/`↓` cursor is on; `Ctrl+O`
unfolds everything at once, and `o` opens the selected block in its own
read-only panel for a bigger view (a command with a saved full log opens that
file). `Tab`, or a click back on the input, returns focus to the input. The
panel follows the newest output until you scroll up, and resumes following when
you scroll back to the bottom.

To copy a whole block, select it (click it or move to it) and press `Ctrl+C`.
In the prompt, `Ctrl+C` copies whatever is selected there instead — selected
text is shown inverted, so it is clear what a copy will take.

Mouse selection works inside the prompt box: press to place the cursor, drag to
select (across wrapped rows), release, then copy or cut. In the transcript a
drag selects text the way a terminal does, from the cell it started on to the
one under the pointer, scrolling when dragged past an edge; `Ctrl+C` copies it,
each row trimmed of its trailing padding. A click without a drag (acting on
release) selects the block under it instead and clears the text selection.

What the agent runs is captured cleanly for the model: colour and cursor
escapes, progress-bar redraws, spinner frames and long runs of near-identical
build lines are stripped or collapsed before the output enters the context,
so a noisy command costs far fewer tokens. A test run — Rust (`cargo test`,
`cargo nextest`), Python (`pytest`), Go (`go test`) or JS/TS (`jest`,
`vitest`) — drops the passing and skipped test lines and keeps the failures and
the result summary. The full, untouched log is still
written to a file whose path the tool reports, and you still see the raw
stream live in the panel.

The status chips run, left to right: the agent, the permission mode, a
**Reasoning** toggle (`on`/`off`), the provider (`OpenAI Compatible`) and the
model. The session's token totals (`↑` input / `↓` output) and the context
window — tokens used of the window with a fill bar, `35k/262k ▰▰▱▱▱▱▱▱` — sit
flush right; on a narrow terminal the chips on the left are cut, never these.
Agent, mode, reasoning and model are buttons, and the same entries sit in the
`[≡]` menu. Clicking **Reasoning** asks the model to reason (extended thinking
/ `reasoning_effort`) from the next request; the choice is remembered in the
session, so a resume comes back with it. What the agent is doing right now is
not repeated in the status bar: each chat block carries it in its byline.

Typing `/` opens a list of the matching prompt templates above the input;
`↑`/`↓` move in it, `Tab` or `Enter` complete the highlighted one, and `Enter`
on a name typed in full sends it.

Typing `@` opens the same list with the files and directories under the
panel's directory instead, so a path is a few keystrokes: `@ma` finds
`src/main.rs`. `Tab` or `Enter` inserts the highlighted one; a directory ends
in `/` and reopens the list for its contents, so you can drill in. The agent
reads the file you name; `@` is only quick path entry, nothing is attached
behind your back.

A large paste (more than five lines or 2000 characters) is held out of the
prompt box as a short `[#1 pasted 40 lines]` placeholder instead of flooding
it; the full text is spliced back in place of the placeholder when you send,
so the model still gets all of it. A smaller paste goes in as it is.

**Model** asks the endpoint for the models it serves and lists them, the
current one marked `●`; the last entry lets you type an id instead, which is
also what you get when the endpoint cannot list its models. The switch takes
effect on your next request and stays with the session: it is written to the
session log — with the provider it runs on — so reopening that session brings
its model and provider back, and a new session starts on whatever model the
panel is on. Switching waits for the current task, like switching sessions.

**Context window.** When the endpoint reports a model's window (vLLM and omlx
report `max_model_len`), the panel always adopts it — at startup and on every
model switch — so `Context:` matches what the server actually allows. The
configured `context_window_fallback` is only a **fallback**, used when the endpoint
reports no window; left unset it shows `(auto)` in the settings modal and the
built-in default stands in until (or unless) the provider is known.

**Mode** offers the three permission modes described below. `Shift+Tab` cycles
through them without the picker. A change applies at the agent's next tool
call, so you can loosen the mode while a long task is running instead of
answering the same prompt again and again. Neither switch touches the
configuration file; the panel starts from `[ai]` again the next time.

### Undoing a request

`/undo`, or **Undo last request** in the `[≡]` menu, takes back the last
request that changed files: a card in the panel names the files, and on
confirmation each is put back as it was before that request (a file the
request created is removed) and the conversation is rewound to just before
it, so the agent no longer remembers doing it either. Open editors reload the
restored files. Repeat it to step back through earlier requests; a request
that changed nothing is skipped. It is a step back, not a redo: the undone
messages stay in the session log on a dead branch, and the files' newer
content is gone.

Before `edit` or `write` runs, the panel keeps a copy of the target under the
session's directory (`ai/sessions/<path>/checkpoints/<session id>/`), one
folder per request, deleted again when the request is undone. Shell commands
are not covered: what `bash` changes, git or your own backups have to hold.

## Tools

The agent has six built-in tools, plus those its MCP servers provide (see
[MCP servers](#mcp-servers)).

- **read** returns a file with line numbers, paged with an offset when a file
  is long.
- **edit** replaces a unique piece of text in a file and reports a diff of
  what changed.
- **write** creates a file or replaces its whole content.
- **bash** runs a shell command in the project directory, streaming its output.
  Long output keeps its beginning and end, and the complete log is written to
  a file the agent can read.
- **web_search** asks a search engine and returns titles, links and snippets.
- **fetch** loads a web page and returns its text as markdown, paged like
  `read` when it is long.

Searching the project is done through `bash` with the tools you already have
(`rg`, `find`), rather than through a separate search tool.

A seventh tool, **skill**, appears when skills are defined; see
[Skills](#skills).

### Web

`web_search` and `fetch` work through a Chrome-family browser on your machine
(Chrome, Chromium, Edge or Brave), because search engines refuse scripted
requests and many pages are built by JavaScript. The browser runs with a
profile of its own in `ai/web/browser/` under the configuration directory, never
your everyday profile, so the agent has none of your logins. It runs without a
window, introducing itself as the same browser does with one; one browser
serves every agent panel, and it quits after five idle minutes.

When a search engine asks to confirm that a human is searching, a window with
that page opens in front of you and the agent's tool call shows the wait:
solve the check and the search carries on. The agent's profile remembers the
clearance, so it is not asked for on every search, and the browser goes back
to running without a window once it has been idle.

Without such a browser, `fetch` still reads pages over plain HTTP (pages built
by JavaScript then come back mostly empty), and `web_search` is not offered.

```toml
[ai.web]
backend = "auto"         # auto: the browser when found, else plain HTTP; chrome; http
engine = "duckduckgo"    # duckduckgo, bing, google, yandex, or your own
chrome_path = ""         # the browser executable; empty looks in the usual places
display = "headless"     # headless (no window); minimized; visible
```

`minimized` gives the browser a real window kept minimized, for an engine that
still tells a windowless browser apart. `visible` leaves the window on screen
to watch the agent work: every page opens in the same tab, which stays on the
last page the agent read, so you can look at it or open the developer tools
there. Closing that tab is fine; the next page opens in a new one. To watch
for a while without changing the setting, use **Show browser window** in the
**AI** menu: the browser is relaunched in a visible window at once and stays
there, idle or not, until **Hide browser window** — for every agent panel,
since they share one browser. The profile and its cookies carry over.
On Linux without a display server the browser runs without a window whatever
the setting says.

Each search engine is a small file in `ai/web/engines/`: the address to send
the query to and the CSS selectors that pick the results out of the page.
termide ships `duckduckgo`, `bing`, `google` and `yandex` and keeps them
current like its other shipped files. When an engine changes its page and the
search starts returning nothing, the selectors in its file need updating; a
file of your own with a new name adds an engine. The keys are described in
the shipped files.

Both tools ask for permission by default. "Allow always" for `fetch` allows
the whole site (`https://docs.rs/*`), for `web_search` every query. Neither
changes anything on your machine, so both work in plan mode.

A file the agent edits while it is open in an editor is reloaded there at
once, cursor and scroll position kept, unless that editor has unsaved changes;
then the editor keeps them and marks the conflict, as with any change on disk
(see [Changes on disk](editor.md#changes-on-disk)).

## Permissions

Nothing that changes your project happens without your say-so. When the agent
wants to do something that is not already allowed, a card appears in the
panel above the input. Its title is the intent — "Agent wants to run bash:" —
and under it, dim, exactly what that is (the command or path); a long one
folds to five lines that a click unfolds. Six rows follow: allow once, allow
for this session, allow always, deny, **deny and tell the agent why** (a
sentence you type, returned to the model as the reason, so it can take another
way), and **stop the run**. `↑`/`↓` and `Enter`, or the digits `1`–`6`, answer
it; a click picks a row and a second click (or `Enter`) confirms it, so a
stray click cannot answer; `Esc` stops the run. The status line announces the
question too, so a panel that is not in focus does not ask unseen. "Allow
always" appends a rule to `.termide/config.toml` in the project.

Rules live per tool. Among the rules that match, the strictest wins, so a
`deny` always beats an `allow`:

```toml
[ai.permissions]
mode = "ask"        # ask | accept-edits | auto | plan — what new sessions start in

[ai.permissions.bash]
"cargo *"     = "allow"
"git status*" = "allow"
"git push*"   = "ask"
"rm -rf *"    = "deny"

[ai.permissions.edit]
"src/**" = "allow"
".env"   = "deny"

[ai.permissions.read]
"**/.env*" = "deny"
```

In a pattern, `*` stands for any text and a leading `**/` is optional, so
`**/.env*` also matches `.env` in the project root. Shell commands are matched
per part: `cargo build && rm -rf target` needs both halves allowed, and a deny
on either half stops the whole command. Command substitution (`$(…)`, backticks)
is never allowed automatically.

The mode decides what happens to anything no rule covers. `mode` in the
configuration is the starting point every new session takes (`ask` unless you
change it), also set from the settings modal's **AI** section under
Permissions; the panel's **Mode** chip and `Shift+Tab` change it for the
current panel only.

- **ask** (the default) asks before every change and every command.
- **accept-edits** also lets the agent edit and create files inside the project
  without asking; shell commands still ask.
- **auto** allows everything. Use it only where a mistake costs nothing, such
  as a container or a scratch checkout.
- **plan** allows nothing that changes anything: the agent reads, searches
  and runs look-only commands, then answers with a plan. See below.

Two things never ask in any mode: reading a file inside the project, and a
short list of commands that only look at things (`ls`, `cat`, `rg`,
`git status`, `find` without `-delete` or `-exec`, and similar). A redirection
in the command disqualifies it.

### Plan mode

For a task you want to see thought through before a line changes, switch to
**plan** (the chip, `Shift+Tab`, or an agent whose `agent.toml` says
`mode = "plan"`). While it is on, the instructions from `system/plan.md` are
added to the system prompt, and every tool call that could change something
is refused with a message the model reads, whatever the rules, the session
grants or a hook's approval say: `edit`, `write`, MCP tools, and any shell
command that is not on the look-only list. Reading and `skill` stay as in
`ask`.

When the agent answers, a card asks what to do with the plan:

```
┌ Plan mode: carry the plan out? ────┐
│ 1. Yes, accepting edits            │
│ 2. Yes, asking before each change  │
│ 3. Keep planning                   │
└────────────────────────────────────┘
```

The first two leave plan mode for accept-edits or ask and send the request
named in the front matter of `system/plan.md` (`request:`), so the same
session goes on to carry the plan out with it in context; the third (or
`Esc`) keeps plan mode, and whatever you type next refines the plan. The
plan is the agent's answer in the session, nothing is written to a file; the
`/undo` checkpoints cover the changes that follow.

## The AI menu

The **AI** menu in the top menu bar (after **Projects**) manages the agent's
resources without opening a panel. It has four sections — **Agents**,
**Sessions**, **Skills**, **Prompts** — each opening a list you browse with
`↑`/`↓`; the arrows, `Enter` and the mouse work as in the **Commands** menu.
Items merged from the project (bold) and the global configuration are shown
together (see [The agent directory](#the-agent-directory)).

- `Enter` (or `F4`) edits: it opens a skill's `SKILL.md` or a prompt's `.md`
  in an editor; an agent opens a further submenu to edit the prompt (`SOUL.md`)
  or the settings (`agent.toml`); a session resumes — focusing the panel that
  already shows it, or opening a new agent panel when none does.
- `Delete` removes the item (with a confirmation); `F2` renames it (for a
  session this sets its display name). The session confirmation names the
  session (its display name, first prompt, or "untitled") and its id.
- The first two rows of Agents, Skills and Prompts create a new item — **New
  (project)** under `.termide/ai/`, or **New (global)** under the configuration
  directory — asking for a name and opening the new file. Sessions are created
  by running an agent, so they have no create rows.
- Each session row shows, dim on the right, when it was last worked on
  (e.g. "2h ago").

Below the sections, once an agent panel has been opened, **Show browser
window** puts the agents' web browser on screen to watch it work (see
[Web](#web)), and **Hide browser window** takes it away again.

## The agent directory

The agent's own files live in an `ai` directory that exists at three
levels, highest priority first:

1. `.termide/ai/` in the directory the panel works in;
2. `.termide/ai/` in the TermIDE project root, when that is another
   directory;
3. `ai/` in the TermIDE configuration directory
   (`~/.config/termide/ai/` on Linux,
   `~/Library/Application Support/termide/ai/` on macOS).

A single file is taken from the first level that has it. A directory of named
entries (agents, skills and prompt templates) is the union of all levels, and a
name defined higher hides the same name below.

```
ai/
  AGENTS.md                the system prompt template of the default agent
  agents/<name>/SOUL.md    the template of a custom agent (optional)
  agents/<name>/agent.toml what else sets the agent apart (optional)
  skills/<name>/SKILL.md   skills, see below
  prompts/<name>.md        prompt templates, typed as /name
  commands/<name>          command scripts, typed as /name, see below
  shims/<command>          command shims (config level only), see below
  mcp.toml                 MCP servers, see below
  hooks.toml               command hooks, see below
  system/compact.md        how the agent summarises a long session
  system/compacted.md      how the summary is worded in the context
  system/plan.md           what plan mode tells the agent, and what accepting a plan sends
  system/goal.md           how the judge decides whether a /goal is reached
  system/handoff.md        how /handoff briefs the unfinished work
  web/engines/<name>.toml  search engines for web_search, see Web
  web/browser/             the web tools' browser profile (config level only)
```

The first time the panel opens, the configuration level is laid out:
`AGENTS.md`, the `system/` files and the search engines receive the shipped
texts, `agents/`,
`skills/`, `prompts/`, `commands/` and `shims/` are created empty. termide
records what it shipped (in `.seeds.toml`) and keeps these files current on
later starts: one you never edited is refreshed when the shipped version
changes, so upgrades reach you; one you edited is left untouched, with the new
default written beside it as `<file>.new` to compare and merge at your leisure.
Delete a file to get the shipped version back.

### Agents

An agent is a directory under `agents/`. `default` is the one the panel
starts as; it has no directory and speaks with `ai/AGENTS.md`. Any directory
defines an agent you can switch to from the **Agent** status chip or **Change
agent…** in the `[≡]` menu; the picker shows each agent's description. Its
`SOUL.md` is the agent's own template; without one it uses `ai/AGENTS.md`
too. Beside it an `agent.toml` may set, every field optional:

```toml
description = "Reviews diffs and points at risks"
model = "Qwen3.8-27B-MTPLX-Optimized-Quality"   # at the configured endpoint
mode = "accept-edits"                            # ask | accept-edits | auto | plan
tools = ["read", "bash"]                         # a subset of the built-in tools
```

Switching agents mid-session swaps the prompt and the tools for the next
request; the model and the mode change only when the definition names them,
and the session log records the switch, as it does for the **Model** chip.
A reopened session comes back as the agent it last ran as, and a saved layout
remembers it too.

### Subagents

When there is more than one agent, each built-in-loop agent gets a `task`
tool that hands a self-contained job to another agent. The delegate runs its
own loop to the end — its own prompt, its own tools, its own model — and its
final answer comes back as the tool's result; the steps and the files it read
along the way stay out of the main conversation. It is how a terse reviewer
or a focused searcher does its work without filling the session, the way
Claude Code's `Task` tool and OpenCode's sub-sessions do.

The delegate does not see the conversation, so the calling agent must put
everything into the prompt. It runs with no one to prompt, so it can only do
what the permission rules and the current mode already allow: anything that
would otherwise ask is refused with a reason it reads. External (`[acp]`)
agents cannot be delegates, and a subagent gets no `task` tool of its own, so
delegation does not nest. A run that will not stop is cut off after fifty
model calls.

### External agents

An agent may be another program altogether: put an `[acp]` table in its
`agent.toml` and the panel drives it over the
[Agent Client Protocol](https://agentclientprotocol.com) instead of running
the built-in loop. Claude Code, Codex and Gemini CLI have ACP adapters or
speak it natively:

```toml
description = "Claude Code through its ACP adapter"

[acp]
command = "npx"
args = ["-y", "@zed-industries/claude-code-acp"]
env = { ANTHROPIC_API_KEY = "$ANTHROPIC_API_KEY" }
```

The program starts in the background when you switch to the agent; the first
request waits for it. Its answers, thoughts and tool calls appear in the
session like the built-in agent's, and it reads and writes files through
TermIDE, so an open editor follows its edits. Its permission requests are
judged by the same rules as the built-in agent's: a read-only command or a
request a `[ai.permissions]` rule or a session grant already covers passes
without a card, and only what is left reaches you — so a granted or read-only
command is never asked twice. The **Mode** chip disappears while an external
agent is active: it has its own, and TermIDE's mode is not cycled for it. The **Model** chip stays when the agent advertises
its models over ACP — it then lists them and switches with `session/set_model`,
so you pick the agent's model in TermIDE; agents that advertise none show no
chip. Skills, prompt templates and MCP servers are the agent's own affair too;
`model`, `mode` and `tools` in `agent.toml` do not apply. Switching agents
rebuilds the conversation on the same session log: earlier messages stay on
screen but the external agent does not know them, and the panel says so.

### The system prompt

The prompt the model receives is assembled from files: the template
`ai/AGENTS.md` with placeholders the agent fills in. No prompt text is
built into TermIDE; the template below ships as a data file
(`crates/agent-core/assets/AGENTS.md`) and is written to the
configuration level on first use, and from then on the file is what counts. A
project or the panel's directory may carry its own `.termide/ai/AGENTS.md`,
which then replaces it:

```markdown
You are a coding agent working inside termide, a terminal IDE. You help with software tasks in the current project: you read code, make targeted edits, run commands and report what you did and what you found.

# Tools
{{tools}}

# Guidelines
- Read a file before you change it, and keep edits small and targeted.
- Name file paths clearly when you talk about files.
- Be concise.
{{guidelines}}

# Skills
When a task matches one of these, load it with the `skill` tool before starting.
{{skills}}

# Environment
{{environment}}

{{project_instructions}}
```

`{{tools}}` is the tool list with a line per tool, `{{guidelines}}` the rules
the tools themselves contribute, `{{skills}}` the skills by name and
description, `{{environment}}` the working directory, platform, date and
whether it is a git repository, and `{{project_instructions}}` the
instruction files described below. Reword the file, drop a section or add
your own; a placeholder you leave out is simply not sent. `/prompt` opens the
assembled result in a viewer, so you can see exactly what the model gets.

### Service prompts

TermIDE's own prompts are files too, under `system/`, at any of the three
levels and seeded on first use like `AGENTS.md`. Compaction, the summary that
replaces the older part of a long session, uses two: `compact.md` is the
system prompt of the summarising call, with the closing user turn in its
front matter (`request:`) and `{{focus}}` where the words given to `/compact`
go; `compacted.md` is the message the summary becomes in the context, with
`{{summary}}` for the model's text. Edit them to change what a summary keeps
or how it is introduced.

```
/compact              summarise now
/compact the API      summarise now, concentrating on the API
```

`/compact` is built in and sits in the `/` list beside your templates; it
waits for a running task like every other switch. Automatic compaction, when
the session approaches the context window, uses the same files.

[Plan mode](#plan-mode) uses `plan.md`: its body is appended to the system
prompt while the mode is on, and `request:` in its front matter is the
message sent when you accept the plan. Reword the body to change what a plan
must contain, or the request to change how the agent is told to go ahead.

`/goal <what to achieve>` uses `goal.md`: the agent works toward the goal, and
after each turn a judge — a separate, read-only model call — decides whether it
is reached. `goal.md` is that judge's system prompt, with `{{goal}}` for the
goal text and the verdict question in its front matter (`request:`). The judge
answers `DONE` or `CONTINUE` with a one-line reason; on `CONTINUE` the agent is
sent back to work with what is still missing, until the judge says done, a turn
errors, or the safety cap of fifty turns is hit. `/goal stop`, `Esc`, or
stopping the run ends it. Reword `goal.md` to change how strictly the goal is
judged.

```
/goal get the test suite green    work until the judge agrees it is done
/goal stop                        end the active goal
```

`/handoff` uses `handoff.md`: a read-only model call distils the transcript into
a forward-looking brief — the goal, what is done, what remains, key decisions,
files and how to verify — for a fresh session or another agent to pick up cold,
unlike `/compact`, which summarises the whole conversation to keep going in
place. `handoff.md` is that call's system prompt, with the request in its front
matter. When the brief is ready a card offers to **save it to `HANDOFF.md`** in
the panel's directory (where another agent, even an external one that reads
files, can pick it up — add it to `.gitignore`) or to **start a new session
from it**, seeding the fresh session with the brief. Reword `handoff.md` to
change what a brief contains.

### Skills

A skill is a directory with a `SKILL.md` in the [agentskills.io](https://agentskills.io)
shape: YAML front matter with `name` and `description`, then the
instructions, plus any files the instructions refer to (scripts, checklists,
examples):

```markdown
---
name: release
description: Cut a release: version bump, changelog, tag, packages
---
# Release

1. Run `scripts/check.sh` …
```

Skills are read from `skills/` at the three levels of the `ai` directory and
also from `.agents/skills/` in the panel's directory and in the project root,
the directory other agents share, so a skill written for Claude Code, Codex
or pi works unchanged. The same name at a higher level hides the lower one.

Only the names and descriptions go into the prompt, one line per skill under
`{{skills}}`; the instructions themselves enter the conversation when the
model loads the skill with the `skill` tool, which returns the text of
`SKILL.md` and lists the files beside it for the model to `read`. Loading a
skill never asks for permission. The tool exists only when at least one skill
does, and it is not subject to an agent's `tools` list.

### Prompt templates

A prompt template is a Markdown file `prompts/<name>.md`, at any of the three
levels, that you send as `/name` followed by arguments. The front matter is
optional: `description` for the picker and `argument-hint` for what to type
after the name. In the body `$ARGUMENTS` stands for everything after the
name and `$1`…`$9` for its words; a body without placeholders gets the
arguments appended on a line of their own.

```markdown
---
description: Review a file for bugs and risks
argument-hint: <path>
---
Review $1. Point at bugs first, style last, and quote the lines you mean.
```

`/review src/parser.rs` then sends the expanded text, which appears in the
session as what the model actually received. **Insert prompt…** in the `[≡]`
menu lists the templates and puts the chosen `/name ` into the input. A
message starting with `/` that names no template is not sent; a path such as
`/usr/bin/ls` is plain text.

### Command scripts

Where a template is fixed text, a command script builds the request: an
executable in `commands/<name>`, run as `/name args` with the arguments as
its argv and the panel's directory as its working directory, whose standard
output is sent to the model. A `/review` that gathers `git diff --staged`, a
`/failing` that runs the tests and pastes what broke, an `/issue 123` that
fetches the ticket. Any language will do; the header comments describe it:

```sh
#!/bin/sh
# description: Review the staged changes
# argument-hint: [focus]
# timeout: 30
printf 'Review this diff%s:\n\n' "${1:+ with attention to $1}"
git diff --staged
```

The output appears in the session as your request, so what the model got is
visible. A script that exits with an error, prints nothing or exceeds its
timeout (60 s by default) sends nothing and reports why. Scripts from the
configuration level are your own and run at once; one that came with the
project or the directory asks first, in a card like a permission: run once,
for this session, always (a rule `[ai.permissions.command]` is written)
or not at all. Templates and scripts share the `/` names; when both exist
at the same level, the template wins.

### MCP servers

Tools from [MCP](https://modelcontextprotocol.io) servers join the built-in
ones. A server is a table in `mcp.toml`, at any of the three levels; the same
name higher up replaces the table below, and `enabled = false` there switches
a server off for a project or a directory.

```toml
[github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_TOKEN" }   # $NAME comes from your environment
tools = ["search_issues", "get_issue", "create_issue"]      # optional: a subset
timeout_secs = 60                                            # startup and one call
```

Servers speak over stdio: TermIDE starts the process when the panel opens,
in the background, and reports in the session when it is connected or why it
is not. Its tools appear as `<server>__<tool>` (for example
`github__search_issues`), the shape OpenAI-compatible endpoints accept; they
are not listed in the system prompt, the model sees their schemas directly.
Every schema travels with every request, so a server with dozens of tools is
worth narrowing with `tools`; the log says so when a server has more than
twenty and no such list.

An MCP tool asks for permission like any other tool that no rule covers,
except in `auto` mode. "Allow always" writes a rule for the tool name:

```toml
[ai.permissions.github__search_issues]
"*" = "allow"
```

### Hooks

A hook is a program TermIDE runs around a tool call, declared in
`hooks.toml` at any of the three levels (same merging as MCP servers). It
gets the event as JSON on standard input and answers with JSON on standard
output, the shape Claude Code, Gemini CLI and Cursor share:

```toml
[no-force-push]
event = "before_tool_call"      # or after_tool_call
tools = ["bash"]                # patterns; every tool when absent
command = "scripts/guard.sh"    # run in the panel's directory
timeout_secs = 30
```

Before a call the input is `{"event","hook","cwd","tool","arguments"}`. The
program may print `{"decision": "block", "reason": "…"}` to skip the call
(the reason goes to the model), `{"decision": "allow"}` to run it without a
permission prompt, or `{"arguments": {…}}` to run it with other arguments;
no decision, or `"ask"`, leaves the permission rules to decide. Exiting with
code 2 blocks too, with standard error as the reason. After a call the input
also carries `"result": {"text", "is_error"}`, and `{"text": "…"}` rewrites
what the model sees; exit code 2 turns the result into an error with
standard error as its text. Hooks run in name order before the permission
rules; a hook that fails in any other way, or exceeds its timeout, is logged
and ignored, so a broken hook never stops the agent.

### Command shims

A shim is an executable in `shims/` named after a command. When the built-in
agent runs a shell command, `shims/` is prepended to the `bash` tool's `PATH`,
so a shim shadows the real command — including inside a pipeline
(`… | grep …`). It is the token-saving layer you extend by hand: a `shims/grep`
that runs a leaner search, a `shims/cat` that trims noise, each printing a
compact result the model reads. (The `bash` tool also cleans output on its own;
a shim is the part you control.)

```sh
# ~/.config/termide/ai/shims/rg  (chmod +x)
#!/bin/sh
# Cap ripgrep at a few matches per file, then hand the rest to the real one.
exec /usr/bin/rg --max-count 5 "$@"
```

Because a shim runs silently on every matching command, only the configuration
level's `shims/` is honoured — never a project's or the working directory's, so
a checkout cannot plant one. A shim is a normal executable in any language; call
the real tool by its absolute path (or `PATH= command <tool>`) to avoid calling
itself. External (ACP) agents run their own commands and are not shimmed.

### Project instructions

The agent reads `AGENTS.md` (or `CLAUDE.md` in the same directory) from every
directory between the filesystem root and the panel's working directory,
most specific last, so the panel directory's file outranks the project's. The
project root's file is included even when the panel works outside it. Put
your conventions there and the agent follows them; global rules go into the
template `ai/AGENTS.md` itself. Files over 32 KiB are skipped.

## Session history

Every session is written to a log in JSON Lines, one file per session, under
`ai/sessions/<path of the panel's directory>/` in the TermIDE configuration
directory, beside the agents. The log records the model and the agent the
session started with and every switch, so a reopened session continues on
the model and as the agent it last used. When a session approaches the
model's context window, the agent replaces the older part with a summary it
writes itself and keeps the recent messages verbatim; the panel says when this
happens, and `/compact` does it on request (see
[Service prompts](#service-prompts)). `/undo` writes a `rewind` entry: the
log keeps every message, but the branch continues from before the undone
request, on reopening too.

When TermIDE reopens a saved layout, the agent panel comes back with it and
continues the session it was in, on that session's model and as its agent. If
the log has been
deleted the panel starts a fresh session; if no model is configured any more
the panel is left out of the layout.

## From the command line

`termide --prompt "<prompt>"` runs one agent task without opening the UI and
prints the answer to stdout, then exits. It is the panel's agent — the same
`ai` directory, agents, tools and permission rules — driven headless, for
scripts, pipelines and CI.

```
termide --prompt "summarise what changed in src/main.rs"
git diff | termide --prompt -          # read the prompt from stdin
termide --prompt "run the tests and report failures" --agent runner
```

The answer is the only thing on stdout, so it pipes cleanly; tool activity
and errors go to stderr. `--agent` picks one of the defined agents, the
default agent otherwise. The exit code is `0` on success, `1` on failure and
`130` when interrupted.

For a machine-readable result, add `--output json`: instead of streaming, it
prints one JSON object at the end with the answer, the token usage, the tool
calls the run made and its status. `--output stream-json` instead prints one
JSON object per line as the run unfolds — a `tool_use` and `tool_result` for
each tool, a `message` for each answer, and a final `result` line carrying the
same fields as `json` — for a caller that follows a long run live.

```
termide --prompt "count the TODOs in src" --output json
# {"ok":true,"answer":"7","stop_reason":"stop","model":"…","provider":"…",
#  "usage":{"input":…,"output":…,"cache_read":…,"cache_write":…},
#  "tools":[{"name":"bash","subject":"rg -c TODO src","error":false}],"error":null}
```

No one is watching to answer a permission card, so a headless run does only
what the rules and the mode already allow: anything that would ask is refused
with a reason the model reads. For unattended work set `mode = "auto"` in the
configuration, or add `allow` rules for the exact commands and paths the task
needs. Plan mode has no meaning without the panel and is treated as `ask`,
and an external (`[acp]`) agent cannot be run this way.
