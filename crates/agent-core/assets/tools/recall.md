---
snippet: search the project's earlier sessions, git history and files for past work, decisions and their reasons
guideline.1: Use `recall` before non-trivial work in an area you have not seen this session, and whenever the user refers to earlier work or decisions ("as we discussed", "why did we", "last time"); prefer it to grepping session logs or `git log` by hand.
guideline.2: Cite the references `recall` returns, and `open` one before relying on a summarised answer.
---
Search what this project already knows: earlier agent sessions (what was asked, decided, tried and why), its git history (commit messages, and commits that added or removed a name) and its files — notes, documents and code, a long Markdown file section by section — ranked together, with a reference for each result. Give `queries`: several phrasings of what you look for — the request in English and in the user's language, plus the names, terms, identifiers or file names it would involve. Narrow with `sources`, `paths` (project paths or globs) and `since` (YYYY-MM-DD). To see the context of a session or commit result, call again with `open` set to its reference; read a file result with the read tool.
