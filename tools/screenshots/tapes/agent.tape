# Coding agent against the scripted mock model, set to work by /goal: the
# goal's row in the state strip, read, edit, a permission card for bash, the
# answer, then the judge's verdict.
Output out/agent.gif
Source tapes/_settings.tape
Source tapes/_start.tape
Alt+a
Sleep 2s
Source tapes/_widen.tape
Show
Sleep 500ms
Type "/goal flag critical low-stock items in the reorder report, tests green"
Sleep 500ms
Screenshot out/agent-prompt.png
Enter
Sleep 4s
Screenshot out/agent-working.png
Sleep 6s
Screenshot out/agent-permission.png
Type "1"
Sleep 14s
Screenshot out/agent-done.png
Sleep 2s
