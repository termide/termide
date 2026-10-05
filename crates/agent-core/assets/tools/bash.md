---
snippet: run a shell command and get its output
guideline.1: Use `bash` for searching (`rg`, `find`), listing, building and running tests; use `read` and `edit` for files.
---
Run a bash command in the working directory and return its combined stdout and stderr with the exit code. Long output keeps the beginning and the end inline and saves the complete log to a file whose path is reported. Commands are killed when `timeout` seconds pass (default 120, maximum 600).
