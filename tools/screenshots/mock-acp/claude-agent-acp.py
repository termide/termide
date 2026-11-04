#!/usr/bin/env python3
"""A stand-in for Claude Code's ACP adapter in the screenshot recordings.

termide runs `npx … @agentclientprotocol/claude-agent-acp` for a `claude_code`
connection; the `npx` beside this file runs this script instead, so the
recordings show a Claude Code connection with no network and no sign-in.

It speaks the Agent Client Protocol on stdin/stdout as the real adapter does
for termide: it lists models and an effort level as config options, and runs
termide's tools by calling the MCP server termide hands it in `session/new`,
announcing each call as `mcp__termide__<tool>` — so termide's permission
cards, edits and blocks are the real ones. The first request plays the same
task as mock-llm/server.py; any later one ends in a used-up session limit
with a reset time, for the limit card. Standard library only.
"""

import calendar
import json
import sys
import threading
import time
import urllib.request

MODELS = [
    ("default", "Default (recommended)"),
    ("opus[1m]", "Opus 5.5 (1M context)"),
    ("sonnet", "Sonnet 5"),
    ("haiku", "Haiku 4.5"),
]
EFFORTS = [("low", "Low"), ("medium", "Medium"), ("high", "High"), ("max", "Max")]
WINDOW = 200_000
# The recordings' clock is pinned at 10:42 UTC (env/setup.sh); the limit
# resets at 15:00 the same day.
RESET = calendar.timegm((2026, 9, 27, 15, 0, 0))

SCRIPT = [
    {
        "thought": "The user wants low-stock items flagged in the reorder report. "
        "I should read the report module first to see how rows are built.",
        "text": "Let me look at how the reorder report is built.",
        "tool": ("read", {"path": "inventory/report.py"}),
    },
    {
        "thought": "`reorder_rows` lists every item below its reorder point but never "
        "marks the critical ones. A `critical` flag for stock at or below a quarter "
        "of the reorder point fits.",
        "text": "The report lists items under their reorder point but does not mark "
        "the critical ones. I'll add a flag for stock at a quarter of the reorder "
        "point or less.",
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
        "thought": "Now the test suite, to make sure nothing depends on four-column rows.",
        "text": "Now the tests.",
        "tool": ("shell", {"command": "python3 -m unittest -q"}),
    },
    {
        "thought": "",
        "text": "Done. `reorder_rows` now returns a fifth column, `critical`, set when an "
        "item's stock is at or below a quarter of its reorder point, and the suite "
        "passes (6 tests).\n\n"
        "- `inventory/report.py` — the new flag in `reorder_rows`\n"
        "- the CSV export picks it up unchanged, since it writes every column\n\n"
        "Want me to highlight critical rows in the terminal report as well?",
        "tool": None,
    },
]

CHUNK_DELAY = 0.04

out_lock = threading.Lock()
state = {"model": "default", "effort": "high", "prompts": 0, "mcp": None, "next_id": 1000}
replies = {}
replies_cv = threading.Condition()


def send(message):
    message["jsonrpc"] = "2.0"
    with out_lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def update(session_id, body):
    send({"method": "session/update", "params": {"sessionId": session_id, "update": body}})


def config_options():
    return [
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": state["model"],
            "options": [{"value": v, "name": n} for v, n in MODELS],
        },
        {
            "id": "effort",
            "name": "Effort",
            "category": "thought_level",
            "type": "select",
            "currentValue": state["effort"],
            "options": [{"value": v, "name": n} for v, n in EFFORTS],
        },
    ]


def words(text):
    out, cur = [], ""
    for ch in text:
        cur += ch
        if ch in " \n":
            out.append(cur)
            cur = ""
    if cur:
        out.append(cur)
    return out


def stream(session_id, kind, text):
    for piece in words(text):
        update(session_id, {"sessionUpdate": kind, "content": {"type": "text", "text": piece}})
        time.sleep(CHUNK_DELAY)


def call_tool(name, arguments):
    """Run termide's tool `name` through its MCP server; the reply's text."""
    server = state["mcp"]
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
         "params": {"name": name, "arguments": arguments}}
    ).encode()
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    for header in server.get("headers", []):
        headers[header["name"]] = header["value"]
    request = urllib.request.Request(server["url"], data=body, headers=headers, method="POST")
    with urllib.request.urlopen(request, timeout=600) as response:
        raw = response.read().decode()
    if raw.lstrip().startswith("data:") or "\ndata:" in raw:
        raw = [line[5:] for line in raw.splitlines() if line.startswith("data:")][-1]
    reply = json.loads(raw)
    content = reply.get("result", {}).get("content", [])
    return "\n".join(c.get("text", "") for c in content if c.get("type") == "text")


def tool_names():
    server = state["mcp"]
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).encode()
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    for header in server.get("headers", []):
        headers[header["name"]] = header["value"]
    request = urllib.request.Request(server["url"], data=body, headers=headers, method="POST")
    with urllib.request.urlopen(request, timeout=30) as response:
        raw = response.read().decode()
    if "data:" in raw and not raw.lstrip().startswith("{"):
        raw = [line[5:] for line in raw.splitlines() if line.startswith("data:")][-1]
    return {t["name"] for t in json.loads(raw).get("result", {}).get("tools", [])}


def run_task(session_id, request_id):
    names = tool_names() if state["mcp"] else set()
    used = 18_400
    for step, entry in enumerate(SCRIPT):
        time.sleep(0.5)
        if entry["thought"]:
            stream(session_id, "agent_thought_chunk", entry["thought"])
        stream(session_id, "agent_message_chunk", entry["text"] + ("\n\n" if entry["tool"] else ""))
        if entry["tool"]:
            name, args = entry["tool"]
            # termide's command tool is `bash` up to 0.48 and `shell` after.
            if name == "shell" and "shell" not in names and "bash" in names:
                name = "bash"
            call_id = "toolu_%02d" % step
            update(session_id, {
                "sessionUpdate": "tool_call", "toolCallId": call_id,
                "title": "mcp__termide__" + name, "kind": "other",
                "status": "pending", "rawInput": args,
            })
            text = call_tool(name, args) if state["mcp"] else ""
            update(session_id, {
                "sessionUpdate": "tool_call_update", "toolCallId": call_id,
                "status": "completed",
                "content": [{"type": "content", "content": {"type": "text", "text": text}}],
            })
        used += 2_600
        update(session_id, {"sessionUpdate": "usage_update", "used": used, "size": WINDOW})
    send({"id": request_id, "result": {
        "stopReason": "end_turn",
        "usage": {"inputTokens": 42, "outputTokens": 1_310, "cachedReadTokens": 61_200,
                  "cachedWriteTokens": 19_800, "totalTokens": 82_352},
    }})


def hit_limit(session_id, request_id):
    time.sleep(1.2)
    update(session_id, {
        "sessionUpdate": "usage_update", "used": 31_000, "size": WINDOW,
        "_meta": {"_claude/rateLimit": {"status": "rejected", "resetsAt": RESET}},
    })
    send({"id": request_id, "error": {
        "code": -32603,
        "message": "Internal error: You've hit your session limit · resets 3pm (UTC)",
        "data": {"errorKind": "rate_limit"},
    }})


def handle(message):
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params") or {}
    if method is None:
        return
    if method == "initialize":
        send({"id": request_id, "result": {
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": False,
                "promptCapabilities": {"image": True, "embeddedContext": True},
                "mcpCapabilities": {"http": True, "sse": True},
            },
            "authMethods": [],
        }})
    elif method == "session/new":
        servers = [s for s in params.get("mcpServers", []) if s.get("type") == "http"]
        if servers:
            state["mcp"] = servers[0]
        send({"id": request_id, "result": {"sessionId": "demo-session", "configOptions": config_options()}})
    elif method == "session/set_config_option":
        if params.get("configId") == "model":
            state["model"] = params.get("value")
        elif params.get("configId") == "effort":
            state["effort"] = params.get("value")
        send({"id": request_id, "result": {"configOptions": config_options()}})
    elif method == "session/set_model":
        state["model"] = params.get("modelId")
        send({"id": request_id, "result": {}})
    elif method == "session/prompt":
        state["prompts"] += 1
        work = run_task if state["prompts"] == 1 else hit_limit
        threading.Thread(target=work, args=(params.get("sessionId"), request_id), daemon=True).start()
    elif request_id is not None:
        send({"id": request_id, "result": {}})


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except ValueError:
            continue
        handle(message)


if __name__ == "__main__":
    main()
