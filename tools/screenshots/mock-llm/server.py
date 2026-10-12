#!/usr/bin/env python3
"""Deterministic OpenAI-compatible endpoint for the screenshot recordings.

Serves `GET /v1/models` and a streamed `POST /v1/chat/completions` that plays
a fixed conversation: the step is chosen by how many tool results the request
already carries, so every recording sees the same turns in the same order.
Standard library only; listens on loopback and never makes a request itself.
"""

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL = "qwen3-coder-30b-a3b"
CONTEXT_WINDOW = 65536

# One entry per step. `reasoning` and `text` stream as deltas; `tool` ends the
# step with a single tool call. The edit strings must match fixtures exactly.
SCRIPT = [
    {
        "reasoning": "The user wants low-stock items flagged in the reorder "
        "report. I should read the report module first to see how rows are built.",
        "text": "Let me look at how the reorder report is built.",
        "tool": ("read", {"path": "inventory/report.py"}),
    },
    {
        "reasoning": "`reorder_rows` lists every item below its reorder point but "
        "never marks the critical ones. I'll add a `critical` flag for items at "
        "or below a quarter of the reorder point.",
        "text": "The report lists items under their reorder point but does not "
        "mark the critical ones. I'll add a flag for stock at a quarter of the "
        "reorder point or less.",
        "tool": (
            "edit",
            {
                "path": "inventory/report.py",
                "old_string": "            rows.append((item.sku, item.name, item.on_hand, item.reorder_point))",
                "new_string": "            critical = item.on_hand <= item.reorder_point // 4\n"
                "            rows.append((item.sku, item.name, item.on_hand, item.reorder_point, critical))",
            },
        ),
    },
    {
        "reasoning": "Now run the test suite to make sure nothing else depends on the "
        "four-column rows.",
        "text": "Now the tests.",
        "tool": ("bash", {"command": "python3 -m unittest -q"}),
    },
    {
        "reasoning": "",
        "text": "Done. `reorder_rows` now returns a fifth column, `critical`, set when "
        "an item's stock is at or below a quarter of its reorder point, and the "
        "suite passes (6 tests).\n\n"
        "- `inventory/report.py` — the new flag in `reorder_rows`\n"
        "- the CSV export picks it up unchanged, since it writes every column\n\n"
        "Want me to highlight critical rows in the terminal report as well?",
        "tool": None,
    },
]

# Pacing: a visible spinner and a believable tokens-per-second figure.
CHUNK_DELAY = 0.04
PREFILL_DELAY = 0.6


def words(text):
    out, cur = [], ""
    for ch in text:
        cur += ch
        if ch == " ":
            out.append(cur)
            cur = ""
    if cur:
        out.append(cur)
    return out


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("mock-llm: " + (fmt % args) + "\n")

    def send_json(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.rstrip("/").endswith("/models"):
            self.send_json(
                {
                    "object": "list",
                    "data": [{"id": MODEL, "object": "model", "max_model_len": CONTEXT_WINDOW}],
                }
            )
        else:
            self.send_json({"error": {"message": "not found"}}, 404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        request = json.loads(self.rfile.read(length) or b"{}")
        if not self.path.rstrip("/").endswith("/chat/completions"):
            self.send_json({"error": {"message": "not found"}}, 404)
            return
        messages = request.get("messages", [])
        if not request.get("tools"):
            # A service call (title, compaction, judge): a short plain answer.
            # The /goal judge is told it is one in its system prompt
            # (agent-core's goal.md) and gets the verdict it waits for.
            system = " ".join(
                str(m.get("content")) for m in messages if m.get("role") == "system"
            )
            if "judge of an autonomous coding session" in system:
                text = "DONE\nCritical items are flagged and the 6 tests pass."
            else:
                text = "Flag low-stock items in the reorder report"
            step = {"reasoning": "", "text": text, "tool": None}
        else:
            done = sum(1 for m in messages if m.get("role") == "tool")
            step = SCRIPT[min(done, len(SCRIPT) - 1)]
        self.stream(step, messages)

    def chunk(self, delta=None, finish=None, usage=None):
        obj = {
            "id": "chatcmpl-demo",
            "object": "chat.completion.chunk",
            "created": 1767225600,
            "model": MODEL,
            "choices": [] if usage else [{"index": 0, "delta": delta or {}, "finish_reason": finish}],
        }
        if usage:
            obj["usage"] = usage
        data = ("data: " + json.dumps(obj) + "\n\n").encode()
        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
        self.wfile.flush()

    def stream(self, step, messages):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        time.sleep(PREFILL_DELAY)
        produced = 0
        self.chunk({"role": "assistant", "content": ""})
        for piece in words(step["reasoning"]):
            self.chunk({"reasoning_content": piece})
            produced += 1
            time.sleep(CHUNK_DELAY)
        for piece in words(step["text"]):
            self.chunk({"content": piece})
            produced += 1
            time.sleep(CHUNK_DELAY)
        if step["tool"]:
            name, args = step["tool"]
            self.chunk(
                {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_%d" % len(messages),
                            "type": "function",
                            "function": {"name": name, "arguments": json.dumps(args)},
                        }
                    ]
                },
                finish="tool_calls",
            )
            produced += 20
        else:
            self.chunk({}, finish="stop")
        prompt = 2400 + 180 * len(messages)
        self.chunk(usage={"prompt_tokens": prompt, "completion_tokens": produced * 2})
        data = b"data: [DONE]\n\n"
        self.wfile.write(b"%x\r\n%s\r\n0\r\n\r\n" % (len(data), data))
        self.wfile.flush()


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 10000
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
