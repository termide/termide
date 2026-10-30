# Perforce, before a submit: the opened files by changelist, in a tree of
# folders (Alt+G, git's status key, in a project outside git), the diff of one against the depot,
# and the default changelist submitted with a description asked for. Space
# lists a row's actions; Enter runs the first, a group's submit.
# The `# hint:` lines are the key badges keys.mjs lays over the recording.
Output out/perforce-opened.gif
Source tapes/_settings.tape
Hide
Type "source /opt/shots/env/setup.sh && source /opt/shots/env/p4-scene.sh && guard_heal && clear" Enter
Sleep 3s
Type "termide --no-lsp" Enter
Sleep 4s
# Source/Game expanded, so the colours show.
Down 3
Right
Sleep 500ms
Down
Right
Sleep 800ms
Show
Sleep 1s
# hint: Alt+G "opened files" for=2s
Alt+g
Sleep 2.5s
Hide
# The panel into the first column under the file manager; the second file
# manager and the outline closed.
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
# Past the folders Source and Game to the file.
Down 3
Sleep 600ms
Show
Sleep 1.5s
Screenshot out/perforce-opened.png
Sleep 500ms
# The row's actions, and its diff against the depot.
# hint: Space "actions" for=1.4s
Space
Sleep 1.2s
Screenshot out/perforce-actions.png
Sleep 300ms
Down
Sleep 500ms
Enter
Sleep 2.5s
# The diff panel alone, to read it whole.
Alt+z
Sleep 1.5s
Screenshot out/perforce-diff.png
Sleep 1s
Hide
Alt+z
Sleep 400ms
Alt+w
Sleep 800ms
# Back up past the folders to the changelist.
Up 3
Show
Sleep 1s
# hint: Enter "Submit" for=1.4s
Enter
Sleep 1.5s
Type "Guard Heal against negative amounts"
Sleep 800ms
Enter
Sleep 6s
Screenshot out/perforce-submitted.png
Sleep 1s
Enter
Sleep 2.5s
Screenshot out/perforce-after-submit.png
Sleep 500ms
