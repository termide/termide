---
snippet: offer the user a prompt to re-run on an interval or back to back, and wait for them to start it
guideline.1: When something must be done again and again — polling a CI run, watching a deploy, working down a queue — offer it with `suggest_loop` rather than writing out a `/loop` line for the user to paste.
---
Offer the user a `/loop`: `prompt` comes to you as their message now, then again every `interval` (`30s`, `5m`, `2h`), or as soon as each run ends when `interval` is left out. The loop is shown on a card with `[Start]`, `[Edit first]` and `[Don't start]`; nothing starts unless they pick `[Start]`, and a loop already under way is replaced only if they do. Each run starts from the prompt alone, so make it complete in itself. A loop runs until the user stops it or its run cap is reached: do not offer one for a single check.
