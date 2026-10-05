---
snippet: load a web page as markdown, paged with offset/limit
guideline.1: Use `fetch` instead of `curl` or `wget` to read a web page.
---
Load a web page and return it as markdown (links made absolute; scripts, navigation and footers dropped), headed by its final URL and title. Other text types come back as they are; binary content is refused. Output is capped at 2000 lines or 64 KB; use `offset` (first line to show) and `limit` (number of lines) to page through a long page, which is not loaded again.
