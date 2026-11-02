---
snippet: run a shell command and get its output
guideline.1: Use `shell` for searching (`rg`, `find`), listing, building and running tests; use `read` and `edit` for files.
---
Run a command with {{shell}} in the working directory and return its combined stdout and stderr with the exit code. Long output keeps the beginning and the end inline and saves the complete log to a file whose path is reported. Commands are killed when `timeout` seconds pass (default 120, maximum 600).

With `session`, the command runs in a terminal session of that name, which stays open for the next call: the directory, variables and background jobs carry over, and a command still running when `timeout` passes runs on instead of being killed, with `terminals` to read, interrupt or close it. `background` starts the command and returns at once, in `session` or a new one. Use a session for what lasts or builds on what came before — a dev server, a watcher, a shell set up step by step; plain calls for the rest. The user can open a session as a terminal panel and type into it too.
