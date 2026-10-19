#!/usr/bin/env python3
"""Shows the words of the active file in the status bar."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from termide_plugin import Plugin  # noqa: E402

plugin = Plugin()


def count(path):
    try:
        words = len(plugin.read(path)["text"].split())
    except Exception:
        return plugin.status("words", "")
    plugin.status("words", f"{words} words")


@plugin.on("editor/focused")
def focused(params):
    count(params["path"])


@plugin.on("editor/changed")
def changed(params):
    active = plugin.active()
    if active and active["path"] == params["path"]:
        count(params["path"])


plugin.run()
