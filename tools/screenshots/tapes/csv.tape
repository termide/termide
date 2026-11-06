# CSV table viewer on the stock count export, sorted by stock on hand.
Source tapes/_settings.tape
Source tapes/_start.tape
Down 3
Right
Sleep 800ms
Down 3
Type "v"
Sleep 2s
Source tapes/_widen.tape
# The cursor to the on_hand column; two sorts: descending.
Right 4
Type "s"
Sleep 600ms
Type "s"
Sleep 1s
Down 2
Show
Sleep 500ms
Screenshot out/csv.png
Sleep 1s
