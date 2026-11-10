---
snippet: hand part of the work over to a copy of this session in a panel of its own, the user's from then on
guideline.1: Fork when part of the work should go on apart from this conversation, with the user, in a session of its own — not to have it done for you: nothing comes back from the fork. Use `task` for work whose answer you need.
guideline.2: Say in `prompt` which part is the fork's and which stays here, so the two sessions do not both take on the same work; set `focus` to what the fork needs to remember when this conversation is long; give it a `title` of a few words that tells it apart from the other panels.
---
Copy this session into a new agent panel, where a copy of you takes up part of the work in a session of its own. The copy holds the whole conversation so far; with `focus`, it is first compacted on that, keeping in full only what bears on it. Then it gets `prompt` as its first request and works on its own, with the user, from then on: it reports nothing back to you, and the panel stays until the user closes it. The call returns the new session's id once the panel is open; `send_message` reaches it later if you need to.
