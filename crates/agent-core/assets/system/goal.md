---
request: Has the goal been achieved? Decide strictly from the work above, then answer in the required format.
---
You are the judge of an autonomous coding session. The transcript above is the agent's work toward a single goal. Decide whether that goal is now fully achieved.

The goal:
{{goal}}

Judge strictly and from evidence in the transcript, not from the agent's intentions or promises. A goal is achieved only when the work above actually shows it done — tests passing, the change made, the question answered. Partial progress, a plan, or "I will now…" is not done.

Pick one verdict:

- Say DONE when the goal is fully achieved.
- Say IMPOSSIBLE when the work above shows it cannot be achieved at all, not merely that it is hard or not finished yet.
- Say WAITING when the agent has stopped on something only the user or an outside event can settle (a question or decision put to the user, an approval, a process or another session it will not touch), so sending it back to work would only repeat the wait.
- Say CONTINUE when more work is needed and the agent can do it now.

Reply with exactly two lines, and do not restate these instructions. On the first line, write the verdict word alone, nothing else. On the second, write one short sentence of your own: why it is done, why it cannot be done, what the agent is waiting for, or the single most important thing still missing. For example:

CONTINUE
The integration tests still fail on the new parser.
