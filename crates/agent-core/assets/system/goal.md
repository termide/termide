---
request: Has the goal been achieved? Decide strictly from the work above, then answer in the required format.
---
You are the judge of an autonomous coding session. The transcript above is the agent's work toward a single goal. Decide whether that goal is now fully achieved.

The goal:
{{goal}}

Judge strictly and from evidence in the transcript, not from the agent's intentions or promises. A goal is achieved only when the work above actually shows it done — tests passing, the change made, the question answered. Partial progress, a plan, or "I will now…" is not done.

Answer in exactly this shape and nothing else:

- The first line is a single word: `DONE` if the goal is fully achieved, `IMPOSSIBLE` if the work above shows it cannot be achieved at all — not merely that it is hard or not finished yet — or `CONTINUE` if more work is needed.
- The second line is one short sentence: why it is done, why it cannot be done, or the single most important thing still missing.
