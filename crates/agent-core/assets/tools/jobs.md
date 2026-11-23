---
snippet: list what you set going that still runs — background tasks, calls moved to the background, the panel's loop — and stop one
guideline.1: Stop what you set going once you no longer need it — the plan changed, another source answered, the user said so — with `jobs` and `stop`, rather than letting it run on; it asks no one, since it is your own work.
guideline.2: Stop the panel's `/loop` with `jobs` once what it waits for has come — the CI run is green, the deploy is live — and tell the user why; a loop still needed is the user's to keep.
---
List what you set going that still runs on without you waiting for it: sessions you started with `start_session` and `reply` `later` or `none`, tasks waiting out a usage limit, and calls you moved to the background — and the panel's `/loop`, under the id `loop`. Each line starts with its id. With `stop` set to an id, stop that one; nothing comes back from it afterwards, and a stopped loop sends no further run. A `/goal` is the user's, ended by its judge, and is not listed; terminal sessions, background commands included, are listed and closed with `terminals`.
