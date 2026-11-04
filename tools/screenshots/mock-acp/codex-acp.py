#!/usr/bin/env python3
"""A stand-in for Codex's ACP adapter in the screenshot recordings.

termide runs `npx … @agentclientprotocol/codex-acp` for a `codex` connection;
the `npx` beside this file runs this script instead, so the recordings show a
Codex connection with no network and no sign-in.

Unlike Claude Code, Codex keeps its own tools: it reads, edits and runs
commands itself and asks the client with `session/request_permission` before
what its approval preset does not allow, so termide's rules and permission
card decide. The script does the same — it lists models (one entry per
reasoning level, as the adapter does) and its approval and collaboration
modes as config options, reads and patches the demo project's files itself,
asks before the edit and the command, and runs the command when allowed. The
first request plays the task of mock-llm/server.py; any later one ends in a
used-up usage limit whose reset is given in its text, as Codex words it.
Standard library only.
"""

import json
import os
import subprocess
import sys
import threading
import time

MODELS = [
    ("gpt-6.1-sol/low", "gpt-6.1-sol (low)"),
    ("gpt-6.1-sol/medium", "gpt-6.1-sol (medium)"),
    ("gpt-6.1-sol/high", "gpt-6.1-sol (high)"),
    ("gpt-6-luna/low", "gpt-6-luna (low)"),
    ("gpt-6-luna/medium", "gpt-6-luna (medium)"),
]
MODES = [
    ("read-only", "Read Only"),
    ("workspace-write", "Default"),
    ("agent", "Auto review"),
    ("agent-full-access", "Full Access"),
]
COLLABORATION = [("default", "Default"), ("plan", "Plan")]
WINDOW = 272_000
# The recordings' clock is pinned at 10:42 UTC (env/setup.sh): the limit
# resets at 15:00 the same day.
LIMIT = "You've hit your usage limit. Upgrade to Pro or try again at 3:00 PM."

OLD = "            rows.append((item.sku, item.name, item.on_hand, item.reorder_point))\n"
NEW = (
    "            critical = item.on_hand <= item.reorder_point // 4\n"
    "            rows.append((item.sku, item.name, item.on_hand, item.reorder_point, critical))\n"
)
REPORT = "inventory/report.py"
TESTS = "python3 -m unittest -q"

CHUNK_DELAY = 0.04

out_lock = threading.Lock()
state = {
    "model": "gpt-6.1-sol/medium", "mode": "read-only", "collaboration": "default",
    "prompts": 0, "cwd": ".", "next_id": 1000,
}
replies = {}
replies_cv = threading.Condition()


def send(message):
    message["jsonrpc"] = "2.0"
    with out_lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def update(session_id, body):
    send({"method": "session/update", "params": {"sessionId": session_id, "update": body}})


def ask(method, params):
    """Send the client a request and wait for its answer's result."""
    with replies_cv:
        state["next_id"] += 1
        request_id = state["next_id"]
    send({"id": request_id, "method": method, "params": params})
    with replies_cv:
        while request_id not in replies:
            replies_cv.wait()
        return replies.pop(request_id)


def select(config_id, name, category, current, values):
    option = {
        "id": config_id, "name": name, "type": "select", "currentValue": current,
        "options": [{"value": v, "name": n} for v, n in values],
    }
    if category:
        option["category"] = category
    return option


def config_options():
    return [
        select("model", "Model", "model", state["model"], MODELS),
        select("mode", "Approval preset", "mode", state["mode"], MODES),
        select("collaboration_mode", "Collaboration mode", None, state["collaboration"], COLLABORATION),
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


def text_content(text):
    return [{"type": "content", "content": {"type": "text", "text": text}}]


def allowed(session_id, call):
    """Ask the client about `call`; whether it allowed it."""
    answer = ask("session/request_permission", {
        "sessionId": session_id,
        "toolCall": dict(call, status="pending"),
        "options": [
            {"optionId": "approved", "name": "Yes", "kind": "allow_once"},
            {"optionId": "approved-for-session", "name": "Always", "kind": "allow_always"},
            {"optionId": "abort", "name": "No", "kind": "reject_once"},
        ],
    })
    outcome = (answer or {}).get("outcome", {})
    return outcome.get("outcome") == "selected" and outcome.get("optionId", "").startswith("approved")


def run_task(session_id, request_id):
    cwd = state["cwd"]
    path = os.path.join(cwd, REPORT)
    used = 21_300

    def step_done():
        nonlocal used
        used += 2_900
        update(session_id, {"sessionUpdate": "usage_update", "used": used, "size": WINDOW})

    time.sleep(0.5)
    stream(session_id, "agent_thought_chunk",
           "The reorder report is built in inventory/report.py; reading it first.")
    stream(session_id, "agent_message_chunk", "I'll start with the report module.\n\n")
    # A command Codex parses as a read: titled and located, no raw input,
    # and no output reported.
    read = {"toolCallId": "call_read", "title": "Read file '%s'" % REPORT, "kind": "read",
            "locations": [{"path": REPORT}]}
    update(session_id, dict(read, sessionUpdate="tool_call", status="in_progress"))
    with open(path) as source:
        before = source.read()
    time.sleep(0.4)
    update(session_id, {"sessionUpdate": "tool_call_update", "toolCallId": "call_read",
                        "status": "completed"})
    step_done()

    time.sleep(0.5)
    stream(session_id, "agent_thought_chunk",
           "`reorder_rows` never marks critical items; stock at a "
           "quarter of the reorder point or less should carry a `critical` column.")
    stream(session_id, "agent_message_chunk",
           "`reorder_rows` lists low stock but never marks the critical items. "
           "Adding a `critical` column.\n\n")
    after = before.replace(OLD, NEW, 1)
    # A file change: announced with its diff, then asked about by its id and
    # paths alone.
    update(session_id, {
        "sessionUpdate": "tool_call", "toolCallId": "call_patch", "title": "Editing files",
        "kind": "edit", "status": "pending",
        "content": [{"type": "diff", "path": path, "oldText": before, "newText": after}],
    })
    if allowed(session_id, {"toolCallId": "call_patch", "title": "Editing files",
                            "locations": [{"path": path}]}):
        with open(path, "w") as target:
            target.write(after)
        update(session_id, {"sessionUpdate": "tool_call_update", "toolCallId": "call_patch",
                            "status": "completed"})
    else:
        update(session_id, {"sessionUpdate": "tool_call_update", "toolCallId": "call_patch",
                            "status": "failed"})
    step_done()

    time.sleep(0.5)
    stream(session_id, "agent_thought_chunk",
           "Running the suite in case something expects four columns.")
    stream(session_id, "agent_message_chunk", "Running the tests.\n\n")
    # A command: asked about before it starts, then announced by its line.
    if allowed(session_id, {"toolCallId": "call_tests", "title": "Run command", "kind": "execute",
                            "rawInput": {"command": TESTS, "cwd": cwd}, "locations": []}):
        update(session_id, {"sessionUpdate": "tool_call", "toolCallId": "call_tests",
                            "title": TESTS, "kind": "execute", "status": "in_progress",
                            "rawInput": {"command": TESTS, "cwd": cwd}})
        done = subprocess.run(TESTS, shell=True, cwd=cwd, capture_output=True, text=True)
        update(session_id, {"sessionUpdate": "tool_call_update", "toolCallId": "call_tests",
                            "status": "completed" if done.returncode == 0 else "failed",
                            "content": text_content((done.stdout + done.stderr).strip())})
    step_done()

    time.sleep(0.4)
    stream(session_id, "agent_message_chunk",
           "Done: `reorder_rows` now returns a fifth column, `critical`, true when an item's "
           "stock is at or below a quarter of its reorder point. The test suite passes.\n\n"
           "- `inventory/report.py` — the new column\n"
           "- the CSV export writes every column, so it carries the flag unchanged")
    send({"id": request_id, "result": {
        "stopReason": "end_turn",
        "usage": {"inputTokens": 9_850, "outputTokens": 1_920, "cachedReadTokens": 47_100,
                  "thoughtTokens": 1_140, "totalTokens": 58_870},
    }})


def hit_limit(session_id, request_id):
    time.sleep(1.2)
    send({"id": request_id, "error": {"code": -32603, "message": LIMIT}})


def handle(message):
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params") or {}
    if method is None:
        if request_id is not None:
            with replies_cv:
                replies[request_id] = message.get("result")
                replies_cv.notify_all()
        return
    if method == "initialize":
        send({"id": request_id, "result": {
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": True,
                "promptCapabilities": {"image": True, "embeddedContext": True},
                "mcpCapabilities": {"http": True, "sse": False},
            },
            "authMethods": [
                {"id": "chatgpt", "name": "Login with ChatGPT"},
                {"id": "codex-api-key", "name": "Use CODEX_API_KEY"},
                {"id": "openai-api-key", "name": "Use OPENAI_API_KEY"},
            ],
        }})
    elif method == "session/new":
        state["cwd"] = params.get("cwd") or "."
        send({"id": request_id, "result": {"sessionId": "demo-thread", "configOptions": config_options()}})
    elif method == "session/set_config_option":
        key = {"model": "model", "mode": "mode", "collaboration_mode": "collaboration"}.get(params.get("configId"))
        if key:
            state[key] = params.get("value")
        send({"id": request_id, "result": {"configOptions": config_options()}})
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
