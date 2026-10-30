# Perforce with a teammate: the status bar says who else has the file
# opened, and Ctrl+E asks first before it checks the file out.
# The `# hint:` lines are the key badges keys.mjs lays over the recording.
Output out/perforce-team.gif
Source tapes/_settings.tape
Hide
Type "source /opt/shots/env/setup.sh && source /opt/shots/env/p4-scene.sh" Enter
Sleep 3s
Type "termide --no-lsp" Enter
Sleep 4s
# Player.cpp from Source/Game, under the file manager in the first column.
Down 3
Right
Sleep 500ms
Down
Right
Sleep 800ms
Down 3
Enter
Sleep 2s
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
Show
Sleep 2s
# hint: Ctrl+E "edit" for=1.4s
Ctrl+E
Sleep 2.5s
Screenshot out/perforce-team.png
Sleep 1.5s
# hint: Enter "open it too" for=1.4s
Enter
Sleep 2.5s
