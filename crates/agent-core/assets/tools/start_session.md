---
snippet: start a session of another agent, or a copy of this one, for part of the work — its answer awaited, brought later, or not at all
guideline.1: Start a fresh session (`agent`, no `context`) for a self-contained piece of work whose answer you need: it keeps the sub-task and the files it touches out of this conversation. Give it everything it needs in `prompt`; it does not see our history.
guideline.2: Fork (`context` `fork`) when the work needs what this conversation already holds; set `focus` to what the copy needs to remember when the conversation is long. Say in `prompt` which part is the fork's and which stays here, so the two sessions do not both take it on.
guideline.3: Set `reply` to `later` when you have other work to do meanwhile, or when you are asked for a background agent; leave it out when your next step needs the answer. An answer that comes later arrives as a message of its own: go on with your work meanwhile, and do not poll for it with tools.
guideline.4: Set `show` to `panel` when the user should follow the session and go on with it themselves — a fresh agent in a panel, or a fork (`reply` `none`) that carries part of the work on with the user; leave it out for work only you need the answer of.
---
Start a new agent session and hand it part of the work. It runs on its own — reading, searching and editing as its permissions allow, asking the user about the rest.
- `context`: `none` (the default) starts a fresh session of `agent`, which knows only `prompt`; `fork` copies this session, compacted on `focus` when given, and runs on this session's own agent.
- `reply`: `wait` (the default out of sight) returns its final answer from this call; `later` returns at once, and the answer comes to you as a message of its own, and `jobs` stops the session by the id the call gave you once you no longer need it; `none` brings nothing back.
- `show`: `background` (the default) runs it out of sight, followed from this panel; `panel` opens it in an agent panel of its own beside this one, where the user follows it and goes on with its session — its answer, if any, comes `later`, and a fork shown there is the user's from its start and reports nothing (`reply` `none`).
`send_message` reaches a session that already exists. Agents a fresh session can run:
{{agents}}
