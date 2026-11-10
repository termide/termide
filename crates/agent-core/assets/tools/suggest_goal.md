---
snippet: offer the user a goal to work toward unattended, judged after each turn, and wait for them to start it
guideline.1: When work will take many turns and can be checked as done — a green build, a finished migration — offer it with `suggest_goal` rather than writing out a `/goal` line for the user to paste.
---
Offer the user a `/goal`: you work toward it on your own, turn after turn, and after each turn a judge reads the session and decides whether it is reached. The goal is shown on a card with `[Start]`, `[Edit first]` and `[Don't start]`; nothing starts unless they pick `[Start]`, and a goal already under way is replaced only if they do. Once started, its first turn comes to you as their message when this run ends, so finish the step you are on and end your turn. Write `goal` as an outcome a judge can check from the session — what is true when it is done — not as a list of steps. Do not offer one for work you can finish in this run.
