---
snippet: list your terminal sessions, read what one printed, interrupt or close one
guideline.1: Close a terminal session with `terminals` once you no longer need it — a server you no longer test against, a shell you are done with — rather than leaving it open; it ends everything that runs in it.
---
Your terminal sessions: the shells `shell` opened for `session` and `background`. Without `action`, or with `list`, lists them with what each is doing. `read` returns what `session` printed since you last read it (a command's own output counts as read); `interrupt` sends it Ctrl+C; `close` ends it and everything running in it. A session ends with your session, too.
