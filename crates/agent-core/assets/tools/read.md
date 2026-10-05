---
snippet: read a file as numbered lines, paged with offset/limit
guideline.1: Use `read` instead of `cat`, `head` or `sed -n` to look at files.
---
Read a text file. Returns lines prefixed with their 1-based line number. Output is capped at 2000 lines or 64 KB, whichever comes first; use `offset` (first line to show) and `limit` (number of lines) to page through larger files. The result ends with a note when more lines remain.
