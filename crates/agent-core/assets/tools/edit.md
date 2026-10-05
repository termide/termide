---
snippet: replace a unique piece of text in a file
guideline.1: Keep `old_string` in `edit` as small as possible while still unique.
---
Replace text in an existing file. `old_string` must match exactly one place in the file (include a few surrounding lines to make it unique), unless `replace_all` is true. Whitespace differences in indentation are tolerated, but copy the text from `read` as literally as you can. Returns a unified diff of the change.
