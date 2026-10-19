# Debugger: a breakpoint in report.py, the launch from the command palette,
# the stop in the debug panel, a continue to the next stop and an expression
# evaluated there. VHS sends no F-keys, so env/config.toml binds the two this
# needs: Alt+y is toggle_breakpoint (Ctrl+F8), Ctrl+b toggle_stack
# (Alt+Backspace); F5 is reached through the palette's "Start debugging".
# The `# hint:` lines are the key badges keys.mjs lays over the recording
# (release-feature-posts): the real keys, not the stand-ins sent here.
Output out/debug.gif
Source tapes/_settings.tape
Hide
Type "source /opt/shots/env/setup.sh && source /opt/shots/env/debug-scene.sh" Enter
Sleep 2s
Type "termide --no-lsp" Enter
Sleep 3s
# report.py from the file manager, widened; the git and outline column
# closed, so the debug panel gets a column of its own beside the editor.
Down 5
Right
Sleep 800ms
Down 4
Enter
Sleep 2s
Source tapes/_widen.tape
Alt+u
Sleep 300ms
Alt+w
Sleep 300ms
Alt+w
Sleep 300ms
Alt+j
Sleep 600ms
# The cursor on `rows.append(...)`.
Down 13
Sleep 1s
Show
Sleep 1s
# hint: Ctrl+F8 "breakpoint" for=1.4s
Alt+y
Sleep 1.5s
# hint: Ctrl+P "Start debugging" for=2.3s
Ctrl+p
Sleep 800ms
Type "start debugging"
Sleep 1s
Enter
# Two launches: the project's debug command and report.py itself.
Sleep 1.5s
# hint: Enter "Debug reorder report" for=1.4s
Enter
Sleep 2.5s
# The debug panel opens stacked under the editor; unstack it.
Ctrl+b
Sleep 3s
Screenshot out/debug.png
# hint: c "continue"
Type "c"
Sleep 2.5s
# hint: e "evaluate" for=3.5s
Type "e"
Sleep 800ms
Type "item.reorder_point - item.on_hand"
Sleep 500ms
Enter
Sleep 2.5s
Screenshot out/debug-eval.png
Sleep 1s
