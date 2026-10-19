#!/usr/bin/env python3
"""Before Ctrl+S writes a file, take the spaces off the ends of its lines."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from termide_plugin import Plugin, utf16_len  # noqa: E402

plugin = Plugin()


@plugin.will_save
def trim(path, version):
    text = plugin.read(path)["text"]
    edits = []
    for number, line in enumerate(text.split("\n")):
        kept = line.rstrip(" \t")
        if kept != line:
            edits.append({
                "line": number,
                "col": utf16_len(kept),
                "end_line": number,
                "end_col": utf16_len(line),
                "text": "",
            })
    return edits


plugin.run()
