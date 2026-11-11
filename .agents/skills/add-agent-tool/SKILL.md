---
name: add-agent-tool
description: Add a tool to termide's coding agent end to end — the Tool implementation, its description text, registration, how permissions and plan mode treat it, its transcript headline, its UI strings, the MCP exposure to Claude Code, tests and docs. Use when asked to give the agent a new tool or a new action on an existing one.
---

# Add an agent tool

A tool reaches the model, the permission checks, the transcript and, through
the MCP server, Claude Code. Each has its own table keyed by the tool's name,
and a name missing from one fails quietly: a call that asks when it should
not, a headline with no glyph, a description that is empty. Reference
commits: `f1b4bb90` (`jobs`) and `cb125881` (`fork`).

## 1. The description — `crates/agent-core/assets/tools/<name>.md`

Front matter: `snippet:` (one line, for the system prompt's tool list) and
`guideline.N:` (rules the prompt gives the model about when to use it);
then the description the model reads. Say what it does, what it returns and
what it does not do. Register the file in `crates/agent-core/src/tool_text.rs`
(`("<name>", include_str!("../assets/tools/<name>.md"))`); users can override
the text, so the tool reads it through `ToolText::seed("<name>")`.

## 2. The implementation — `crates/agent-tools/src/<name>.rs`

Implement `termide_agent_core::Tool` (`name`, `description`, `parameters`
as JSON schema, `prompt_snippet`, `prompt_guidelines`, `check`, `execute`),
following a tool of the same kind (`jobs.rs` for one that acts on the
panel, `read.rs` for one on files). What the tool needs from the app comes
in as a trait object (`OwnTasks`, `ForkSession`), not as a dependency on
`termide-app` — crate boundaries go one way. `mod` and `pub use` it in
`crates/agent-tools/src/lib.rs`.

## 3. Registration — `crates/app/src/app/agent_panel.rs`

Insert it where the agent's registry is built. `base_tools` gives every
agent on termide's own loop its tools; the block under `if backend.is_none()`
adds those that need someone watching the panel (`question`, the `suggest_*`
cards, `fork`, `debug`), which a subagent and a headless run do not get.
Pick the place by who may call it. Mind `restrict_tools`: an agent's
`tools:` list removes every tool it does not name, unless the tool comes
with another (as `jobs` with `task`).

## 4. Permissions — `crates/agent-core/src/permissions.rs`

- `not_read_only`: a call that changes nothing returns `None` — plan mode
  lets it through. Otherwise it is refused in plan mode.
- `asks_no_one`: a call no mode asks about (it changes nothing, is put to
  the user anyway, or touches only the agent's own work). One list serves
  the plan guard and every mode; do not add a second.
- `subject_of`: the text rules match and "allow for the session" remembers.
  The fallback takes the first string argument — for a tool with an action
  argument that is usually the action alone, which would let every later
  call of that action through. Give the tool an arm that names what the
  call acts on, and check `suggested_pattern` offers a pattern no wider than
  that.
- Add cases to `permissions/tests.rs` for each mode, plan mode included.

## 5. The transcript — `crates/panel-agent/src/transcript.rs`

Add an arm to the headline match (near `"jobs" =>`): a glyph (`&` for
work with other agents, `$` for terminals, the file glyphs for file tools),
a verb and its subject. The verb is an i18n string `agent_tool_<name>`
(and one per action), in all 15 TOMLs, the `Translation` trait and the
`runtime.rs` key list (see the `add-setting` skill, step 3).

## 6. External agents

A panel on Claude Code is served termide's tools over MCP
(`crates/agent-mcp`, started from `crates/agent-acp`). Codex and Gemini CLI
keep their own tools and get only the `COMPANION_TOOLS` (`crates/agent-core/src/acp.rs`),
those they have no counterpart of: add the tool there if that holds for it.
Check what each agent is offered live rather than by reading — it is easy
to get wrong.

## 7. Docs, changelog, validation

- `doc/en/agent.md` and `doc/ru/agent.md`: the tool under **Tools** (what it
  does, its headline, what it asks in each mode), and in any section whose
  behaviour it changes.
- `changelog.d/added/<kebab-title>.md`.
- Validate:

```
cargo nextest run -p termide-agent-core -p termide-agent-tools -p termide-panel-agent -p termide-app -p termide-i18n
cargo clippy --workspace --all-targets -- -D warnings
```

Then run it live: `tools/tui-harness` with the mock model, a scenario whose
model calls the tool, checking the headline and the permission card.
