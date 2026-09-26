# Agent design notes

Working notes for the built-in coding agent (`crates/agent-*`, `crates/panel-agent`).
Each decision is argued from a comparison of existing agents; nothing is copied
from a single one. Facts marked *(unverified)* come from memory, not from a
checked source, and should be confirmed before they are relied on.

Compared: pi 0.85 (TypeScript), Claude Code 2.1 (TypeScript), Codex CLI 0.153
(Rust), Gemini CLI, OpenCode, Goose (Rust), Aider (Python), jcode (Rust),
DeepSeek Harness (TypeScript), Hermes Agent (Python), and Zed's Agent Client
Protocol (ACP).

## 1. Edit tool

| Agent | Format |
|---|---|
| Claude Code | `old_string`/`new_string`, exact and unique match, `replace_all`; the file must have been read first |
| pi | list of `{oldText, newText}` edits, each unique in the *original* file |
| Anthropic `text_editor`, Goose | `str_replace` with unique `old_str`, plus `insert`, `view`, `create` |
| Gemini CLI | `replace` with `expected_replacements`; a model-based corrector repairs a mismatched `old_string` *(unverified)* |
| OpenCode, Cline | `oldString`/`newString` with tolerant matching: exact, then whitespace-normalised, then indentation-flexible, then block anchors *(unverified)* |
| Aider | `<<<<<<< SEARCH / ======= / >>>>>>> REPLACE` blocks in plain text; its benchmark ranks this format best for most models |
| Codex CLI | `apply_patch` grammar (`*** Begin Patch`, `*** Update File:`, `@@` hunks); OpenAI models are trained on it |

Decision: **search/replace with a unique anchor**, one edit per call, `replace_all`
flag. It is the consensus format and works for every model family; `apply_patch`
is tied to one vendor. Add a **tolerant matcher** (exact, then trailing-whitespace
and indentation tolerant) because local models are the first target and they
misquote whitespace most often. Return a unified diff in `details` for the UI.
Claude Code removed its multi-edit tool; pi kept one. We start with single edits
and revisit if transcripts show many sequential edits to one file.

## 2. Read tool

Claude Code, OpenCode and Anthropic's `text_editor` return numbered lines; pi
returns raw text; Codex reads through the shell. Decision: **numbered lines**
(`cat -n` style), `offset`/`limit`, default cap 2000 lines, byte cap, and an
explicit continuation note when truncated. Numbers give the model stable anchors
for `edit` and for `OpenFileAt` links in the panel.

## 3. Shell tool

Every agent has one. Sandboxing exists natively only in Codex (Seatbelt,
Landlock) and optionally in Claude Code and Gemini CLI. Output limits: pi keeps
the last 2000 lines / 50 KB, Claude Code about 30 000 characters, Codex keeps
head and tail *(unverified)*.

Decision: `bash` with a timeout, cooperative cancel that kills the process
group, **head-and-tail truncation** (the command echo and the final error both
survive) and the full output saved to a file whose path is returned. No sandbox
in the first version; the tool takes its environment from `ToolContext`, so a
sandbox can be added later as a separate module without changing the contract.

## 3a. Subagents

Claude Code: a `Task` tool spawns a subagent of a chosen type, with its own
system prompt, tool allow-list and model; it runs in its own context and
returns a final report, and several can run in parallel. OpenCode: a `task`
tool starts a sub-session as a named agent. Codex, Gemini CLI and pi: none in
the same shape (pi leans on its extension host).

Decision: a `task` tool, present on every built-in-loop agent once a custom
agent exists, that runs one of the other agents to completion and returns its
final message. It reuses the whole machinery — the same agent definitions
(`SOUL.md`, `agent.toml` tools/model/mode), the same prompt builder, the same
permission rules — so a subagent is just an agent run without a panel. Built
in the app (`Subagents` in `agent_panel.rs`), not in `agent-tools`: the tool
there holds only a closure, so the tools crate stays free of the provider and
the catalog. The subagent shares the provider and the rules but has no one to
prompt, so its prompter is `AutoDenyPrompter` — anything the rules and mode do
not already allow is refused with a reason, rather than blocking a user who is
not watching the nested run; this is the deliberate limit that keeps a
subagent from doing more than the parent could without asking. No nesting: the
subagent build path adds no `task` tool, and external (ACP) agents are refused
as delegates, since driving one headlessly has no permission surface. A
runaway is cut after fifty model calls (the emit closure trips a budget
token), and the parent's cancel aborts the sub-run. Not done yet: parallel
delegation (the loop runs one tool at a time) and streaming the sub-run as a
foldable sub-transcript rather than the plain text it reports.

## 3b. Web tools

Most requests that reach outside the checkout are searches, and `curl` fails
them: a search engine answers a script with a block page, a JavaScript page
comes back as an empty shell, and a real page is hundreds of kilobytes of
markup the model has to wade through.

| Agent | Search | Reading a page |
|---|---|---|
| Claude Code | `WebSearch`, run by the provider | `WebFetch`: HTML to markdown, then a small model condenses it *(unverified)* |
| Codex CLI | the provider's `web_search` | none; the shell *(unverified)* |
| Gemini CLI | `google_web_search` through the Gemini API | `web_fetch` *(unverified)* |
| OpenCode | `websearch` through an external API | `webfetch`, HTML to markdown *(unverified)* |
| pi | none built in | none built in |
| Cline | none | `browser_action` on Puppeteer: screenshot and click by coordinates *(unverified)* |

Everyone who searches does it through an API (the model provider's, or a
search service); only Cline drives a browser, and it does so for interaction,
not for search. An API does not work for a user with a local model and no
search key, which is the first target here, so the search goes through a real
browser instead.

Decision: two tools, `web_search` and `fetch`, in a crate of their own
(`crates/agent-web`) so the tools crate stays free of the browser.

- `fetch(url, offset, limit)` loads a page and returns it as markdown
  (headings, paragraphs, lists, code, links made absolute; scripts, styles and
  navigation chrome dropped), headed by the final URL and the title. Text
  types other than HTML come back as they are, binary types are refused.
  Output is paged by line like `read` (2000 lines or 64 KB), and the last
  converted pages are cached so paging does not reload.
- `web_search(query, limit)` returns a numbered list of title, URL and
  snippet. The model reads a result with `fetch`. Two tools rather than one
  `web(query | url)`: the inputs differ, a known URL needs no search, and
  mutually exclusive arguments are a known weak spot of tool schemas across
  providers.

The model sees the same two tools whatever does the work; the backend is a
setting.

```toml
[ai.web]
backend = "auto"         # auto | chrome | http
engine = "duckduckgo"    # an engine file under ai/web/engines/
chrome_path = ""         # empty: look in the usual places
display = "headless"     # headless | minimized | visible
```

- `http` loads pages with `termide-fetch` (GET only, time and size limits, no
  https downgrade). It cannot search, so `web_search` is not registered: the
  model never sees a tool that cannot work.
- `chrome` drives a local Chrome or Chromium over the DevTools protocol.
- `auto` is `chrome` when a browser is found and `http` otherwise.

The browser is spoken to over `--remote-debugging-pipe` (file descriptors 3
and 4, NUL-separated JSON), the way Playwright drives a browser it launched:
no WebSocket, no open port, and no other process on the machine can attach to
the agent's browser. One browser serves every agent of the termide process (calls take turns); it
starts on first use and quits after five idle minutes and on exit. It keeps a
profile of its own in `<config>/ai/web/browser/`, never a copy of the user's: the
agent reads untrusted pages and has a shell, so it must not hold the user's
mail and bank sessions. Nor does it touch the user's keychain: Chrome
encrypts cookies with a key it keeps in the OS keychain, and a fresh profile
asks for access to it (on macOS, with a relocated `HOME`, even to create a new
keychain), so the browser runs with `--use-mock-keychain` on macOS and
`--password-store=basic` on Linux, as Playwright does. Cookies the agent's
profile gathers (a consent page
passed once, a solved captcha) persist between runs. If the profile is in use
by another termide instance, a throwaway profile is used for that run.

Measured first (Chrome 154, macOS, a fresh profile, one query per engine):
headless as it comes, only Bing answered; DuckDuckGo (both endpoints),
Google, Yandex, Brave Search, Startpage and Mojeek answered with a captcha or
a block page. The same browser with a real window got results from
DuckDuckGo and Yandex as well; Google still asked for a captcha, the address
having been flagged by then.

Since version 112, headless Chrome is the same browser as the windowed one,
and comparing what pages see showed how little is left of the difference:
the client hints (`Sec-CH-UA`, `navigator.userAgentData`) already name
"Google Chrome", WebGL reports the real GPU, and `navigator.webdriver` is
true in both (it comes from being driven over DevTools, and DuckDuckGo lets
the window through regardless). What differed was the user-agent string,
`HeadlessChrome/154.0.0.0` for `Chrome/154.0.0.0`, and the screen (800×600,
24-bit, ratio 1, against the display's own). Launched with the windowed
user-agent string, headless got ten results from DuckDuckGo and Bing in two
separate runs, where unchanged it got a captcha and one result; Yandex and
Google could not be judged, both refusing the real window too by then.

So the default display is `headless`, launched with `--user-agent` set to
the string the same browser sends with a window. That string is rebuilt from
the major version (`<binary> --version`), everything else in it being frozen
by Chrome's user-agent reduction, so it never drifts from the binary. The
flag and not `Emulation.setUserAgentOverride`: an override without full
`userAgentMetadata` drops the client hints altogether (checked), which would
be a louder tell than the word it replaces, while the flag leaves them as
they are. Nothing else is altered: no screen or GPU emulation, no scripts
patching the page, no synthetic input; detection beyond that is an arms race
a coding tool should not join, and the answer to a challenge stays the user.

`minimized` is a real window, created in the background and minimized
before the page loads, which leaves the terminal focused (checked: the
frontmost application stayed the terminal throughout). `visible` is for
watching the agent: every page opens in the tab the browser started with,
which is not closed after reading, so the last page stays on screen for a
look or the developer tools; if the user closes it, the next page opens in a
new one that stays in turn. The tab is not brought to the front, so the
terminal keeps the focus.

Whether to watch is the user's call, not the model's, so it is not a tool:
a second tool doing what `fetch` does with a window would cost schema tokens
on every request and invite the model to pick the wrong one, and Playwright
MCP and browser-use likewise make headless a server setting *(unverified)*.
Beside the setting, the **AI** menu has a switch, **Show/Hide browser
window**: it flips a flag on the shared service and relaunches the browser on
a thread of its own, so the window appears or goes at once without blocking
the UI behind a call in progress; the idle shutdown leaves a watched browser
alone. "Show me this page" is a different request, answered in the user's
own browser (`open <url>` through `bash`), not in the agent's. Without a display server the browser runs headless
whatever the setting says.

When an engine answers with a challenge anyway, its window is restored and
brought to the front, the tool reports the wait while it lasts, and the page
is watched until the challenge is gone; after five minutes or on cancel the
search fails. A headless browser has no window to show, so it is closed and
relaunched minimized for that search; the next idle shutdown returns to the
configured display, and what the user solved stays in the profile.

A search page is ready when the engine's result or challenge selectors match,
not when the document finishes loading (Yandex never does); a list rendered
in pieces is read once its length holds for three polls, at most three
seconds. `fetch` waits for the document to load (or for five seconds of
`interactive`), then for its visible text to stop changing, at most three
seconds more. A loaded page with no text at all is an application still
fetching its content (crates.io showed an empty shell of constant size for
about three seconds), so it gets up to ten more seconds to render; measuring
the HTML's size instead took that shell for the page.

An engine is data, not code, so a markup change is fixed by editing a file,
and a user can add an engine (an intranet search, say):

```toml
# ai/web/engines/duckduckgo.toml
name = "DuckDuckGo"
url = "https://html.duckduckgo.com/html/?q={query}"
item = ".result:not(.result--ad)"   # one element per result
title = ".result__a"          # relative to the item; "" is the item itself
link = ".result__a"           # its href
link_param = "uddg"           # the real URL is this query parameter of the href
snippet = ".result__snippet"
challenge = ["#challenge-form", ".anomaly-modal"]   # any match means a captcha
```

Two more keys cover the other engines: `link_param_prefix` and
`link_param_base64` decode Bing's `u=a1<base64>` redirects, and `exclude`
drops results whose URL contains a fragment (Yandex's ad redirects).
`challenge_url` matches the address of a challenge page (Google's `/sorry/`).

The selectors run in the page (`querySelectorAll` in a `Runtime.evaluate`),
so no CSS engine is linked in. DuckDuckGo, Google, Bing and Yandex ship as
seeds under `<config>/ai/web/engines/`, reconciled like the other seeds
(`.seeds.toml`), and a same-named file at a higher level overrides one below.
DuckDuckGo is the default: its HTML endpoint needs no JavaScript and answered
a windowed browser at once. The DuckDuckGo, Bing and Yandex selectors were
checked against live results; Google's could not be, since it answered every
probe with a captcha, and are marked so in the file.

Permissions: both tools ask by default. The subject of `fetch` is the URL and
its suggested rule `https://host/*`; the subject of `web_search` is the query
and its suggested rule `*`. Both count as read-only, so plan mode lets them
through. Nothing blocks private addresses: the shell can reach them anyway,
and the prompt is the gate.

Not done: the `chrome` backend on Windows (passing descriptors 3 and 4 to a
child needs the CRT's inherited-handle block; `auto` falls back to `http`
there), API backends (the provider's own search, SearXNG, Brave), and
interaction with a page (clicks, forms), which Playwright MCP already covers
through the MCP client; if it is ever built in, the shape to follow is an
accessibility snapshot with element references and Playwright's
actionability waits, not screenshots and coordinates.

## 4. Permissions

| Agent | Model |
|---|---|
| Claude Code | modes (`default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions`, `dontAsk`) plus `allow`/`ask`/`deny` rules such as `Bash(git commit *)`, evaluated deny → ask → allow; "always allow" is persisted to `.claude/settings.local.json` |
| Codex CLI | approval policy (`untrusted`, `on-failure`, `on-request`, `never`) × sandbox mode; answers `Approved`, `ApprovedForSession`, `Denied`, `Abort` *(unverified)* |
| OpenCode | `permission` config: `edit`, `webfetch`, and `bash` pattern → `allow`/`ask`/`deny` *(unverified)* |
| Gemini CLI | `default`, `auto_edit`, `yolo` plus a policy engine *(unverified)* |
| Goose | `auto`, `approve`, `smart_approve`, `chat` *(unverified)* |
| pi | none; a `tool_call` hook in an extension may block |
| Hermes | allow-list patterns, approval for dangerous commands |
| ACP | `session/request_permission` with `allow_once`, `allow_always`, `reject_once`, `reject_always` options |

Decision: **rules plus a mode**. Rules are `deny`, `ask`, `allow` lists keyed by
tool and an argument pattern (`bash: "git push *"`, `edit: "src/**"`), evaluated
deny → ask → allow. The mode decides which rules count and what unresolved
calls do: `ask` (everything asks, configured `allow` rules set aside), `plan`
(reads and the web pass, changes are refused), `edit` (edits inside the
project and the web pass, commands ask), `configured` (default: the rules
decide, the rest asks) and `all` (everything passes). A rule's `deny` and
`ask` hold in every mode, so a stricter mode never protects less; answers
given for the session count everywhere but in `all`, and "allow always" is
offered only in `configured`, the one mode the configured rules count in.
Prompt answers: allow once, allow for the session, allow always in the project
or everywhere (a rule in the project `.termide` or the global config), deny,
deny for the session. This is the Claude Code / OpenCode shape with ACP's
answer set; it stays small and is data-driven, so the panel and a future ACP
client render the same prompt.

Chosen TOML shape (OpenCode-style tables, so "allow always" appends one key):

```toml
[ai.permissions]
mode = "configured" # ask | plan | edit | configured | all

[ai.permissions.bash]
"git status*" = "allow"
"git push*"   = "ask"
"rm -rf *"    = "deny"

[ai.permissions.edit]
"src/**" = "allow"
".env"   = "deny"
```

Implementation (`crates/agent-core/src/permissions.rs`): `*` spans any text
including slashes, a leading `**/` is optional. Subjects are the command for
`bash` and the project-relative path for file tools. Shell commands are split
on `&&`, `||`, `;`, `|` and newlines with quote awareness, as Claude Code
does: every part needs its own allow, a deny or ask on any part wins, and
command substitution is never auto-allowed. A built-in read-only command list
(`ls`, `cat`, `rg`, `git status`, `find` without `-delete`/`-exec`, ...)
skips the prompt in every mode, the way Claude Code's read-only Bash set and
Codex's `is_safe_command` do; redirections disqualify. Reads inside the
project never prompt. Suggested rule for "allow always": `git push *` style
for shell (first two words for `git`, `cargo`, `npm`, ...), the exact path for
files.

## 4a. Plan mode

Claude Code: `plan` is one of the permission modes in the `Shift+Tab` cycle;
it allows read-only tools only, injects plan-mode instructions into the
turn, and ends with an `ExitPlanMode` tool call that asks the user to
approve, offering "auto-accept edits", "manually approve" or "keep planning";
the plan is a file. Codex has a plan mode too *(unverified)*, Gemini CLI a
read-only `plan` approval mode writing the plan under `.gemini/plans`,
OpenCode a separate `plan` agent with edit/write/bash denied, switched with
`Tab`. pi has none.

Decision: a fourth permission mode, `plan`, not a separate agent — the mode
is already the thing the user flips mid-run, and an agent definition can
still fix it (`mode = "plan"` in `agent.toml`, which gives OpenCode's plan
agent for free). The guard is `PlanGuard` in `permissions.rs`, first in the
hook chain, so it also overrides a command hook's `allow`: in plan mode only
`read`, `skill` and a shell command made of look-only parts without
substitution pass; everything else is blocked with a fixed reason the model
reads. Instructions come from `ai/system/plan.md`, appended to the system
prompt while the mode is on (the panel updates the worker's prompt on the
toggle, or at the end of the run if one is in flight) — a prompt suffix
rather than Claude Code's per-turn reminder, so the log holds only the user's
words. No exit tool: when the run ends in plan mode with an answer, the panel
shows a `ChoiceForm` — carry out accepting edits, carry out asking, keep
planning — and accepting switches the mode and sends the `request:` from the
same file, so the plan stays in context. The plan is the answer in the
session, not a file: the session log is the record, and the panel's `/undo`
checkpoints cover the changes the accepted plan then makes. Not covered: MCP tools with
a read-only annotation are blocked too, since annotations are not plumbed.

## 5. Hooks and extension mechanism

| Mechanism | Who uses it |
|---|---|
| MCP servers for tools | Claude Code, Codex, Gemini, Goose, OpenCode, Hermes, jcode |
| `SKILL.md` skills (agentskills.io) | Claude Code, Codex, pi, Hermes, OpenCode |
| Context files (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`) | all |
| External command hooks, JSON on stdin/stdout, exit-code semantics | Claude Code (also `http`, `prompt`, `agent` hook types), Codex `hooks.json` *(unverified)* |
| In-process scripting | pi (TypeScript via jiti), OpenCode (TypeScript plugins), DeepSeek Harness (everything is a Cordis plugin) |

Every Rust agent (Codex, Goose, jcode) extends through MCP and data files, none
embeds a scripting language. Decision: **levels 0–2 only** for now: data files,
external processes (MCP for tools, command hooks), and Rust traits. Embedded
Lua stays a documented option, not a plan.

Command hooks (`crates/agent-hooks`, configured in `ai/hooks.toml` at the
three levels): the JSON-on-stdin, JSON-on-stdout, exit-code-2-blocks protocol
that Claude Code, Gemini CLI and Cursor share, reduced to two events —
`before_tool_call` and `after_tool_call`, the two extension points the
`Hooks` trait already had. A before-hook answers `decision` (`block`,
`allow`, `ask`) and optionally `arguments`; `allow` maps to a new
`ToolDecision::Approve`, which runs the call and skips the remaining hooks,
including the permission rules — the way Claude Code's `permissionDecision:
allow` bypasses its prompt — so a policy script can stand in for the user.
Hooks compose through `ChainedHooks` in agent-core: command hooks first, the
rules last, a `Replace` from one hook being what the next one judges. Any
failure other than exit 2 is logged and ignored: a hook is a guard, not a
dependency, and a broken one must not stop the agent. Cursor's per-event
names (`beforeShellExecution`) and Claude Code's `Stop`/`SessionStart` events
are left out until something needs them.

## 5a. Instruction files and system prompt

`AGENTS.md` is the cross-vendor convention (Codex, pi, Gemini CLI, Cursor,
Zed, Amp); Claude Code reads `CLAUDE.md`. pi and Claude Code walk every
ancestor of the working directory from the root down, Codex starts at the
repository root; Codex caps a file at 32 KiB, Claude Code at 4 MiB. Claude
Code and Gemini support `@file` imports, the others do not.

Decision (`crates/agent-core/src/context.rs`): the project root's file when
the panel works outside the project, then per ancestor from the root down to
the working directory
`AGENTS.md`, falling back to `CLAUDE.md` in the same directory; 32 KiB cap; no
imports. Files are appended under a `# Project instructions` heading with
their path as a sub-heading (Markdown, not pi's XML wrapper, because small
local models follow Markdown more reliably).

Where the agent's own files live:

| Agent | Directory | Contents |
|---|---|---|
| Claude Code | `~/.claude/`, `.claude/` | `CLAUDE.md`, `agents/*.md` (body = system prompt), `skills/*/SKILL.md`, `commands/*.md` |
| Codex CLI | `~/.codex/` | `config.toml`, `AGENTS.md`, `prompts/*.md`, `skills/` |
| pi | `~/.pi/agent/`, `.pi/` | `AGENTS.md`, `SYSTEM.md`, `APPEND_SYSTEM.md`, `skills/`, `prompts/`, `extensions/` |
| OpenCode | `~/.config/opencode/`, `.opencode/` | `AGENTS.md`, `agent/*.md`, `command/*.md` |
| cross-agent | `.agents/` | `skills/*/SKILL.md` (agentskills.io) |

Decision (`crates/agent-core/src/layers.rs`): three roots — `.termide/ai/`
in the panel's working directory, the same in the termide project root, and
`ai/` in the configuration directory — highest first. A single file comes
from the first root that has it; a directory of named entries is the union
with higher names hiding lower ones, which is how agents, skills and prompts
will merge. Session logs go to `<config>/ai/sessions/<panel directory>/`,
keyed by the directory the panel works in: the user's decision, the shape pi
and Claude Code use (sessions under the tool's own directory), taken over the
XDG data/config split. The configuration level is laid out on first use —
`AGENTS.md` seeded from the shipped data file, empty `agents/`, `skills/` and
`prompts/` — and files present are never touched again.

Agent definitions: Claude Code's `agents/*.md` and OpenCode's `agent/*.md`
carry the settings (`description`, `model`, `tools`, `permissionMode` /
`permission`) as YAML front matter above the prompt body. Decision: the
prompt stays a plain Markdown file (`SOUL.md`) and the settings go beside it
in `agent.toml` — termide is TOML throughout, and a prompt without front
matter can be copied from and to any other tool. `default` exists without
files; switching agents goes through an `AgentCatalog` trait the app
implements over the directories, so the panel chooses among definitions
without knowing how they are stored. The switch is one `AgentRuntime::update`
closure applied between runs (prompt, tools, model); the mode goes through
the shared `ModeHandle`. Model and mode change only when the definition names
them, so a user's runtime choice survives switching to an agent that has no
opinion. The session log records the agent as it records the model
(`agent_change` entries, one at the start), so a reopened session comes back
with the prompt and tools it ran with.

Skills (`skills/<name>/SKILL.md`, agentskills.io front matter) are a
cross-agent format; how they reach the model differs:

| Agent | In the prompt | Loading the body |
|---|---|---|
| Claude Code | names and descriptions | a `Skill` tool pulls the body into the context |
| Codex CLI | names, descriptions and paths | the model reads the file with its read tool |
| pi | names, descriptions and paths | the same, through `read` |
| OpenCode | explicitly enabled skills in full | none |

Decision: names and descriptions under `{{skills}}`, one line each, and a
`skill` tool that takes the name. Per request the two options cost the same
— a list line per skill, cached with the prompt prefix — and the tool's
schema adds a few dozen tokens, also cached. The difference is on the load:
a name is two or three tokens and an `enum` in the schema, where a path is
thirty and a thing a local model mistypes; the body comes back verbatim,
without `read`'s line-number prefixes (a few tokens per line), together with
the skill's companion files, which `read` would need a second call to
discover; and a skill in the configuration directory lies outside the
project, where `read` would have to ask permission — `skill` never does.
Skills are found in each level's `ai/skills` and in `.agents/skills`, the
shared directory, so nothing has to be copied to work with termide. The tool
exists only when a skill does, and an agent's `tools` list does not remove
it: skills are instructions, not a capability.

Prompt templates: Claude Code (`commands/*.md`), Codex (`prompts/*.md`), pi
(`prompts/*.md`) and OpenCode (`command/*.md`) agree on a Markdown file per
`/name` with `description` and `argument-hint` front matter and `$ARGUMENTS`
/ `$1`…`$9` in the body; Claude Code and OpenCode also splice shell output
(`!`cmd``) and files (`@path`) into the template. Decision: the shared
shape under `prompts/<name>.md`, merged across the three levels, without
shell or file splicing — that is a second way to run commands outside the
permission model, and a template can ask the agent to run them instead. The
expanded text is what the transcript and the log show, since it is what the
model received. A leading `/` is a command only when the first word is a
plain name; an unknown name is refused rather than sent, so a typo does not
reach the model.

MCP:

| | Claude Code | Codex | OpenCode | termide |
|---|---|---|---|---|
| Transport | stdio, SSE, HTTP | stdio, HTTP | stdio, HTTP | stdio |
| Taken from a server | tools, prompts, resources | tools | tools | tools |
| Configuration | `.mcp.json`, `~/.claude.json` | `[mcp_servers.<name>]` in `config.toml` | `opencode.json` | `ai/mcp.toml` at the three levels |
| Tool names | `mcp__server__tool` | `server/tool` | `server_tool` | `server__tool` |
| Permissions | allow-list of `mcp__*` | per-server approval | per-tool | the per-tool rules, `ask` by default |

Decisions (`crates/agent-mcp`): stdio only, because the servers that matter
for a local coding agent run locally and it spares the crate OAuth and HTTP
plumbing; tools only, since prompts are ours and resources are files the
model reads anyway. Names use `__` because OpenAI-compatible endpoints accept
only `[A-Za-z0-9_-]` in a function name, which rules out `/` and `.`.
Connecting happens on a thread per server and the tools arrive as
`LateTools` through a subscription: an `npx` server takes seconds to come
up and the panel must not wait for it; the panel hands them to the worker
between runs with `AgentRuntime::update`. MCP tools carry no prompt snippet,
their schemas already reach the model, and a `tools` filter plus a warning
past twenty tools keep the per-request cost visible — the same reasoning as
for skills. The client is blocking JSON-RPC over pipes with a reader thread,
no tokio, like the rest of the agent; a server's own requests (roots,
sampling) are declined with -32601.

Command scripts (`ai/commands/<name>`, executables): the user's answer to
Claude Code's and OpenCode's `!`cmd`` splicing and Gemini CLI's `!{cmd}`,
which we had declined because a template that runs commands runs them
outside the permission model. A whole executable instead of inline shell
keeps `prompts/` static text, makes the command a file one can run by hand,
and lets the panel gate it: a script from the configuration level is the
user's own and runs unasked, one from the project or the directory asks in
the same card as a permission, with "run always" written as a rule under the
tool name `command`. The script's stdout is sent as the user's request, so
the transcript shows what the model got; a non-zero exit, silence or a
timeout sends nothing. `# description:`, `# argument-hint:` and `# timeout:`
in the header feed the picker, and the built-in `/compact`, the templates
and the scripts share one `/` namespace with the template winning a tie.

The prompt is a template with `{{tools}}`, `{{guidelines}}`,
`{{environment}}` and `{{project_instructions}}` placeholders: the `ai`
directory's root `AGENTS.md` for the default agent (the user's decision — the
root file of the directory is the default prompt, and a global instruction
file would only duplicate what one can write into it), `agents/<name>/SOUL.md`
for a custom agent, which falls back to the root file when it has none. No
prompt text is code: the seed is the data file
`crates/agent-core/assets/AGENTS.md` (the former fixed prompt, base
guidelines included), copied to the configuration on first use; code only
fills the placeholders from tool metadata, the environment and the instruction
files. A file replaces the template whole rather than layering
`identity`/`append` overrides, so what the user reads is what the model gets;
the panel's **Show system prompt** writes the assembled text next to the
session logs and opens it. The name is termide's own: only pi calls the file
`SYSTEM.md`, Claude Code and OpenCode keep the prompt in the agent's Markdown
body, and Codex has no such file, so there is no convention to follow.

## 6. Sessions

pi: JSONL tree with `parentId`. Claude Code: JSONL with `parentUuid`,
sidechains, per-block assistant records, file-history snapshots. Codex: rollout
JSONL with typed items *(unverified)*. OpenCode, Goose, Hermes: SQLite.

Decision: **append-only JSONL, one file per session** under the termide data
dir, every entry with `id` and `parent_id` so branching needs no migration.
Search across sessions is out of scope; if it comes, termide's `db` crate exists.

## 6a. Compaction

pi compacts when the context exceeds `window - reserve`, inside the run,
keeping a `retainedTail` of recent messages verbatim and retrying after an
overflow error. Claude Code compacts near the window with a summary that
preserves requests, decisions, files, errors and pending work, then re-reads
recently touched files. Codex has `model_auto_compact_token_limit` and a
summary prompt.

Decision (`crates/agent-core/src/compaction.rs`): threshold check before every
model call using the last reported usage plus a characters-over-four estimate
for later messages; the reserve (default 16 K tokens) is capped at a quarter of
the window so short-context local models still work. The summary is produced
by the same model with a fixed six-point prompt (Claude Code's list); the most
recent messages within a token budget (`keep_recent_tokens`, default 4 K,
capped at a quarter of the window) stay verbatim, as pi's retained tail does,
and a tool call is never split from its results. A summary shorter than 40
characters is rejected as degenerate and the transcript stays untouched: a
live run showed a small model answering `{}` when asked to summarise a lone
prompt. The summary message ends with an instruction to continue the task. An
overflow error from the provider triggers one compaction and one retry. The transcript becomes `[summary user message] +
tail`; the session log records a `compaction` entry with `keep_last`, so
reopening rebuilds the same context. Re-reading touched files is left to the
model.

Compaction prompts as files: no other agent lets the user edit the summary
prompt — Claude Code and Codex build it in and take a focus from `/compact`,
OpenCode compiles a `summarize.txt` in, pi lets an extension replace the whole
step. Decision: the same rule as for the system prompt — `ai/system/compact.md`
(instructions plus the closing `request:` in front matter, `{{focus}}` for
`/compact`'s words) and `ai/system/compacted.md` (the wrapper with
`{{summary}}`), seeded from `assets/system/` on first use and read through the
three levels; `CompactionPrompts` carries them, and a reopened session words
its old summaries with the current file. `system/` rather than `prompts/`
because a slash template is something the user sends and a service prompt is
not, and rather than `tools/` because compaction is the panel's operation, not
a tool the model calls; `tools/` stays free for overriding built-in tool
descriptions if that is ever wanted. `/compact` is a built-in slash command
next to the templates, a `Compact` worker command between runs.

## 6b. Checkpoints and undo

Claude Code snapshots every file before a tool changes it and offers
`/rewind` with three choices: conversation only, code only, or both; its
snapshots are copies under the session, independent of git, and shell
commands are explicitly not covered. OpenCode's `/undo` and `/redo` are git
snapshots of the whole worktree (a hidden git dir, so also for ignored files)
per message, with the messages reverted alongside; Codex, Gemini CLI and pi
have nothing and point at git.

Decision (`crates/agent-core/src/checkpoints.rs`): Claude Code's shape,
because copying the two or three files a request touches is cheaper and more
predictable than a git snapshot of a large tree on every request, needs no git
and works in `target/` or any ignored path. `CheckpointHooks` sits first in
the hook chain and copies the target of `edit`/`write` into
`<session_dir>/checkpoints/<session>/<n>/` with a manifest; a target that does
not exist is recorded as created and removed on undo. The panel calls
`begin_run` with the session's leaf when a request starts and `end_run` at
`AgentEnd`; a run that touched nothing leaves no folder. `/undo` (also in the
menu) shows a `ChoiceForm` naming the files, restores them, appends a
`rewind` entry to the session log whose parent is the leaf before the request
and rebuilds the runtime from the new leaf, emitting `FileChangedOnDisk` per
file so editors reload. One kind of undo, not three: the conversation-only
variant is `↑` recall plus a new request, and code-only undo leaves the agent
believing the edit is in place, which misleads the next turn. No redo: the
messages stay in the log on the dead branch, but the files' newer content is
not kept. Shell commands are not covered, as in Claude Code; the doc says so.

## 6c. The panel

Layout follows pi, Claude Code and Codex: transcript above, multi-line input
below. `Enter` sends, `Shift+Enter` adds a line, `Esc` aborts a run and then
clears the input. A message typed while the agent works is queued as a
steering message rather than starting a second run.

The transcript is a stack of foldable blocks. The user asked for this shape
against auto-spawning a terminal panel per command (dozens of subagent panels
would flicker) and against a chat that scrolls away under noise: only the
agent's answer is unfolded by default, and every other block shows a preview —
a user message its first lines, a tool call its command plus the tail of its
output, thinking a one-line summary. The answer of an assistant block always
shows; folding it only hides its thinking, so the read never loses the point.
A block unfolds on click, on `Space`/`Enter` when the chat cursor is on it,
or all at once with `Ctrl+O`; `o` opens the selected block in its own
read-only panel (`PanelEvent::ViewFile`, reusing a tool's saved raw log or a
temporary file) for a full view without a panel per command. `Tab` moves focus between the input and the
chat; in the chat `↑`/`↓` move the cursor (a tinted row) and other keys are
swallowed so they do not type into the unfocused input. Fold state lives in
the `Transcript` (`collapsed` parallel to `items`), not in the item, so the
same key toggles any kind; folding by default is a setting
(`[ai].fold_blocks`: `immediately`, `on-finish` or `never`) surfaced in the settings
modal's Agent section alongside every other agent setting, and the block
labels are localised through `crates/i18n`; the transcript renders through `crates/richtext`
so answers get real Markdown with syntax-highlighted code, and lines are
cached per item, so a streaming token re-renders one message rather than the
whole history.

Command output is cleaned for the model, not just truncated (`clean.rs` in
`crates/agent-tools`): a real terminal makes tools emit colour, cursor moves
and progress redraws that are pure noise in the context, so escapes are
stripped, carriage-return redraws resolved to their final line, three-plus
identical lines collapsed to `(×N)`, blank runs squeezed, and command-aware
rules (a data table — cargo, pip, npm, go) fold a run of progress lines to a
count. It is conservative — a warning or error line is never dropped — and
runs before the head/tail truncation; the user's live view and the on-disk
log stay raw. Modelled on rtk (`rtk-ai/rtk`), which proxies noisy CLIs to
save tokens; the rule table is the extension point.

Permission prompts are a card inside the panel (`ChoiceForm` in
`crates/ui`), between the separator and the input, answered with the arrows,
a digit, a click or `Esc`. They started as termide's selection modal, on the
argument that a modal cannot be missed; the user's counter-argument won: with
several panels open a modal does not say who is asking, so a question a panel
raises on its own belongs in that panel, and modals stay for choices the user
starts (the model, agent and session pickers). The status line announces the
question for a panel out of focus. Crossing the thread boundary needs care —
the agent thread blocks inside `before_tool_call` while the UI thread owns
the form — so the prompter sends the request over a channel and waits with a
timeout, checking the shared `CancelToken` so an aborted run never hangs on
an unanswered prompt. The `/command` completion is the sibling widget,
`CompletionList`: a list owning its selection and keys, anchored above any
input, so the editor or the terminal can complete paths or symbols with the
same piece later. It also drives `@file` mentions in the same input: a second
trigger that lists files under the panel's directory (a budgeted walk that
skips `.git`, `target` and the like) and replaces the `@token` with the path,
a directory keeping the `@` so the list reopens for its contents. `@` is only
quick path entry — the agent still reads the named file with its tool, so
nothing enters the context unseen; Claude Code and Gemini attach the file's
contents on `@`, which is heavier and less transparent.

The title is the session's name when it has one, else the first prompt,
else the working directory, so stacked agent panels stay apart; renaming goes
through the panel's `[≡]` menu and appends a `session_name` entry to the log,
the way pi's `/rename` and Claude Code's `-n` name a session. That prompt
needed the input twin of the selection round-trip:
`InputAction::Custom` → `PanelCommand::InputSubmitted`.

Resuming: the panel keeps everything its agent was built from (provider,
tools, model, prompt, rules, compaction policy), so switching sessions is a
rebuild rather than a new panel; `persist_rule` is a plain `fn` pointer rather
than a closure so it survives that rebuild. The `[≡]` menu offers **New
session** and **Open session** (a picker over `Session::list`, newest first,
the current one marked), mirroring pi's `/resume` and Claude Code's
`--resume`. A switch is refused while a run is in flight: simpler than
draining the old worker, and it never leaves a half-finished turn in a log.

Session logs live in `<config>/ai/sessions/<panel directory>/`; the
picker lists the sessions of the directory the panel works in.

Switching model and mode at runtime:

| Agent | Model | Permission mode |
|---|---|---|
| Claude Code | `/model` picker over a fixed list plus a typed id | `Shift+Tab` cycles default → accept-edits → plan; not persisted |
| Codex CLI | `/model` picker | `/approvals` picker |
| pi | `/model` picker over its registry, `Ctrl+P` cycles | none (no modes) |
| OpenCode | `Ctrl+X M` list from models.dev | `Tab` toggles the build/plan agents |
| Aider | `/model <id>` typed | none |

Decisions: the two status chips are buttons, mirrored in the `[≡]` menu, and
`Shift+Tab` cycles the mode as in Claude Code. The model list comes from the
endpoint's own `GET /models` (every OpenAI-compatible server answers it),
fetched on a helper thread and shown when it arrives, with a typed-id entry
last and as the whole picker when the endpoint cannot list; a config-side
model list would be a second place to keep in sync with the server. vLLM and
omlx put `max_model_len` on each entry, and the panel takes it as the context
window of the model it switches to, since the configured figure belongs to
the configured model. The mode
is a `ModeHandle` — an atomic shared with the hooks on the agent thread, the
way `CancelToken` is — so a switch applies to the next tool call of a run in
flight; the model goes to the worker as a `SetModel` command and is refused
while a run is active, since the worker reads commands only between runs. A
switch is recorded as a `model_change` entry in the session log and a new
session records its starting model, so resume continues on the session's model
(pi's behaviour) rather than the config's. Neither switch is written to the
config: the chips are per-panel state, as Claude Code's `Shift+Tab` is.

Edits and open editors: VS Code, Zed and JetBrains reload a clean buffer
when its file changes on disk and keep the cursor; a dirty buffer keeps its
work and shows a conflict. termide's editor now does the same for every
on-disk change, so the agent needs no special path into the editor. It only
speeds the reload up: a successful `edit` or `write` result carries the path
in its details, and the panel raises `PanelEvent::FileChangedOnDisk`, which
the app fans out like a watcher batch — at once, and also for paths the
watcher drops under `.gitignore`.

Persisted in a termide project layout as `PanelState::Agent { cwd, session }`:
the working directory and the path of the session log. That is enough because
the log carries the model and the rest (endpoint, rules, prompt) is
configuration, which the layout-restore constructor now receives as
`AgentSettings` alongside the editor config. Zed's ACP threads and JetBrains'
tool windows restore the same way — a reference to the conversation, not its
content — and Claude Code's `--continue` is the CLI shape of it. A missing log
starts a fresh session in the same project; no configured model skips the
panel, as an unavailable image backend skips an image panel.

## 7. Panel ↔ agent boundary

ACP is becoming the editor-side standard: Gemini CLI speaks it natively, Claude
Code and Codex have adapters, Zed, Neovim and JetBrains are clients. Decision:
shape the panel's contract after ACP (`prompt`, `session/update` with
`tool_call` status `pending`/`in_progress`/`completed`/`failed`,
`request_permission` with the four answers). The built-in agent is the first
backend; an ACP client over stdio is the second and reuses the same panel.

Done (`crates/agent-acp`): the panel drives a `Backend` trait (agent-core)
with two implementations, `AgentRuntime` and `AcpRuntime`. termide is the
ACP client: it starts the agent from an `[acp]` table in `agent.toml`, runs
`initialize` and `session/new` on a thread (adapters started through `npx`
take seconds), and turns `session/update` into the loop's own `AgentEvent`s
— `agent_message_chunk` into `MessageStart`/`TextDelta`, a `tool_call`
closing the streamed text as a `ToolUse` message before `ToolExecutionStart`,
its kind standing as the tool name so an `edit` reloads the editor. The
agent's requests are served in the client: `session/request_permission`
through the same `ChannelPrompter` as the built-in prompt (answers mapped
onto the offered `allow_once`/`allow_always`/`reject_once` options),
`fs/read_text_file` and `fs/write_text_file` from the working directory (a
write surfaces as a `write` tool call for the editor reload), terminals not
advertised. Model and mode chips are hidden for an external agent —
`Backend::update` answers `Unsupported` — and switching between engines
rebuilds the runtime on the same session log, whose history is shown but not
known to the external agent (ACP's `session/load` is the way to change that
later). Still hand-written JSON-RPC rather than the `agent-client-protocol`
crate: the surface used is small, and the crate would bring tokio.

## 8. Provider

One wire format first: OpenAI-compatible streaming chat completions
(`crates/agent-providers`), which covers llama.cpp, Ollama, vLLM, omlx,
OpenRouter and most gateways. Vendor differences are data in `Compat`
(`max_tokens_field`, `reasoning_effort`, `send_reasoning`, `extra_body`), the
way pi's per-model `compat` table works, instead of one code path per vendor.

Reasoning between turns: Anthropic requires thinking blocks to be echoed with
their signature; DeepSeek rejects an echoed `reasoning_content`; vLLM and Qwen
accept it for the current turn. Decision: keep thinking in the transcript for
the UI and the session, do not send it back by default; `send_reasoning` opts
in for OpenAI, and the Anthropic provider drops prior thinking blocks rather
than replay them, since the transcript does not keep the block signature the
API demands. Extended thinking for the current turn is requested with a
`thinking` budget derived from the thinking level and capped below
`max_tokens`.

Retries: pi retries at the session level (3 attempts, 2 s base), Claude Code and
Codex inside the client. Decision: inside the provider, only while no content
has arrived (a half-streamed answer is returned as an error, not replayed),
exponential backoff, surfaced as `StreamEvent::Retry` so the panel can show
the wait. Transient: transport errors, 408, 409, 425, 429, 5xx.

Verified against the local omlx server (Qwen3.8 27B): `reasoning_content`
deltas, keepalive chunks with empty content, `tool_calls` deltas with an index
and complete arguments in one chunk, `finish_reason: "tool_calls"`, `[DONE]`.
That transcript is a replay test in `sse.rs`.

## 9. Loop

Kept from the first increment (`crates/agent-core`): one turn = one assistant
message plus its tool batch; the provider stream never fails; steering at turn
boundaries, follow-up when the agent would stop; `before_tool_call` may block.
Added after the comparison: an `after_tool_call` hook (Claude Code
`PostToolUse`, pi `afterToolCall`) so an external hook can rewrite a result, and
an `updated_input` field on allow so a hook can rewrite arguments.

## 10. Headless mode

Claude Code has `-p/--print` (one prompt, prints the result, `--output-format`
json/stream-json) and Codex `exec`; both run the same agent without the TUI
for scripts and CI. Gemini CLI and pi have similar non-interactive paths.

Decision: `termide --prompt "<prompt>"`, an early-exit CLI branch beside
`--diagnostics`, before the terminal is touched, so stdout stays plain
(`run_agent_headless` in `agent_panel.rs`). It reuses the whole stack — the
provider (OpenAI or Anthropic), the agent definitions, the tools, the prompt
builder and the permission rules — so the headless agent is the panel's agent
without the panel, the way the subagent runner is. Streaming: text deltas to
stdout so the answer pipes cleanly, tool activity and errors to stderr, one
tool line per call. Permissions: no one to prompt, so `AutoDenyPrompter` as in
a subagent — the run does only what the rules and mode already allow, and
`mode = "auto"` or `allow` rules opt into more; plan mode collapses to ask,
and an ACP agent is refused (no headless permission surface, as for
subagents). `-` reads the prompt from stdin. Exit code: 0, 1 on a failed
message, 130 on abort. `--output json` prints one object instead of streaming
— answer, stop reason, model, provider, usage and the tool calls — for a
consumer that parses rather than reads, and `--output stream-json` prints one
object per line as the run unfolds (`tool_use`/`tool_result`/`message`, then a
`result`), for one that follows it live. Not done yet: `--agent` delegation to
the `task` tool (headless carries the built-in tools and skills only).
