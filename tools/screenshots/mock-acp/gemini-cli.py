#!/usr/bin/env python3
"""A stand-in for Gemini CLI in its ACP mode in the screenshot recordings.

termide runs `npx … @google/gemini-cli --acp` for a `gemini_cli` connection;
the `npx` beside this file runs this script instead, so the recordings show a
Gemini CLI connection with no network and no sign-in.

Gemini CLI keeps its own tools, as Codex does, and speaks the older ACP
shapes: its session states `modes` (approval modes, switched with
`session/set_mode`) and `models` (switched with `session/set_model`) rather
than config options. A call carries no raw input: a read is titled by its
path, an edit by `path: old => new` with its diff, a command by its line, all
located; what its approval mode does not allow is announced as pending and
then asked about with the whole call. Each thought is its subject in bold and
a description. The first request plays the task of mock-llm/server.py; any
later one ends in the error Gemini CLI gives for a used-up quota, which says
nothing of when it resets. Standard library only.
"""

import json
import os
import subprocess
import sys
import threading
import time

MODELS = [
    ("auto-gemini-4", "Auto (Gemini 4)"),
    ("gemini-4-pro-preview", "gemini-4-pro-preview"),
    ("gemini-4-flash-preview", "gemini-4-flash-preview"),
    ("gemini-3.5-pro", "gemini-3.5-pro"),
    ("gemini-3.5-flash", "gemini-3.5-flash"),
    ("gemini-3.5-flash-lite", "gemini-3.5-flash-lite"),
]
MODES = [
    ("default", "Default", "Prompts for approval"),
    ("autoEdit", "Auto Edit", "Auto-approves edit tools"),
    ("yolo", "YOLO", "Auto-approves all tools"),
]
WINDOW = 1_048_576
# What Gemini CLI's ACP mode answers for any 429 once its own retries are
# spent: the reset time the API stated is not passed on.
LIMIT = "Rate limit exceeded. Try again later."

OLD = "            rows.append((item.sku, item.name, item.on_hand, item.reorder_point))\n"
NEW = (
    "            critical = item.on_hand <= item.reorder_point // 4\n"
    "            rows.append((item.sku, item.name, item.on_hand, item.reorder_point, critical))\n"
)
REPORT = "inventory/report.py"
TESTS = "python3 -m unittest -q"

CHUNK_DELAY = 0.04

out_lock = threading.Lock()
state = {"model": "auto-gemini-4", "mode": "default", "prompts": 0, "cwd": ".", "next_id": 1000}
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


def say(session_id, text):
    for piece in words(text):
        update(session_id, {"sessionUpdate": "agent_message_chunk",
                            "content": {"type": "text", "text": piece}})
        time.sleep(CHUNK_DELAY)


def think(session_id, subject, description):
    """A thought as Gemini CLI sends it: one chunk, its subject in bold."""
    update(session_id, {"sessionUpdate": "agent_thought_chunk",
                        "content": {"type": "text", "text": "**%s**\n%s" % (subject, description)}})
    time.sleep(0.6)


def text_content(text):
    return [{"type": "content", "content": {"type": "text", "text": text}}]


def confirmed(session_id, call, options):
    """Announce `call` as pending, ask the client about it, and tell whether
    it allowed it."""
    update(session_id, dict(call, sessionUpdate="tool_call", status="pending"))
    answer = ask("session/request_permission", {
        "sessionId": session_id,
        "toolCall": dict(call, status="pending"),
        "options": options,
    })
    outcome = (answer or {}).get("outcome", {})
    return outcome.get("outcome") == "selected" and outcome.get("optionId", "").startswith("proceed")


OPTIONS = [
    {"optionId": "proceed_once", "name": "Allow", "kind": "allow_once"},
    {"optionId": "proceed_always", "name": "Allow for this session", "kind": "allow_always"},
    {"optionId": "cancel", "name": "Reject", "kind": "reject_once"},
]


def run_task(session_id, request_id):
    cwd = state["cwd"]
    path = os.path.join(cwd, REPORT)
    used = 18_400

    def turn_done(spent):
        nonlocal used
        used += spent
        update(session_id, {"sessionUpdate": "usage_update", "used": used, "size": WINDOW})

    time.sleep(0.5)
    think(session_id, "Locating the report",
          "The reorder report is built in inventory/report.py; reading it first.")
    say(session_id, "I'll start with the report module.\n\n")
    # A read needs no approval: announced running, titled by its path.
    read = {"toolCallId": "read_file-1", "title": REPORT, "kind": "read",
            "content": [], "locations": [{"path": path}]}
    update(session_id, dict(read, sessionUpdate="tool_call", status="in_progress"))
    with open(path) as source:
        before = source.read()
    time.sleep(0.4)
    update(session_id, dict(read, sessionUpdate="tool_call_update", status="completed"))
    turn_done(2_600)

    time.sleep(0.5)
    think(session_id, "Flagging critical stock",
          "reorder_rows never marks critical items; stock at a quarter of the "
          "reorder point or less should carry a critical column.")
    say(session_id, "`reorder_rows` lists low stock but never marks the critical items. "
                    "Adding a `critical` column.\n\n")
    after = before.replace(OLD, NEW, 1)
    diff = {"type": "diff", "path": path, "oldText": before, "newText": after,
            "_meta": {"kind": "modify"}}
    edit = {"toolCallId": "replace-2",
            "title": "%s: %s => %s" % (REPORT, OLD.strip()[:30] + "...", NEW.strip()[:30] + "..."),
            "kind": "edit", "content": [diff], "locations": [{"path": path}]}
    if confirmed(session_id, edit, OPTIONS):
        with open(path, "w") as target:
            target.write(after)
        update(session_id, dict(edit, sessionUpdate="tool_call_update", status="completed"))
    else:
        update(session_id, {"sessionUpdate": "tool_call_update", "toolCallId": "replace-2",
                            "status": "failed", "kind": "edit",
                            "content": text_content('Tool "replace" was canceled by the user.')})
    turn_done(2_900)

    time.sleep(0.5)
    think(session_id, "Checking the tests",
          "Running the suite in case something expects four columns.")
    say(session_id, "Running the tests.\n\n")
    shell = {"toolCallId": "run_shell_command-3", "title": TESTS, "kind": "execute",
             "content": [], "locations": []}
    if confirmed(session_id, shell, OPTIONS):
        done = subprocess.run(TESTS, shell=True, cwd=cwd, capture_output=True, text=True)
        update(session_id, dict(shell, sessionUpdate="tool_call_update",
                                status="completed",
                                content=text_content((done.stdout + done.stderr).strip())))
    turn_done(3_100)

    time.sleep(0.4)
    say(session_id,
        "Done: `reorder_rows` now returns a fifth column, `critical`, true when an item's "
        "stock is at or below a quarter of its reorder point. The test suite passes.\n\n"
        "- `inventory/report.py` — the new column\n"
        "- the CSV export writes every column, so it carries the flag unchanged")
    send({"id": request_id, "result": {
        "stopReason": "end_turn",
        "usage": {"inputTokens": 56_200, "outputTokens": 1_870, "cachedReadTokens": 41_300,
                  "thoughtTokens": 980, "totalTokens": 58_070},
    }})


def hit_limit(session_id, request_id):
    time.sleep(1.2)
    send({"id": request_id, "error": {"code": 429, "message": LIMIT}})


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
            "authMethods": [
                {"id": "oauth-personal", "name": "Log in with Google"},
                {"id": "gemini-api-key", "name": "Gemini API key"},
                {"id": "vertex-ai", "name": "Vertex AI"},
            ],
            "agentInfo": {"name": "gemini-cli", "title": "Gemini CLI", "version": "0.0.0-demo"},
            "agentCapabilities": {
                "loadSession": True,
                "promptCapabilities": {"image": True, "audio": True, "embeddedContext": True},
                "mcpCapabilities": {"http": True, "sse": True},
            },
        }})
    elif method == "session/new":
        state["cwd"] = params.get("cwd") or "."
        send({"id": request_id, "result": {
            "sessionId": "demo-session",
            "modes": {
                "availableModes": [{"id": i, "name": n, "description": d} for i, n, d in MODES],
                "currentModeId": state["mode"],
            },
            "models": {
                "availableModels": [{"modelId": i, "name": n} for i, n in MODELS],
                "currentModelId": state["model"],
            },
        }})
    elif method == "session/set_mode":
        state["mode"] = params.get("modeId")
        send({"id": request_id, "result": {}})
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
