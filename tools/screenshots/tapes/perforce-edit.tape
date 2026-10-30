# Perforce, the everyday loop: a file the depot has is read-only in the
# editor, the status bar tells its revision; Ctrl+E, the editor's own edit
# toggle, checks it out (the plugin runs `p4 edit`), and as it is typed the
# gutter marks what changed against the depot and the file manager colours
# the file. Ctrl+E again, on the saved file, asks whether to revert it.
# The `# hint:` lines are the key badges keys.mjs lays over the recording.
Output out/perforce-edit.gif
Source tapes/_settings.tape
Hide
Type "source /opt/shots/env/setup.sh && source /opt/shots/env/p4-scene.sh" Enter
Sleep 3s
Type "termide --no-lsp" Enter
Sleep 4s
# Source/Game expanded, Health.cpp opened in the first column.
Down 3
Right
Sleep 500ms
Down
Right
Sleep 800ms
Down
Show
Sleep 1.5s
Screenshot out/perforce-files.png
Enter
Sleep 2s
# Into the first column under the file manager; the second file manager
# and the outline (no symbols without a language server) closed.
Alt+v
Sleep 400ms
Alt+u
Sleep 300ms
Alt+w
Sleep 600ms
Alt+w
Sleep 600ms
Alt+j
Sleep 300ms
Sleep 1.5s
Screenshot out/perforce-readonly.png
# hint: Ctrl+E "edit: p4 edit" for=1.6s
Ctrl+E
Sleep 2.5s
# The clamp in Heal gets a guard like Damage's; the editor closes the brace.
Down 14
Right
Sleep 500ms
Enter
Type "if (amount <= 0) {"
Enter
Type "return;"
Down 2
Sleep 1s
# hint: Ctrl+S "save" for=1.4s
Ctrl+S
Sleep 2.5s
Screenshot out/perforce-edited.png
Sleep 1s
# Editing back off on the saved, opened file: revert it, or only view it.
# hint: Ctrl+E "view: revert?" for=1.6s
Ctrl+E
Sleep 2s
Screenshot out/perforce-stop.png
Sleep 1.5s
Escape
Sleep 1s
# Editing off over unsaved work: save it, drop it, or revert the file.
Type "x"
Sleep 600ms
# hint: Ctrl+E "view: unsaved?" for=1.6s
Ctrl+E
Sleep 2s
Screenshot out/perforce-stop-unsaved.png
Sleep 1.5s
Escape
Sleep 1s
