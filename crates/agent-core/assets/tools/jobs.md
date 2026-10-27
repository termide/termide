---
snippet: list what you set going that still runs — background tasks, calls moved to the background — and stop one
guideline.1: Stop what you set going once you no longer need it — the plan changed, another source answered, the user said so — with `jobs` and `stop`, rather than letting it run on; it asks no one, since it is your own work.
---
List what you set going that still runs on without you waiting for it: tasks you handed over with `task` and `background`, tasks waiting out a usage limit, and calls you moved to the background. Each line starts with its id. With `stop` set to an id, stop that one; nothing comes back from it afterwards. A `/goal` or `/loop` is the user's and is not listed.
