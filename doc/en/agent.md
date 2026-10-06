# Coding Agent

TermIDE has a built-in coding agent: a panel where you describe a task in
plain language and a language model carries it out by reading files, editing
them and running shell commands in your project. Every action it takes that
could change something asks for your permission first.

Open it with `Alt+A`, from **Windows → Agent**, or from the command palette
(**Open Agent**).

## Configuring a model

The agent reaches a model through a **connection**: an OpenAI-compatible
endpoint, Anthropic's Messages API, or a CLI agent. The OpenAI protocol covers
local servers (llama.cpp, Ollama, vLLM, omlx) and most hosted gateways, OpenAI
and OpenRouter among them. There are no connections by default; until one exists,
the panel refuses to open and says so.

```toml
[ai]
connection = "local"               # the one new sessions start on; else the first by name
max_tokens_per_turn = 0            # default: no limit, the model decides
reasoning = "high"                 # default; off | minimal | low | medium | high | xhigh | max
fold_blocks = "immediately"        # immediately (default) | on-finish | never
bell_on_attention = true           # default; ring the bell when a panel out of sight waits for you

[ai.connections.local]
provider = "openai_compatible"     # openai_compatible (default), anthropic_compatible, claude_code, codex, gemini_cli
base_url = "http://127.0.0.1:10000/v1"
model = "Qwen3.8-Flash-Next-oQ4e-mtp"  # left out: the provider's first model
# api_key_env = "OPENAI_API_KEY"   # name of the variable, never the key itself
# context_window_fallback = 32000  # used only when the server does not report a window
# prefill_progress = true          # ask a llama.cpp server for its prompt-processing progress
# reasoning_param = "enable_thinking"  # auto (default) | reasoning_effort | enable_thinking | none

[ai.connections.cloud]
provider = "anthropic_compatible"
model = "claude-sonnet-5"
api_key_env = "ANTHROPIC_API_KEY"

[ai.connections.codex]
provider = "codex"                 # a CLI agent needs nothing else
```

A connection carries `provider`, `base_url`, `model`, `api_key_env`,
`context_window_fallback` and, for `openai_compatible`, `prefill_progress`
and `reasoning_param`; what it leaves out takes that field's default.
`prefill_progress` sends `return_progress` with each request, which llama.cpp
answers with its prompt-processing progress; it is off by default because
servers that do not know the field (OpenAI's own API among them) may reject
the request. Everything else in `[ai]` — the output limit, reasoning,
permissions, compaction and the rest — applies whichever connection a session
runs on.

`reasoning` is the level new sessions ask for: `off`, `minimal`, `low`,
`medium`, `high` (the default), `xhigh` or `max`. Each model offers the levels
its API accepts, and a level it lacks falls to the nearest one it has: Claude
Opus 5.5 and Fable cannot stop thinking, so `off` gets their lowest effort,
and `max` on a model that tops out at `high` gets `high`. On the Messages API
the model decides the form: adaptive thinking with its effort on Claude 4.6
and later (asking for the reasoning's summary, which the newer models
otherwise leave out), a thinking budget on Claude 4.5 and older and on a
gateway's own models. An OpenAI-compatible connection sends the level in the
field its `reasoning_param` names: `reasoning_effort` (OpenAI's models offer
their own set of values, other models `low`, `medium` and `high`),
`enable_thinking` — `chat_template_kwargs.enable_thinking`, the on/off switch
of the Qwen3, GLM and DeepSeek chat templates on vLLM or llama.cpp — or
`none`. The default, `auto`, sends `reasoning_effort` to OpenAI, OpenRouter and
the Gemini API, `enable_thinking` to a server on this machine or the local
network (`localhost`, a `.local` name, a loopback or private address) — which
is how a local model gets its **Reasoning** chip — and nothing to other
servers, which may reject a field they do not know; a model that is sent nothing reasons as its server has it and shows
no **Reasoning** chip. The older `prefer_reasoning = true | false` still
reads, as `high` or `off`.

The settings modal (the gear, or the command palette) has all of it under
**AI**. **Connections** comes first: each row names a connection with its
provider and model, the one new sessions start on marked `●`. `Enter` or a
click opens a connection on a page of its own — name, provider, base URL, API
key variable, model, context window, **Prefill progress (llama.cpp)** and
**Reasoning parameter** (for an OpenAI-compatible one) and **Use by default** (new sessions start
on it) — and **[ Back to list ]**, `Esc` or `Backspace` returns to the list;
**+ Add connection** adds an OpenAI-compatible one, and
**[ Delete connection ]** on the page, or `Del` on its row, removes one.
Exactly one connection is the default: the first one added is, a later one
takes over only when you tick its switch, and turning the switch off (or
deleting the default) hands it to the first other connection by name. A new
connection is named after its provider until you name it. The model is a
dropdown: **Auto — the provider's choice** first, then the connection's
models, fetched in the background when its page opens, and last "Enter a model
id…" to type one by hand when the endpoint cannot list them. Auto, a
connection's model left empty, runs on the first model the provider lists (a
CLI agent on its own default); a request sent before that list arrives waits
in the input with a notice. Text fields edit like every input in termide: the
cursor moves by character and word, `Shift` or a mouse drag selects, and
`Ctrl+C`/`Ctrl+X`/`Ctrl+V`, `Ctrl+A` and undo work.

The API key is read from the environment variable named by `api_key_env`, so
the configuration file never holds a secret. Local servers usually need no key
at all; leave the variable unset. `fold_blocks` decides when reasoning and tool
calls fold to their one-line headline: `immediately` folds them while they
still run (a running reasoning block shows its latest line), `on-finish` shows
them in full until they finish, `never` leaves every block expanded; a
selected block unfolds with `Enter` or a second click either way. The settings modal offers it as
**Fold blocks**. An older `autofold = false` reads as `never`, `true` as
`on-finish`. With `max_tokens_per_turn` at zero or below no
output limit is sent and the model decides how long to reply; the Anthropic API
requires one, so there it becomes a generous 32000.

A configuration written before connections, with `provider`, `base_url`,
`model`, `api_key_env` or `context_window_fallback` directly under `[ai]`, is
read as one connection named `default` that new sessions start on; the next
save from the settings modal writes it in the new shape.

The banner's connection line and the status bar's **Connection** chip switch
the session to another connection: its endpoint and its model replace the ones
in use, the agent restarts on the same log and carries the conversation over,
and delegated tasks follow. A CLI agent (`claude_code`, `codex`, `gemini_cli`) does
not take over a conversation, so a switch to or from one works only before the first
request. The session log records the connection, so a reopened session
reconnects to it while it is still in the config, and to the one new sessions
start on otherwise.

For a hosted OpenAI-compatible endpoint, keep `provider = "openai_compatible"` and point
the connection's `base_url` and `api_key_env` at it, for example OpenAI itself
(`https://api.openai.com/v1`, `OPENAI_API_KEY`) or OpenRouter
(`https://openrouter.ai/api/v1`, `OPENROUTER_API_KEY`), or the Gemini API
(`https://generativelanguage.googleapis.com/v1beta/openai`, `GEMINI_API_KEY`).
A tool call a model signs — Gemini's thought signature, in the call's
`extra_content` — is kept in the session log and sent back with the call, as
the endpoint requires. For an Anthropic
subscription set `provider = "anthropic_compatible"`, drop `base_url` (the API root is
built in; set it only for a gateway) and point `api_key_env` at your
`ANTHROPIC_API_KEY`; the reasoning level then asks the model to think. The
**Model** chip lists the endpoint's models for each.

`provider = "claude_code"`, `provider = "codex"` and `provider = "gemini_cli"`
are different in kind: instead of the built-in loop talking to a model
endpoint, the panel drives that tool's own CLI as an
[external agent](#external-agents) over ACP
(`@agentclientprotocol/claude-agent-acp` / `@agentclientprotocol/codex-acp`
/ `@google/gemini-cli --acp`, the latest release, run through
`npx`). The CLI owns the endpoint and the sign-in — its own subscription or
API key — so the connection's `base_url`, `api_key_env` and context window do
not apply; the settings modal hides them for these providers and clears them
from the file. `model` is
kept: it is the model **pre-selected** on the agent — applied over ACP once the
session starts — and at runtime the **Model** chip lists and switches the
agent's own models. The
tool must be installed and signed in first (`npx` on `PATH`); termide does
not run a sign-in itself, so start Gemini CLI once on its own
(`npx @google/gemini-cli`) and choose how it signs in.

termide takes these agents as far as they let it, so a session runs the same
whichever connection it is on:

- **Claude Code** gets termide's system prompt in place of its own and calls
  termide's tools, served to it over a local MCP server, in place of its own —
  the tools of termide's MCP servers among them, as they connect or go.
  Its built-in tools and its own settings — their rules, hooks, `CLAUDE.md`
  and MCP servers — are left out. Every call runs in termide, through the same
  checks as the built-in loop's: the permission mode, the rules, the session
  answers, plan mode and the `/undo` checkpoints. Calls Claude Code makes
  together run side by side; only their permission decisions take turns. A
  call may run as long as it needs — a subagent, a long build, a card you have
  not got to — without Claude Code giving up on it; if Claude Code does give
  a call up, the call stops and its permission card comes down, so the calls
  after it go on.
- **Codex** keeps its own system prompt and tools; termide puts it in the
  modes that match the panel's (`ask`, `configured` and `auto`: ask for approval,
  `plan`: that plus its plan collaboration mode, `edit`: approve for me,
  `all`: full access) and decides what it asks.
- **Gemini CLI** keeps its own system prompt and tools too; termide sets its
  approval mode to match the panel's (`ask`, `configured` and `auto`: `default`, which
  asks before edits and commands, `plan`: `plan`, or `default` where Gemini's
  plan mode is off, `edit`: `autoEdit`, `all`: `yolo`) and decides what it
  asks.

Codex and Gemini CLI also get the tools of termide's MCP servers, served over
the same local MCP server beside their own and checked as Claude Code's calls
are; their own MCP configuration still applies. They read the list of an MCP
server's tools only once, when their session starts, so the session waits up
to ten seconds for termide's MCP servers to answer. A server that connects
later, or is reloaded, reaches them in the next session.

All three keep their own conversation loop: they compact their context themselves,
a run cannot pause between steps, and there is no prefill or generation
timing (token totals show when the agent reports them, as Claude Code does).
The **Permissions** chip works for all three. The settings modal says the same under a
connection's page.

## Using the panel

The session fills the panel, the input box sits at the bottom, under a titled
border that carries the agent's name (`─ default ─`) so parallel agent panels
are easy to tell apart. A long line wraps to the panel width and the box grows
to fit — up to half the panel's height — before it starts scrolling. Like a new
terminal, the agent works in the directory of the panel that had focus when
you opened it (a file manager's directory, an editor's file), or in the project
root. The panel title is your first request, so several agent panels stay
apart at a glance; before you ask anything it shows the working directory
instead. A title too long for the panel loses its end, so a wider panel shows
more of it. Give a session a name of your own through the panel's `[≡]` menu →
**Rename session**, and the title shows that name from then on.

A fresh session greets you with a banner: a small logo on the left and, on the
right, what the agent is set up with. Its title is the agent's name, with the
agent's description under it when the definition has one (the default agent,
unless a definition says otherwise, is described as "Agent", the label of the
panel's title); below them come the
directory it works in, its connection, model and the tools it may use, each
under a label in the language you set. The agent's name, the connection, the
model and the tools are shown bold in the accent colour: a click on any opens
the same picker its status-bar chip does, so you can set the session up before
you start. The directory is shown that way too while the panel is idle: a
click opens the directory picker (`.` there shows hidden directories), and the
panel moves to the picked one in place — the session log moves with it, so the
connection, model and agent picked stay, while the project agents, skills,
hooks and the list of sessions below are now that directory's. Further down, under the sessions heading, the list of this
directory's other sessions, newest first, each with the local date and time
of its last change, starting from the logo's column so the titles get the
panel's width, filling the space the panel has and scrolling through the rest
with the wheel. A session open in another panel is left out, and comes back
once that panel lets it go. A click opens one in place of the empty session;
from the keyboard, `Tab` moves into the list, the arrows, `PgUp`/`PgDn` and
`Home`/`End` walk it, `Enter` opens the session under the cursor, `F2` renames
it, `F8` or `Delete` deletes it after a confirmation, and `Tab` or `Esc` goes back to the
prompt. What the panel reports before then — an MCP server connected, a
`/name` defined twice — goes under the banner, past a dashed rule, the latest
in view; the list of sessions gives up its rows to it first. Each MCP server
keeps one line there, rewritten as it changes: its tools and how many of them
are on, connecting, needs sign-in. The banner gives way to the conversation as
soon as you send your first message, and those lines stay at the top of it;
from then on every change is a line of its own — a server reconnected or
changing its tools, a toolset applied (`mcp github: 3 of 12 tools on`,
`Tools switched off: bash; on: —`) — since from there the model has a
different set.

The **Tools** chip (and the banner's tools line) opens a checklist of what
the session may use: the built-in tools, the skills, and each MCP server's
tools once it has connected. Every group opens collapsed to its heading, which
shows how many of its items are on (`▶ [-] MCP github  3/12`): `→` or a click
on the arrow opens it, `←` closes it (or goes from an item up to its heading).
The checkbox on a group's heading switches the whole group on or off at once,
open or not, and shows `[-]` while it is partly on. The list applies however
it is closed — `Enter`, `Esc` or a click beside it. Every configured MCP server
has a heading, one that has not connected too — with its state beside the
name (`needs sign-in`, `failed: …`) — and buttons at its right end, standing
in one column: `[↻]` connects it again (`r` on the heading), and on a server
that signs in with OAuth `[⇥]` signs in or `[⇤]` signs out (`l`) to its left.
A button applies the ticks as `Enter` does, closes the list and does what it
says. A server that connects while the list is open appears in it there and
then — its heading loses its remark and its tools come under it — so you do
not have to close the list and open it again to see where a server stands;
what you had ticked stays ticked.
Unchecking an item before the first request keeps
it out of the model's context altogether — its description and schema are
never sent, which saves tokens and takes the capability away. Later in the
session an unchecked item stays in the context, so as not to throw away the
provider's prompt cache, but every call to it is refused; it leaves the
context at the next moment the cache is lost anyway — a compaction, an agent
or model switch — and cannot be switched back on in that session. The set is
kept in the session log, so a reopened session comes back with it. Claude Code
has the checklist too: it is served termide's tools when its session starts,
so an unchecked item is refused from the first request on rather than kept out
of its context. Codex and Gemini CLI have it once an MCP server of termide's
has connected, with those servers' tools alone. Other ACP agents bring their
own tools, so they have no checklist.

The panel's `[≡]` menu is kept to the actions with no home elsewhere —
**Session info** (also `F3` and `/usage`), **Rename session**, **Save chat as
Markdown…** (also `Ctrl+S`), **Fork session** (also `F5` and `/fork`) and
**Delete session**. The assembled system prompt
opens with `/prompt`. Saving the chat writes the conversation through a Save As
dialog, named after the session: each day under its own heading, and under it
each of your messages and each of the agent's answers under a heading of 🧑 or
🤖, the speaker's name (a custom agent gives its own) and the time.
Reasoning, tool calls and their output, and the panel's notices are left out;
a message sent as `/name args` is saved as you typed it. The whole branch is
saved, a compacted part included. Managing sessions
is on the F-keys and in the AI menu instead: `F7` starts a new session, `F6` opens
the picker of this directory's sessions (newest first, the current one marked
`●`), `F5` forks the current one, `F8` deletes it after a confirmation, and
`F2` renames it. Switching waits for the current task: stop it with `Esc`
first if the agent is still working. A fork asks nothing of the run, so it
works while the agent is busy.

Forking copies the session log and opens the copy in a new agent panel, which
takes up the conversation where it stood — the same messages, the same agent and
model — while the panel you forked from keeps working at its own log. The two
part at that moment: each goes on writing its own file, so one conversation now
runs in two places, and you can have the agent try two approaches side by side.
The copy takes a name of its own — the source's with a counter on it,
`Refactor` becoming `Refactor (2)`, then `Refactor (3)` — so the two are told
apart in the picker and in the panel's title; rename either whenever you like.
The copy is a session like any other: it shows in the session picker and in a
fresh panel's banner once its panel lets it go, and you can switch back to it.

From the input, `/new` starts a fresh session (keeping the current one),
`/fork` copies this one into a new panel, and `/clear` starts one too but
deletes the current one first; `/rename` (or `/name`) renames it. The model,
agent and permission-mode pickers are the status-bar chips.

A session you never send anything to is discarded when you switch away from it
or close the panel, so opening a panel and closing it — or trying a couple of
new sessions — leaves no empty logs cluttering the list or the disk. A session
you have named or sent even one message to is always kept.

| Key | Action |
|---|---|
| `Enter` | Send. While the agent works, the text is queued for the next turn instead and waits in the state strip above the input |
| `Shift+Enter`, `Alt+Enter`, `Ctrl+J` | New line in the input |
| `Esc` | Stop the running task; with nothing running, clear the input; with the input empty too, pick a message to rewind to (see [Rewinding to a message](#rewinding-to-a-message)). `Esc` never closes the agent panel |
| `Ctrl+O` | Expand or collapse every block, and new blocks after them |
| `Tab` | Move focus between the input and the chat; in the chat, `↑`/`↓` pick a block, `Space`/`Enter` fold or unfold it, `→`/`←` unfold or fold it as in the file manager's tree, `o` opens it in its own panel; on a fresh session's banner, `↑`/`↓` pick a recent session and `Enter` opens it |
| Click a block | Focus the chat and select that block (the selected block is shown inverted, success and error colours keeping their hue); click it again to fold or unfold it |
| Ctrl+Click on a URL | Open it in the browser |
| `Ctrl+C` | Copy: the selected prompt text, or — with a block selected in the chat — the block's text |
| `Ctrl+X` / `Ctrl+V` | Cut / paste the prompt selection |
| `Ctrl+A` | Select all the prompt text |
| `Shift+arrows`, `Shift+Home`/`End`, `Ctrl+Shift+arrows` | Extend the prompt selection by character, to the line edges, by word |
| `Ctrl+Left` / `Ctrl+Right` | Word-by-word navigation in the prompt |
| `Ctrl+Z` / `Ctrl+Y`, `Ctrl+Shift+Z` | Undo / redo a prompt edit |
| `Shift+Tab` | Cycle the permission mode: ask → plan → edit → configured → auto → all |
| `F2` | Rename this session (the same prompt as the `[≡]` menu); in the banner's list of sessions, rename the one under the cursor |
| `Ctrl+S` | Save the chat as a Markdown file (your messages and the agent's answers under who and when, each day under its own heading); also the `[≡]` menu |
| `F3` | Open the session-info modal (model, agent, mode, directory, created/last-active times, messages, compactions, tokens, context, how much shell output was cleaned); also `/usage` and the `[≡]` menu |
| `F4` | Pick a message to rewind the session to, as `Esc` does in the idle empty prompt |
| `F5` | Fork this session (after a confirmation): its log is copied and the copy opens in a new agent panel, while this one goes on working at its own |
| `F6` | Switch session — open the picker of this directory's sessions |
| `F7` | Start a new session (the used one is kept in the list) |
| `F8` | Delete this session (after a confirmation) and start a fresh one; in the banner's list of sessions, delete the one under the cursor |
| `/name args` + `Enter` | Send the prompt template `name` with `args` filled in, run the command script `name` or send the skill `name` (`/skill:name` when the name is taken); `/compact [focus]` summarises the session, `/undo` takes the last request back, `/new` starts a fresh session, `/fork` copies this one into a new panel, `/clear` starts one after discarding the current session, and `/rename [name]` (or `/name`) renames it; `/pause` stops the run after the current step and `/continue` resumes it (or, before the step ends, cancels the pause); `/loop [interval] <prompt>` re-runs a prompt on an interval (or back-to-back), `/loop stop` (or `Esc`) ends it; `/goal <what to achieve>` works autonomously toward a goal until a judge says it is reached, `/goal stop` (or `Esc`) ends it; `/handoff` briefs the unfinished work, then offers to save it to `HANDOFF.md` or start a new session from it; `/usage` opens the session-info modal and `/prompt` opens the assembled system prompt; `/mcp` lists the MCP servers, `/mcp reload [server]` reads their configuration again (for one server, or all), and `/mcp login <server>` and `/mcp logout <server>` sign in to one and out of it (see [MCP servers](#mcp-servers)) |
| `↑` / `↓` | On the first or last line of the input: take back the messages still queued (`↑`, while any wait), else recall an earlier request of this session, or come back to what you were typing |
| `Tab` | Complete the highlighted `/command` or `@file` while the list is open |
| `Ctrl+↑` / `Ctrl+↓`, `PageUp` / `PageDown` | Scroll the session |
| `Ctrl+Home` / `Ctrl+End` | Jump to the start, or back to following the newest output |

The conversation is a stack of blocks, each opened by an accent-coloured mark:
`› ` for your message and for the agent's answer, `@ ` for its reasoning, `$ `
for a shell call, `< ` for a file read, `> ` for a write, `± ` for an edit,
`/ ` for a skill, `& ` for a subagent, `* ` for an MCP tool, `¿ ` for a
question to you, `# ` for the system prompt. The answer is shown in full;
a user message or the system prompt longer than five lines is
folded to a preview that keeps the first line and the last few, with a
`… N more lines` note between them. A block of five lines or fewer has nothing
worth hiding, so it is shown in full with no fold marker; so is any finished
block that takes a single row unfolded. A finished tool call
folds to a single line: its headline (a shell call's first command line)
alone; how long it took (`🕒`) shows once it is unfolded. A tool call's mark
and action are in the accent while it runs, then turn the success colour once
it succeeds or the error colour once it fails; its subject keeps its own
colour. A single row carries no `✓`, the colour of its mark telling how it
ended; the unfolded form of a failed call ends with the `✗`. Durations grow from seconds to minutes, hours and
days (`2s`, `1m13s`, `2h5m`, `3d4h`). Finished reasoning folds the same
way, to its first line alone; unfolded, it shows the `⏫`/`✍️` lines. A block still in progress — a
streaming reasoning, a running tool call with its live output — is always shown
unfolded and folds only once it finishes. Your
message reads as plain text on a faint background; the reasoning, the system
prompt and a tool's output are dim text, except an edit's diff, which is
coloured the way the Git diff panel colours one. Every step opens with its type
glyph and a localized action in the same accent, then its subject: the
reasoning as `@ Thinking` and its text, a shell call as `$ Running` and its
command (dim, wrapped to the width), a file tool with its path
(`< Reading src/main.rs`, `> Writing`, `± Editing`), a web tool with the URL or
the query (`↓ Fetching https://docs.rs`, `? Searching ratatui scrollbar`), a
skill with its name and arguments (`/ Using skill review src/x.rs`), a
subagent with its name and the first line of the task (`& Delegating to
reviewer: check the diff`), an MCP tool with its server, name and arguments as
`key=value` (`* Using MCP github: create_issue title=Crash`); any other tool
as its name and a summary. The reasoning is its own block above the
answer, and its text wraps to the width. Only the system prompt
opens with a dim dashed rule that sets it apart from the block before; an
annotation has none, its glyph marks it. A blank line follows your message and another precedes the answer, and one
follows an answer that more steps come after; the reasoning and tool calls in
between stack with no gap. A folded block is marked
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
closed by the next request). Claude Code, Codex and Gemini CLI run their own
loop, which cannot stop between steps: they show `[■]` alone, and `/pause` says so. The
controls take the panel border's accent color at rest; `[▶]` is green. Once `[■]` (or `Esc`) is pressed,
a stop cannot be taken back: until the run has actually stopped, only a red
`[■]` stays, and pressing it again does nothing.

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
work figures, no wall-clock, and show them only unfolded. A tool call shows
how long it took (`🕒 6s`) and its status. A call that waited on a permission question shows that
wait apart, first, as the pause it was: `‖ 12s 🕒 2s`, the `‖` marked and
ticking while the question is up; the `🕒` counts only the call's own run. When a turn reasons, the reasoning block carries the turn's cost —
the prefill phase (`⏫ 6s (↑42k, 7k tok/s)`) and the generation phase
(`✍️ 12s (↓5k, 420 tok/s)`), each with its duration (whole seconds), token count
and average speed; large counts are abbreviated (`40k`, `1.2M`). A turn with no
reasoning shows those on the answer instead.
While a turn is still running, the same right-aligned meta zone shows the live
figures: until the first token, a `⏫` prefill line with how long the model has
been reading the prompt and an estimate of its size (`⏫ 14s (↑~48k)`), or, from
a server that reports its progress (`prefill_progress`), a bar with the tokens
read so far and the speed over those not served from its cache
(`⏫ 14s ▰▰▰▰▱▱▱▱ (↑24k/48k, 1k tok/s)`); once
tokens stream, a `✍️` generation line with the running duration, estimated
tokens and speed; and below either the run clock. The exact input count and
prefill speed come with the finished block. A compaction, `/compact` between
runs included, shows the same `⏫`/`✍️` lines for its summary call, and the
status bar's context figure drops to the summary and the kept tail as soon as it
ends. An external agent shows no live
prefill line, as its message starts with its first text. Reopening a
conversation restores each block's time, its reasoning, the turn's
prefill/generation lines and each tool call's duration from the log.

Unfold a block to see all of it: click it, or press `Tab` to move into the
chat and `Space`/`Enter` or `→` on the block the `↑`/`↓` cursor is on (`←`
folds it back). `Ctrl+O`
unfolds everything at once and keeps the blocks that arrive after it unfolded
too; pressed again, it folds everything back and new blocks fold as
`fold_blocks` says (on finish, when that is `never`). `o` opens the selected block in its own
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

The status chips run, left to right: the agent, the permission mode, the
**Reasoning** level (none for a model that cannot be asked), the connection with its protocol
(`local · OpenAI Compatible`) and the model. The session's token totals and
the context window sit flush right; on a narrow terminal the chips on the left
are cut, never these. The totals are `↑` the prompt tokens billed in full (the
uncached input and what was written to the prompt cache), `↻` those the cache
served, shown once there are any, and `↓` the output: `↑2.1k ↻48k ↓900`. The
window is the tokens used of it with a fill bar, `66k/262k ▰▰▱▱▱▱▱▱`; for
Claude Code, Codex and Gemini CLI both come from what the agent reports, and the window
shows once it has.
Agent, mode, reasoning, connection and model are buttons. Clicking **Reasoning** lists
the levels the model offers (an on/off model just flips) and applies the one
picked from the next request; the choice is remembered in the session, so a
resume comes back with it, and stays when the model changes, falling to the
nearest level the new one has. What the agent is doing right now is
not repeated in the status bar: each chat block carries it in its byline.

Typing `/` opens a list of the matching prompt templates above the input;
`↑`/`↓` move in it, `Tab` or `Enter` complete the highlighted one, and `Enter`
on a name typed in full sends it.

Typing `@` opens the same list with the files and directories under the
panel's directory instead, so a path is a few keystrokes: `@ma` finds
`src/main.rs`, and the letters need not be adjacent (`@srmain`). What git
ignores is left out, and hidden entries unless you type the `.`. `Tab` or `Enter` inserts the highlighted one; a directory ends
in `/` and reopens the list for its contents, so you can drill in. The agent
reads the file you name; `@` is only quick path entry, nothing is attached
behind your back.

A large paste (more than five lines or 2000 characters) is held out of the
prompt box as a short `[#1 pasted 40 lines]` placeholder instead of flooding
it; the full text is spliced back in place of the placeholder when you send,
so the model still gets all of it. Pasting the same block again unmasks it: the
placeholder gives way to the full text, to read or edit in place. A smaller
paste goes in as it is.

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

The **Permissions** chip offers the six modes described below. `Shift+Tab` cycles
through them without the picker. A change applies at the agent's next tool
call, so you can loosen the mode while a long task is running instead of
answering the same prompt again and again. Neither switch touches the
configuration file; the panel starts from `[ai]` again the next time.

### Running a command yourself

`$` typed into an empty prompt switches it to a shell command: the `$` takes
the place of the `›` marker, and `Enter` runs what you type there and then,
without asking the model anything. Each command switches the mode on anew:
running it, `Esc`, or `Backspace` in the empty prompt returns to messages. In
shell mode `↑` and `↓` walk the commands you ran before rather than the
messages. Text that merely opens with a `$` — pasted, or after a space — is
still a message.

The command runs in the session's directory through the same `bash` the agent
uses, so a long output keeps its beginning and end and writes the complete log
to a file. It shows in the transcript as a shell call block, like the agent's
own — `$ git status --short`, folded, with its time and a `✗` if it failed;
unfolded, it says you ran it — and the agent reads the command with its output
when you next ask something, so you can follow a command you ran with "what
does that mean for the fix". `Esc` stops a command still running; running one
never spends a model call, and works while the agent is busy too.

What you type is what runs — no permission card, because you wrote it. That is
the whole of the difference: the agent's own commands are still judged by the
mode and the rules below, and a command the agent offers you (see
**suggest_command**) runs only if you confirm it.

### Undoing a request

`/undo` takes back the last
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

### Rewinding to a message

`Esc` in the empty prompt, with nothing running, lists the messages you sent
in this session right above the prompt, as the `/command` completions are
(`F4` opens the same list at any time). The newest stands next to the
prompt and is selected, `↑`/`↓` move through the others, `Enter` (or a
click) picks one, and `Esc` or typing closes the list. A message after
which the agent changed files names them beside it. Picking one rewinds the conversation to just before it and puts the message back into
the prompt, to be edited and sent again — unless you have started typing
something else. When files changed since that message, a card names them
and asks what to put back: the files and the conversation, the conversation
only (the files keep their current content), or the files only (the
conversation goes on where it stands). The files come back from the same
checkpoints `/undo` uses, and either way the checkpoints of the requests
rewound past are used up. As with `/undo`, the rewound messages stay in the
session log on a dead branch.

## Tools

The agent's built-in tools are listed below, plus those its MCP servers
provide (see [MCP servers](#mcp-servers)). The panel's own agent also has
**question** and **suggest_command**, described after the list.

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
- **recall** searches the project's earlier agent sessions, its git history
  and its code for past work, decisions and their reasons; see
  [Recall](#recall).

Searching the project's files by pattern is done through `bash` with the tools
you already have (`rg`, `find`), rather than through a separate search tool.

One more tool, **skill**, appears when skills are defined; see
[Skills](#skills).

**question** lets the agent ask you when a decision is yours to make: which
approach to take, what an ambiguous requirement means. It puts up to four
questions, asked one after another as a card in the panel above the input.
The title says the agent asks, with the question's topic and, when there are
several, its number (`Agent asks: Approach (1/2)`); the question itself sits
dim under it, then the choices, each with a dim note on what it means. A
question offers one choice to pick, or several: then each choice has a
checkbox that `Space`, `Enter` or its digit toggles, and **Submit** sends them.
**Type your own answer** always follows the choices — a line you type instead,
or, where several can be picked, alongside them. `↑`/`↓`, digits and clicks
work as on a permission card; **Decline and stop the run**, or `Esc`, tells the
agent you declined and stops the run, so you can say what you want in your own
message. With several questions, `←` goes back to the one before and `→`
on past one already answered, as the arrows in the title show
(`Agent asks (← 2/3 →)`): an answer given before shows picked, and
answering again moves on to the next. The answers go back once the last
question is answered. Waiting for an answer counts as the call's pause
(`‖`), as a permission question does. Asking never needs a permission, in plan mode too.
Only the panel's own agent has this tool: a subagent and a `termide --prompt`
run have no one to ask and decide on their own. An agent whose `tools` list leaves out
`question` does not ask either.

**suggest_command** hands you a command instead of running it. The agent puts
one on a card — the command in full and exactly as it would run, why it is
offered, and the directory — marked as the agent's suggestion. **Run** runs it
through the same shell path as a [`$` command](#running-a-command-yourself) you typed,
and its output comes back to the agent; **Edit first** puts it in the prompt as
a shell-mode command for you to change and run; **Copy** takes the text alone;
**Don't run**, or `Esc`, declines, and the agent is told it did not run and must not offer it again. This is what
a blocked call ends up doing: the refusal tells the agent to say what it needs
run, and this turns that into a card rather than a line of text you retype. It
is also how the agent hands over something it should not do itself — publishing,
anything needing your credentials or your judgement.

The card never runs anything on its own, and it cannot talk you past your own
rules: where plan mode is on or a `deny` rule covers the command, **Run** and
**Edit first** are left off and only **Copy** and **Don't run** remain. Offering a command costs
no permission — it changes nothing by itself — so the agent can always reach you
this way; only your confirmation reaches the shell. Like `question`, the tool
belongs to the panel's agent alone.

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

In `plan` and `edit` both tools run without asking; in `ask` and, without a
rule, in `configured` they ask; in `auto` a search runs and a fetch, whose
URL can carry data out, goes to the reviewer. "Allow" beyond once for `fetch` covers the
whole site (`https://docs.rs/*`), for `web_search` every query.

A file the agent edits while it is open in an editor is reloaded there at
once, cursor and scroll position kept, unless that editor has unsaved changes;
then the editor keeps them and marks the conflict, as with any change on disk
(see [Changes on disk](editor.md#changes-on-disk)).

### Recall

`recall` searches what the project already knows — the logs of its earlier
agent sessions, its git history and its files — and ranks the results
together. It is how the agent finds what was decided, tried or discussed
before ("why did we drop tokio", "where did the compaction work stop") without
a separate memory to keep up: the logs, the commits and the files are the
record, and the tool writes nothing. It serves a project of notes and documents
as well as one of code.

- **Sessions**: the project's logs under `ai/sessions/` (the project root and
  the directories under it), along each log's live branch — what `/undo` or a
  rewind took back is left out, what a compaction summarised is still found.
  Your messages, the agent's answers, compaction summaries and handoff briefs
  count most, tool calls less, reasoning and tool output least (only the first
  4 KB of an output is searched). A session on Claude Code, Codex or Gemini
  CLI logs its tool calls and their output too, as the built-in agent's does.
  Of the session the panel is in only what a
  compaction took out of the context is searched: the rest is there already. A `git commit` a session ran is
  linked to its commit.
- **Git**: commits whose message holds a word of the query, and commits that
  added or removed an identifier it names (`git log -S`), in every repository
  the project holds — the one it is in, its submodules, or the repositories of
  a folder that only contains them, as the git panels find them. A project that
  is one directory of a larger repository sees that directory's history.
- **Files**: notes, documents and code under the project with a word of the
  query. Text files (Markdown, plain text, reStructuredText, Org, AsciiDoc) are
  prose: a Markdown file is searched heading by heading, so each section of a
  long file — each entry of a decision log — is a result of its own, labelled
  with its headings; a match is shown as its whole paragraph, and a recently
  changed file ranks above a stale one. Any other file is code: one result per
  file, shown by its matching lines. As with ripgrep, hidden entries, ignored
  files and binary files are skipped, and the walk stays on one file system (a
  mounted disk or a virtual machine's files under the project are not read);
  files of any size are read line by line. Every repository nested in the
  project is walked with its own `.gitignore`, so one the outer repository
  ignores is still searched; the session logs are not read as files.

A panel moved to a directory outside the project searches that directory as
well — its session logs, its repositories and its files — and names what it
finds there by its path.

Words match by stem, in English and Russian, and identifiers by their parts,
so `split_command_line`, `splitCommandLine` and "split the command line" meet,
and "сессия" finds "сессии"; a Russian word is also matched by its stem
without the case ending, so a name the stemmer does not know ("сомбала",
"сомбалу") is found in every case. A two-letter acronym typed in capitals
("CI", "UI") is matched as a whole word, not inside a longer one. The agent passes several phrasings of what
it looks for, and each is ranked on its own: a result needs most of one
phrasing's weight, where a rare word counts for far more than common ones. Each result carries a reference —
`session:<id>#<entry>`, `commit:<repo>@<sha>` or `file:<path>:<line>` — and the
agent calls `recall` again with `open` set to a reference to see the entries
around a session result or the commit itself, and reads a file from the line
given. A search can be narrowed to
`sources` (`sessions`, `git`, `files`), `paths` (paths or globs, relative to
the panel's directory like any path the agent gives) and `since` (a date, for
sessions and commits). File results name their path the same way, so the agent
reads them as given.

`recall` only reads, so it never asks for permission, in plan mode too;
subagents and `termide --prompt` runs have it as well, and an agent's `tools`
list can leave it out like any other tool.

With the solver on, one more model call reads the results and answers the
question from them, citing the references, and the results follow the answer
so the agent can check it. It is off by default.

The three sources search side by side, each with its own time limit, a minute
by default: what a source found by its limit is ranked with the rest, and the
result says which source stopped — git included when one slow command, a
`git log -S` over a long history, ran into it. So a project where one source is slow — the
files of a project rooted in the home directory, say — does not cost the others
their time, and a search lasts as long as its slowest source.

```toml
[ai.recall]
sessions_timeout_secs = 60   # how long each source may search, in seconds; 0 is no limit
git_timeout_secs = 60
files_timeout_secs = 60
solver = false               # answer from the results with one model call
connection = ""              # the connection whose model answers; empty uses the session's
```

The solver's instructions are `system/recall.md` (see
[Service prompts](#service-prompts)).

## Permissions

Nothing that changes your project happens without your say-so. When the agent
wants to do something that is not already allowed, a card appears in the
panel above the input. Its title is the intent — "Agent wants to run bash:" —
and under it, dim, exactly what that is (the command or path); a long one
folds to five lines that a click unfolds. The rows follow: allow once, allow
for this session, in `configured` and `auto` also allow always in this project and
allow always everywhere, then deny, deny for this session, **deny and tell the
agent why** (a sentence you type, returned to the model as the reason, so it
can take another way), and **stop the run**. The rows that outlast the call
name the pattern they record (`cargo build *`, a site, a path). `↑`/`↓` and
`Enter`, or the row's digit, answer it; a click picks a row and a second click
(or `Enter`) confirms it, so a stray click cannot answer; `Esc` stops the run.
The status line announces the question too, so a panel that is not in focus
does not ask unseen; the header of such a panel shows 🔔 in place of its icon
until the panel is focused, and so does it when a run ends. Unless the panel is the one
in front of you in a focused terminal window, the question also rings the
terminal bell, and so does the end of a run that took ten seconds or more (not
one you stopped); it rings once until you look at the panel, and
`bell_on_attention = false` under `[ai]` silences it. "In this project" appends the rule to
`.termide/config.toml` in the project, "everywhere" to the global
configuration; answers for the session live until the panel closes.

Every call keeps who decided it, in the session log and so across a resume:
the rules, plan mode, a hook, the auto mode reviewer with its reason, no one
(a subagent or a headless run that had no one to ask), or you — with the
answer you gave and how long it holds. An unfolded call shows it on a line
of its own (`✓ you allowed it for this session`, `✗ the auto mode reviewer
blocked it: …`); a call the rules allowed without asking, the common case,
says nothing. A refusal by a rule tells the model the rules are yours, so it
does not go looking for another way to the same thing. What the model reads
for each kind of refusal — a rule, plan mode, the reviewer, your denial, a
run with no one to ask — is `system/permissions.md` in the configuration's
agent directory, one line per case; edit it to word them your way.

Rules live per tool. Among the rules that match, the strictest wins, so a
`deny` always beats an `allow`:

```toml
[ai.permissions]
mode = "auto"       # ask | plan | edit | configured | auto (default) | all — what new sessions start in

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
on either half stops the whole command. The shell's own words are not parts:
in `if [ -f x ]; then make; fi` the parts are `[ -f x ]` and `make`, and a
`for … in` header or a closing `fi` or `done` is no part at all. Command substitution (`$(…)`, backticks)
is never allowed automatically, and it asks about the part that carries it and
not about the parts beside it, so `cd x && ls && echo $(date)` asks about the
`echo` alone; the operators inside a substitution belong to it and split the
line no further. An inline script is one command, not the
commands its lines would be: a quoted script (escaped quotes included) and
the body of a here-document (`python3 - <<'EOF'`) are the command's data.

A command's parts are judged in the directory they run in: termide follows
`cd`, `pushd` and `popd` through the line, and a program named by a path is
matched as the resolved path — project-relative inside the project, absolute
outside — as well as as written, so `cd /tmp && ./venv/bin/pip install x`
matches `/tmp/venv/bin/pip *`. The card asks only about the parts no rule or
look-only default settles, lists them under the command, and an answer for
the session or for always records a rule for each. A part whose directory
cannot be told (after `cd $DIR`, `cd -` or a subshell) or that runs a
substitution is marked "this time only": no rule is recorded for it, and
when no part can have one, the card offers only allow once and deny.

An answer that outlasts the call is a decision made without seeing the calls
it will cover, so the card offers it only where the rule says enough to trust.
**Allow always** appears for one command whose pattern states its scope: not
for a command made of several parts, which you answered as a whole; not for
one that destroys what it touches (`rm`, `git clean`, `git push`, `git stash
drop`, installing or publishing a package); not for one that runs some *other*
program, whose name the pattern would not show (`env rm`, `xargs rm`,
`./deploy.sh`, `sh -c …`, `python3 -c …`, `make`, `nix run`). **Allow for
this session** is withheld only from a command that destroys: it is short
enough to live with, and ends when the panel closes. So a destructive command
is allowed once at a time — `all` is the mode that lets a run through without
asking. **Deny for this session** is always on
offer: it trusts nothing. The rows withheld are absent, not dimmed, so the
answer you give is the answer that is recorded.

The mode decides which rules count and what happens to anything none covers.

`mode` in the configuration is the starting point every new session takes
(`auto` unless you change it), also set from the settings modal's **AI**
section under Permissions; the panel's **Permissions** chip and `Shift+Tab` change it
for the current panel only.

- **ask** asks about everything; the configured `allow` rules do not count,
  only your answers in this session do.
- **plan** reads and uses the web, and refuses anything that could change
  something; the agent answers with a plan. See below.
- **edit** also edits and creates files inside the project without asking;
  commands, MCP tools and files outside the project ask. The configured
  `allow` rules do not count.
- **configured** follows the configured rules and your answers in this
  session, asks about the rest, and offers "allow always".
- **auto** (the default) follows the configured rules less the broad `allow`
  ones, edits inside the project without asking, and hands what would
  otherwise ask to a reviewer model that allows or blocks it in your place.
  See below.
- **all** allows everything.

| | ask | plan | edit | configured | auto | all |
|---|---|---|---|---|---|---|
| read inside the project | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| read outside it | ? | ? | ? | rules / ? | rules / R | ✓ |
| web search | ? | ✓ | ✓ | rules / ? | ✓ | ✓ |
| fetch a page | ? | ✓ | ✓ | rules / ? | rules / R | ✓ |
| edit inside the project | ? | ✗ | ✓ | rules / ? | ✓ | ✓ |
| edit outside it | ? | ✗ | ? | rules / ? | rules / R | ✓ |
| look-only command | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| other command, MCP tool | ? | ✗ | ? | rules / ? | rules / R | ✓ |

✓ runs, ? asks, ✗ is refused, R goes to the reviewer, "rules / ?" follows a
matching rule and asks without one. Whatever the mode, a rule's `deny` refuses and its `ask` asks:
the modes set the configured `allow` rules aside, never the refusals. Your
answers for the session count in every mode but `all`.

The look-only commands are a short list that only look at things (`ls`,
`cat`, `rg`, `git status`, `git log`, `find`, `cd`, and similar); a
redirection into a file disqualifies it, while pointing a stream at another
(`2>&1`) or at `/dev/null` does not. So do the arguments with which such a
command writes a file or runs another program: `env` counts only without a
command to run, `git branch`, `git tag`, `git remote`, `git stash`,
`git worktree`, `git reflog` and `git config` only while they list or get
(never with `-c` or `-C`: a setting or another repository's config can
run a program), `git grep` only without `-O`, the forge CLIs `gh`, `glab` and `tea` only to list, view and search
issues, pull/merge requests, CI runs, releases, repositories and labels
(no `--web`, no `api`; `glab ci view` and `glab ci status` ask, since they
can retry jobs, and so does a bare `tea comments`, which posts), `sed` only while it prints (no `-i` or `-f`, and a script of
addresses with `p`, `l`, `=`, `q` or `d`, such as `sed -n '40,80p'`; a
substitution asks), `find` only without `-delete`, `-exec`, `-ok`, `-fprint` or `-fls`, and
`sort -o`, `uniq` with an output file, `tree -o`/`-R`, `rg --pre` and
`git diff --output` ask. Loading a skill never asks.

### Plan mode

For a task you want to see thought through before a line changes, switch to
**plan** (the chip, `Shift+Tab`, or an agent whose `AGENT.md` says
`mode: plan`). While it is on, the instructions from `system/plan.md` are
added to the system prompt, and every tool call that could change something
is refused with a message the model reads, whatever the rules, the session
answers or a hook's approval say: `edit`, `write`, MCP tools, and any shell
command that is not on the look-only list; for a command the message names
the part that is not, so the agent can take another way. Reading inside the
project, the web and `skill` run without asking; reading outside it asks.
`task` runs too: the subagent is held to plan mode whatever its `AGENT.md`
says, so it only reads.

The default instructions have the agent work the task through with you
before it writes the plan. It explores first and finds out the facts itself,
handing wide searches to a subagent. Then it puts the open decisions to you in
rounds of `question` cards, up to four questions a round, each with its
recommended answer first. Every round builds on the answers to the last, and
there are as many rounds as the task needs: dozens on a large one, none on a
small, clear one. Routine choices it makes itself and names in the plan. The
plan then gives the goal and how to tell it is done, the decisions and why,
what is out of scope, the steps in order with the files each changes, the
risks and the checks.

When the agent answers, a card asks what to do with the plan:

```
┌ Plan mode: carry the plan out? ──────────────┐
│ 1. Yes, accepting edits, from a clean context│
│ 2. Yes, accepting edits                      │
│ 3. Yes, under the configured rules           │
│ 4. Keep planning                             │
└──────────────────────────────────────────────┘
```

The first three leave plan mode for edit or configured and send the request
named in the front matter of `system/plan.md`, so the same session goes on to
carry the plan out; the last (or `Esc`) keeps plan mode, and whatever you
type next refines the plan. The plan is the agent's answer in the session,
nothing is written to a file; the `/undo` checkpoints cover the changes that
follow.

The second and third keep everything the planning read in the context and
send `request:`. The first clears the exploration out of the context before
it sends `clean_request:`: what stays is your messages, the questions you
answered with your answers, the skills loaded and every answer the agent
ended a run with, the plan among them; the files read, the commands run, the
web pages and the subagents' reports go. Nothing is summarised, so the
decisions stay word for word, and the agent re-reads what each step touches.
It suits a long planning session whose exploration would crowd the work.
The session log keeps all of it: reopening the session shows the cleared
conversation, as after a compaction, `recall` still finds what was cleared,
and undoing the carry-out request brings it back.

On Claude Code, Codex and Gemini CLI the card comes up too, without the
clean-context row: such an agent keeps its own history, so there is nothing
of termide's to clear. An external agent that answers to its own
configuration has no plan mode and no card.

### Auto mode

**auto** is for a run you trust in its direction but do not want to answer
card after card. What a rule or a safe default settles is settled as in
`configured`; everything that would put a card up goes to a reviewer, a
separate call to a model that decides whether the action is a reasonable
step toward what you asked for or goes beyond it. An allowed call runs; a
blocked one does not, and the model reads why, with the advice to take a
safer way or tell you what it needs. Claude Code's auto mode and Codex's
auto-review work the same way.

The reviewer sees what you wrote in the session and the calls the agent made,
and never the results of those calls nor the agent's own text. The results
are where hostile content from a file or a web page enters, and the agent
must not argue its case, so neither reaches the reviewer. A boundary you set
("don't push", "wait for my review") counts until you lift it, and it
outlives a compaction: the reviewer's record of your words is kept apart
from the context the model sees.

Some things are never left to the reviewer, and ask as in `configured`:

- a call an `ask` rule covers;
- removing the filesystem root, the home directory, or the working directory
  or one above it (`rm -rf ~`, `rm -rf .`, `rm -rf *`);
- everything, once the reviewer has blocked three calls in a row or twenty in
  the session: the questions come back to you, and allowing one hands the
  decisions back to the reviewer;
- a call the reviewer could not decide: the call failed, timed out or
  answered something other than a verdict.

A rule's `deny` refuses as in every mode. The broad `allow` rules — a
whole tool (`"*" = "allow"`), a wildcard after an interpreter, a script
runner or a wrapper (`python3*`, `npm run *`, `make*`, `env *`), an `allow`
for `task` — are set aside, since they would let any program run past the
reviewer; narrow ones such as `"cargo test*"` still pass without it.

What the reviewer is told is `system/classify.md` in the configuration's
agent directory: what to allow, what to block, and the shape of the verdict.
Edit it to tell the reviewer about your infrastructure or your habits. By
default the session's own model reviews; `auto_reviewer` under `[ai]` (in
the settings modal, **Auto mode reviewer** under Permissions) names another
connection to review with, a small fast one being the usual choice. A review
is one more model call before each action it decides; reads, edits inside
the project and look-only commands never reach it and cost nothing.

```toml
[ai]
auto_reviewer = "haiku"   # a connection's name; empty reviews with the session's model
```

The reviewer judges delegated work too: a subagent's calls are reviewed
against your words and the task it was given, marked as another agent's.
Headless runs use it the same way, falling back to a refusal where the panel
would ask. An external (`command`) agent keeps its conversation to itself, so
in `auto` its requests are asked about as in `configured`.

## The AI menu

The **AI** menu in the top menu bar (after **Projects**) manages the agent's
resources without opening a panel. It has four sections — **Agents**,
**Sessions**, **Skills**, **Prompts** — each opening a list you browse with
`↑`/`↓`; the arrows, `Enter` and the mouse work as in the **Commands** menu.
Items merged from the project (bold) and the global configuration are shown
together (see [The agent directory](#the-agent-directory)).

- `Enter` (or `F4`) edits: it opens a skill's `SKILL.md` or a prompt's `.md`
  in an editor, and an agent's `AGENT.md` the same way; a session resumes —
  focusing the panel that already shows it, or opening a new agent panel when none does.
- `Delete` removes the item (with a confirmation); `F2` renames it (for a
  session this sets its display name). The session confirmation names the
  session (its display name, first prompt, or "untitled") and its id.
- The first two rows of Agents, Skills and Prompts create a new item — **New
  (in project)** under `.termide/ai/`, or **New (global)** under the
  configuration directory — asking for a name and opening the new file.
  Sessions are created by running an agent, so they have no create rows.
- Each session row shows, dim on the right, when it was last worked on
  (e.g. "2h ago").
- An agent, skill or prompt with a description is listed as `name ·
  description` (the `description` front matter of an `AGENT.md`, a
  `SKILL.md` or a prompt); a row too long for the menu ends
  in `…`.

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
  agents/default/AGENT.md  the default agent (config level only)
  agents/<name>/AGENT.md   any other agent: settings and prompt template
  skills/<name>/SKILL.md   skills, see below
  prompts/<name>.md        prompt templates, typed as /name
  commands/<name>          command scripts, typed as /name, see below
  shims/<command>          command shims (config level only), see below
  mcp.toml                 MCP servers, see below
  mcp-auth.json            MCP sign-ins (config level only), see MCP servers
  hooks.toml               command hooks, see below
  system/compact.md        how the agent summarises a long session
  system/compacted.md      how the summary is worded in the context
  system/plan.md           what plan mode tells the agent, and what accepting a plan sends
  system/goal.md           how the judge decides whether a /goal is reached
  system/handoff.md        how /handoff briefs the unfinished work
  system/recall.md         how recall's solver answers from the results
  system/classify.md       what the auto mode reviewer allows and blocks
  system/permissions.md    what the model reads when a call is refused
  tools/<name>.md          what the model is told a built-in tool does
  web/engines/<name>.toml  search engines for web_search, see Web
  web/browser/             the web tools' browser profile (config level only)
```

The first time the panel opens, the configuration level is laid out: the
default agent's `AGENT.md`, the `system/` and `tools/` files and the search
engines receive the shipped texts, `skills/`, `prompts/`, `commands/` and
`shims/` are created empty. A default agent template kept as `ai/AGENTS.md`
by an earlier version is moved into `agents/default/AGENT.md` then. termide
records what it shipped (in `.seeds.toml`) and keeps these files current on
later starts: one you never edited is refreshed when the shipped version
changes, so upgrades reach you; one you edited is left untouched, with the new
default written beside it as `<file>.new` to compare and merge at your leisure.
Delete a file to get the shipped version back.

### Agents

An agent is a directory under `agents/`. `default` is the one the panel
starts as; only the configuration level defines it, so a project cannot
change the agent every panel starts as. Any directory with an `AGENT.md`
defines an agent you can switch to from the **Agent** status chip (an empty
file will do; a directory without one defines none); the picker shows each agent's
description. The agent is its `AGENT.md`: the front matter sets it apart,
every field optional, and the body is its own prompt template. An `AGENT.md`
with no body speaks with the default agent's template, not
with its settings; the default agent without one with the shipped template.
`prompt: none` means no system prompt at all: the model still gets the tools,
but no instructions. The rest of the directory is the agent's own: scripts or
checklists its prompt refers to.

```markdown
---
description: Reviews diffs and points at risks
model: Qwen3.8-27B-MTPLX-Optimized-Quality
mode: edit
tools: read, bash
---
You review the changes you are given. …

{{tools}}
```

The front matter is `key: value` lines; a line starting with `#` is a
comment, and a value may be quoted. `model` is a model id at the configured
endpoint; `mode` is the permission mode the agent starts in (`ask`, `plan`,
`edit`, `configured`, `auto` or `all`); `tools` lists the built-in tools the
agent keeps, separated by commas (brackets around the list are fine too, and so is a
YAML list of `- name` lines under `tools:`),
`task` included; without it the agent has them all, and `tools: []` leaves
none. Skills and MCP servers' tools are not governed by
it: they come with what you configured. A key termide does not read — a typo
such as `descripton:` — is reported under the banner when a panel opens.

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
everything into the prompt. It runs in the session's current mode, unless its
`AGENT.md` names one (plan mode overrides that: a delegate of a planning
agent only reads), with no one to prompt, so it can only do what the
rules and that mode already allow: anything that would otherwise ask is
refused with a reason it reads. In `auto` the reviewer decides those calls
instead. External agents (those with a `command`) cannot be delegates, and a
subagent gets no `task` tool of its own, so delegation does not nest. An agent whose `tools` list leaves out `task`
cannot delegate. A run that will not stop is cut off after fifty
model calls.

### External agents

An agent may be another program altogether: put a `command` in the front
matter of its `AGENT.md` and the panel drives it over the
[Agent Client Protocol](https://agentclientprotocol.com) instead of running
the built-in loop. Claude Code, Codex and Gemini CLI have ACP adapters or
speak it natively:

```markdown
---
description: Claude Code through its ACP adapter
command: npx -y @agentclientprotocol/claude-agent-acp
---
```

`command` is the program and its arguments, split as a shell splits words
(quotes keep spaces in an argument) but not run through a shell, so nothing
in it is expanded. The program inherits TermIDE's environment; an
`env.<NAME>: value` line per variable adds to it or overrides it, `$NAME` and
`${NAME}` in the value coming from TermIDE's environment
(`env.ANTHROPIC_BASE_URL: http://localhost:8080`). `timeout` is how many
seconds to wait for it to start, 120 by default.

The program starts in the background when you switch to the agent; the first
request waits for it. Its answers, thoughts and tool calls appear in the
session like the built-in agent's, and it reads and writes files through
TermIDE, so an open editor follows its edits. Its permission requests are
judged by the same rules as the built-in agent's: a read-only command or a
request a `[ai.permissions]` rule or a session grant already covers passes
without a card, and only what is left reaches you — so a granted or read-only
command is never asked twice. The **Permissions** chip disappears while an external
agent is active: it has its own, and TermIDE's mode is not cycled for it. The **Model** chip stays when the agent advertises
its models over ACP — it then lists them and switches with `session/set_model`,
so you pick the agent's model in TermIDE; agents that advertise none show no
chip. Skills, prompt templates and MCP servers are the agent's own affair too;
`model`, `mode`, `tools` and the body of its `AGENT.md` do not apply. Switching agents
rebuilds the conversation on the same session log: earlier messages stay on
screen but the external agent does not know them, and the panel says so.

### The system prompt

The prompt the model receives is assembled from files: the body of the
default agent's `ai/agents/default/AGENT.md`, a template with placeholders the
agent fills in. No prompt text is built into TermIDE; the template below ships
as a data file (`crates/agent-core/assets/agents/default/AGENT.md`) and is
written to the configuration level on first use, and from then on the file is
what counts. It is the fallback of every agent, so a project's
`.termide/ai/agents/default/` is ignored, and the template changes only when
you pick an agent whose `AGENT.md` has a body. A
project's own conventions go into its `AGENTS.md`, which the template takes in
as project instructions (see below):

```markdown
You are a coding agent working inside termide, an all-in-one terminal workspace (editor, file manager, terminal, git). You help with software tasks in the current project: you read code, make targeted edits, run commands and report what you did and what you found.

# Guidelines
- Read a file before you change it, and keep edits small and targeted.
- Name file paths clearly when you talk about files.
- Be concise.
- Check the facts of the moment with a tool rather than guess them: run `date` for today's date, read a file for its contents, `git log` for history. State plainly when you did not check.
{{guidelines}}

{{if skills}}
# Skills
When a task matches one of these, load it with the `skill` tool before starting.
{{skills}}
{{/if}}

# Environment
{{environment}}

{{project_instructions}}
```

`{{tools}}` is the tool list with a line per tool — the seed leaves it out,
since the model already receives every tool with its description and schema
beside the prompt, but a template for a weaker model may want the overview —
`{{guidelines}}` the rules
the tools themselves contribute, `{{skills}}` the skills by name and
description, `{{environment}}` the working directory, platform and whether it
is a git repository, and `{{project_instructions}}` the instruction files
described below. The prompt carries no date on purpose: a session runs for
hours and the date would quietly rot in it, so the seed instead tells the
model to check a situational fact with a tool — `date` for today's date —
rather than guess it.

`{{if skills}} … {{else}} … {{/if}}` keeps a section out of the prompt when
there is nothing for it: the `{{if}}` branch survives when that name has
something to show, the `{{else}}` branch otherwise, and the whole block
vanishes when you write no `{{else}}`. So a session with no skills never tells
the model about skills, and never invites it to call a `skill` tool that is
not there. Conditions are line tags only — a tag in the middle of a line, or
one inside a filled-in value, stays plain text — and they do not nest. A bare
`{{name}}` outside a block is still just the value, `(none)` when the list is
empty, so a template written before blocks keeps working.

Reword the file, drop a section or add your own; a placeholder you leave out
is simply not sent. `/prompt` opens the assembled result in a viewer, so you
can see exactly what the model gets.

### Service prompts

TermIDE's own prompts are files too, under `system/`, seeded on first use like
the default agent's `AGENT.md`. Only the configuration level's `system/` is read: a project's is
ignored, so a checked-out repository cannot rewrite how termide summarises,
plans or judges. Compaction, the summary that
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
the session approaches the context window, uses the same files. When it
starts and how much it keeps is set in `config.toml`; a key left out keeps its
default:

```toml
[ai.compaction]
enabled = true              # compact automatically near the window
reserve_tokens = 16384      # start when the context passes the window minus this
keep_recent_tokens = 4096   # recent messages kept verbatim (at most a quarter of the window)
```

[Plan mode](#plan-mode) uses `plan.md`: its body is appended to the system
prompt while the mode is on, and `request:` in its front matter is the
message sent when you accept the plan; `clean_request:`, the one sent when
you accept it from a clean context (without it, `request:` serves). Reword the body to change what a plan
must contain, or the request to change how the agent is told to go ahead.
Claude Code takes its system prompt once, when its session starts, so a
switch of plan mode after that reaches it as a note before your next
message, which the session log leaves out: the body when the mode comes on,
`leave:` when it goes off.

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

[Recall](#recall)'s solver uses `recall.md`: the system prompt of the call that
answers from the search results, with the question line in its front matter
(`request:`, `{{question}}` for the queries); the results follow it. Reword it
to change how the answer is drawn or cited.

### Tool texts

What the model is told about each built-in tool is a file too:
`tools/<name>.md` for `read`, `edit`, `write`, `bash`, `question`,
`suggest_command`, `task`, `skill`, `fetch`, `web_search` and `recall`, seeded
and kept current like the `system/` files, and read from the configuration
level only. The body is the description the model sees with the tool;
`snippet:` in the front matter is its line in the system prompt's tool list
(leave it out to keep the tool off the list, still callable), and
`guideline.1:`, `guideline.2:`, … are rules added to the prompt's guidelines,
in their order. In `task.md`, `{{agents}}` is where the agents it can delegate
to are listed. Reword a file to change how the agent uses that tool; the tool
itself, its arguments and what it returns stay the same. An edit reaches a
panel the next time it takes up its agent (a new panel, a switch of agent or of
tools) and every task it hands to a subagent from then on. A file with an empty
body is passed over whole, its front matter too; that, another key, or a file
named after no built-in tool is reported under the banner when a panel opens.

```markdown
---
snippet: read a file as numbered lines, paged with offset/limit
guideline.1: Use `read` instead of `cat`, `head` or `sed -n` to look at files.
---
Read a text file. Returns lines prefixed with their 1-based line number. …
```

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

A skill can take arguments the way a [prompt template](#prompt-templates)
does: `argument-hint` in the front matter says what to pass and is listed
after the name (`- review <path>: Review a file`), the model passes the
arguments along with the name, and `$ARGUMENTS` and `$1`…`$9` in the body are
filled in; a body without placeholders gets the arguments appended on a line
of their own.

You can send a skill yourself too, as `/name args` or `/skill:name args`: its
text, with the arguments filled in and its files listed, goes out as your
request, as a template's does. A skill switched off in the toolset still
works this way — switching it off keeps it out of the model's context, not
out of your reach.

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

`/review src/parser.rs` then sends the expanded text. In the session the
message is headed by what you typed, `/review src/parser.rs`, with the text
the model actually received folded under it; `↑` recalls the command, not the
text. Typing `/` lists the templates, and picking one puts `/name ` into the
input. A message starting with `/` that names no
template is not sent; a path such as `/usr/bin/ls` is plain text.

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

The output appears in the session as your request, folded under the command as
a template's text is, so what the model got is a click away. A script that
exits with an error, prints nothing or exceeds its timeout (60 s by default)
sends nothing and reports why. Scripts from the configuration level are your
own and run at once; one that came with the project or the directory asks
first, in a card like a permission: run once, for this session, always (a rule
`[ai.permissions.command]` is written) or not at all.

Built-in commands, templates, scripts and skills share the `/` names. When
several define one, a built-in command wins, then a template, then a script,
then a skill: a project cannot take over `/clear`, and what was written to be
typed after `/` beats what was written for the model. `/skill:name` always
reaches the skill, and the `/` list offers a skill whose name is taken under
that spelling. Names defined more than once are reported when the panel
opens: as a **shadowed** row on the welcome screen, which explains them when
clicked, or as warnings under a resumed session. The same name at several
levels of one kind is not reported — the higher level hiding the lower is how
levels work.

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

A server is either a program TermIDE starts or a URL it reaches. A program
speaks over stdio; a URL speaks Streamable HTTP, one request per POST, with
its keys in `headers` (`$NAME` in a header value comes from your environment,
as in `env`). Non-`https` URLs are refused except on the loopback, since
these servers carry tokens. Either way the panel connects in the background
when it opens, and reports in the session when it is connected or why it
is not. Its tools appear as `<server>__<tool>` (for example
`github__search_issues`), the shape OpenAI-compatible endpoints accept; they
are not listed in the system prompt, the model sees their schemas directly.
Every schema travels with every request, so a server with dozens of tools is
worth narrowing with `tools`; the log says so when a server has more than
twenty and no such list.

A server may change its tools while it runs. A program says so on its pipe; a
URL server on a stream TermIDE holds open to it after the handshake, opened
again when it drops. Either way the tools are listed again and the session
takes the new set, with a line saying so. Like any change of tools, it costs
the next request its prompt cache.

The configuration is read when the panel opens. `/mcp reload` reads it again,
at every level and from every `.mcp.json`: a server no longer configured
leaves with its tools, a new or changed one connects, one that failed tries
again, and one that is connected and unchanged is left as it is.
`/mcp reload <server>` does it for one server, and connects it again even when
nothing changed — as the `[↻]` on its heading in the toolset list does. `/mcp`
alone lists the servers and where each stands.

A URL server with no `Authorization` among its `headers` that answers `401`
wants an OAuth sign-in, and the session says so. `/mcp login <server>` opens
the browser on the server's authorization page; once you approve, the browser
returns to a one-off listener on `127.0.0.1` and the server connects. The
endpoints come from the server's own metadata, TermIDE registers itself as a
client where the server allows it, and the exchange uses PKCE. The sign-in is
kept in `mcp-auth.json` in the configuration's `ai/` directory, readable by
you only, filed by the server's URL — so every panel and every project that
configures the same URL shares it — and renewed when it lapses. `/mcp logout
<server>` forgets it. When the browser does not open (over SSH, say), the
session shows the address to open by hand; the browser must then reach the
listener, so forward its port and fix it with `callback_port`. A server that
does not register clients needs one registered by hand:

```toml
[jira]
url = "https://mcp.example.com/mcp"
oauth = { client_id = "termide", client_secret = "$JIRA_SECRET", scopes = ["read"], callback_port = 8765 }
```

A `.mcp.json` at the directory the panel works in, at any directory above
it, or at the project root is read beside it, so a repository committed for Claude Code, Cursor or VS Code
brings its servers here unchanged — the same courtesy as reading `CLAUDE.md`
and `.agents/skills/`. There is no standard for this file: the MCP
specification leaves configuration to each client. So it is honoured only as
far as it agrees with termide — `command`, `args`, `env`, `cwd` and `tools`,
or `url` with `headers` and `oauth` (`clientId`, `clientSecret`, `scopes`,
`callbackPort`); under either root key (`mcpServers`, or VS Code's
`servers`); with `disabled` read as `enabled = false` and `${workspaceFolder}`,
`${userHome}` and `${env:NAME}` resolved. An `sse` server is skipped — the
older transport, where requests go to an address a stream names first, is not
the one TermIDE speaks — and so is one
holding a `${input:…}` reference: that asks you for a secret in the other
tool, and termide would start it on an empty one. Every skip is in the log. Within one directory `mcp.toml` wins the
same name; the directory nearer the panel wins both. One `.mcp.json` above a
group of repositories therefore serves every panel opened inside them, the
same way Claude Code reads it.

An MCP tool asks for permission like a command: it runs in `all`, is refused
in `plan`, goes to the reviewer in `auto`, and asks elsewhere unless a rule
covers it in `configured`. "Allow
always" writes a rule for the tool name:

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
template, the default agent's `AGENT.md`, itself. Files over 32 KiB are skipped.

## Session history

Every session is written to a log in JSON Lines, one file per session, under
`ai/sessions/<path of the panel's directory>/` in the TermIDE configuration
directory, beside the agents. The log records the model and the agent the
session started with and every switch, so a reopened session continues on
the model and as the agent it last used. When a session approaches the
model's context window, the agent replaces the older part with a summary it
writes itself and keeps the recent messages verbatim; the panel says when this
happens, and `/compact` does it on request (see
[Service prompts](#service-prompts)). `/undo` and a rewind write a `rewind`
entry: the log keeps every message, but the branch continues from before the
undone request, on reopening too.

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
with a reason the model reads. In the default `auto` mode the reviewer decides
what the rules leave open, and a call it cannot decide is refused; in
`configured`, add `allow` rules for the exact commands and paths the task
needs, or set `mode = "all"` for unattended work. Plan mode has no meaning
without the panel and is treated as `auto`, and an external (`command`) agent cannot be run this
way.

`termide --recall "<query>"` runs the [recall](#recall) search without the UI
and prints the results — the text the agent would read — or, with
`--output json`, one object with the results (and the solver's answer when it
is on). It exits `1` when nothing is found. Another agent can call it through
its shell, and it shows quickly what `recall` finds for a question.

```
termide --recall "why did we drop tokio"
termide --recall "compaction summary" --output json
```
