# Git diff: the working tree's changes staged a hunk and a line at a time.
Source tapes/_settings.tape
# _start.tape, with a second change at the top of models.py first, so its
# diff holds two hunks.
Hide
Type "source /opt/shots/env/setup.sh" Enter
Sleep 2s
Type `sed -i 's/^    supplier: str = "Acme Supply"$/    supplier: str = "Acme Supply"\n    category: str = "General"/' inventory/models.py` Enter
Sleep 300ms
Type "termide --no-lsp" Enter
Sleep 3s
# Git status; Shift+Tab goes to its buttons, the fifth is [Diff].
Alt+g
Sleep 800ms
Shift+Tab
Right 4
Enter
Sleep 1500ms
Source tapes/_widen.tape
Show
Sleep 300ms
# The first hunk of models.py staged on its own: it moves to the staged side,
# and git status lists models.py on both.
Type "]"
Sleep 300ms
Type " "
Sleep 1500ms
Screenshot out/git-diff.png
Sleep 1s
