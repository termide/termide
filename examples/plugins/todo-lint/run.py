#!/usr/bin/env python3
"""Marks TODO comments in the editor, offers to mark one DONE, and lists them
all in a panel (command "List TODOs", Ctrl+Alt+T)."""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from termide_plugin import Plugin, utf16_len  # noqa: E402

plugin = Plugin()
TODO = re.compile(r"\bTODO\b")
found = {}  # path -> [(line, col, text)]


def scan(path):
    try:
        text = plugin.read(path)["text"]
    except Exception:
        return
    marks = []
    for number, line in enumerate(text.split("\n")):
        match = TODO.search(line)
        if match:
            col = utf16_len(line[: match.start()])
            marks.append((number, col, line.strip()))
    found[path] = marks
    plugin.diagnostics(path, [
        {"line": n, "col": c, "end_col": c + 4, "severity": "info", "message": "TODO left behind"}
        for n, c, _ in marks
    ])


@plugin.on("file/opened")
def opened(params):
    scan(params["path"])


@plugin.on("editor/focused")
def focused(params):
    # A file open before the plugin started is seen here.
    if params["path"] not in found:
        scan(params["path"])


@plugin.on("editor/changed")
def changed(params):
    scan(params["path"])


@plugin.code_actions(["*"])
def fixes(path, span, diagnostics):
    return [
        {
            "title": "Mark this TODO as DONE",
            "edits": [{"line": d["line"], "col": d["col"], "end_col": d["col"] + 4, "text": "DONE"}],
        }
        for d in diagnostics
        if d.get("source") == plugin.name
    ]


@plugin.command("list", "List TODOs", key="Ctrl+Alt+T")
def list_todos():
    places = [(path, n, c, text) for path, marks in found.items() for n, c, text in marks]
    lines = ["# TODOs", ""]
    for index, (path, n, _, text) in enumerate(places):
        lines.append(f"- [{os.path.basename(path)}:{n + 1}](plugin:open-{index}) {text}")
    if not places:
        lines.append("None in the files opened so far.")

    def act(action):
        if action.startswith("open-"):
            path, n, c, _ = places[int(action[5:])]
            plugin.open_file(path, n, c)

    plugin.open_panel("TODOs", "\n".join(lines), on_action=act)


plugin.run()
