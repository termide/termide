#!/usr/bin/env python3
"""Deterministic OpenAI-compatible endpoint for the screenshot recordings.

Serves `GET /v1/models` and a streamed `POST /v1/chat/completions` that plays
a fixed conversation: the step is chosen by how many tool results the request
already carries, so every recording sees the same turns in the same order.
Standard library only; listens on loopback and never makes a request itself.

The second argument picks the server it stands in for (`server.py PORT
FLAVOR`): the default reports its window as vLLM does; `llamacpp` reports
none, names its one model by its `--alias`, and streams its prompt-processing
progress when asked (`return_progress`), its cache growing turn by turn;
`omlx` lists several models with their windows and streams the progress too;
`openrouter` lists a gateway's models with their names and windows
(`context_length`), streams the reasoning as `reasoning`, has no progress to
give, and caches Anthropic's models only when the request asks
(`cache_control`), as OpenRouter does. OpenRouter is reached by name, so its recordings map `openrouter.ai`
to loopback and serve it on port 80 (see env/local-server-scene.sh).
"""

import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL = "qwen3-coder-30b-a3b"
CONTEXT_WINDOW = 65536
FLAVOR = "default"

# The models each flavor lists; the first is the one a request without a
# model gets. llama.cpp serves the one model it loaded; omlx every one in its
# model directory, with its window.
MODELS = {
    "default": [{"id": MODEL, "object": "model", "max_model_len": CONTEXT_WINDOW}],
    "llamacpp": [
        {
            "id": "qwen3.6-35b-a3b",
            "object": "model",
            "owned_by": "llamacpp",
            "meta": {"n_ctx_train": 262144},
        }
    ],
    "omlx": [
        {"id": m, "object": "model", "owned_by": "omlx", "max_model_len": 262144}
        for m in (
            "Qwen3.8-Flash-Next-oQ4e-mtp",
            "Qwen3.8-27B-oQ6",
            "GLM-5.2-Air-oQ4",
            "gemma-4-31b-it-oQ4",
        )
    ],
    "openrouter": [
        {"id": i, "name": n, "context_length": w}
        for i, n, w in (
            ("anthropic/claude-haiku-5", "Anthropic: Claude Haiku 5", 200000),
            ("anthropic/claude-opus-5.5", "Anthropic: Claude Opus 5.5", 1000000),
            ("anthropic/claude-sonnet-5", "Anthropic: Claude Sonnet 5", 1000000),
            ("deepseek/deepseek-v4", "DeepSeek: DeepSeek V4", 163840),
            ("google/gemini-3.5-flash", "Google: Gemini 3.5 Flash", 1048576),
            ("google/gemini-4-pro-preview", "Google: Gemini 4 Pro Preview", 1048576),
            ("moonshotai/kimi-k3", "MoonshotAI: Kimi K3", 262144),
            ("openai/gpt-6-luna", "OpenAI: GPT-6 Luna", 400000),
            ("openai/gpt-6.1-sol", "OpenAI: GPT-6.1 Sol", 400000),
            ("qwen/qwen3.6-35b-a3b", "Qwen: Qwen3.6 35B A3B", 262144),
            ("qwen/qwen3.8-coder-plus", "Qwen: Qwen3.8 Coder Plus", 1000000),
            ("z-ai/glm-5.2", "Z.ai: GLM 5.2", 202752),
        )
    ],
}

# The prompt a fresh session sends (system prompt and tool schemas), and the
# prefill speed the progress is paced at, in tokens per second.
PROMPT_BASE = 9400
PREFILL_SPEED = 2600

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
        "tool": ("shell", {"command": "python3 -m unittest -q"}),
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
            self.send_json({"object": "list", "data": MODELS[FLAVOR]})
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
        kwargs = request.get("chat_template_kwargs") or {}
        if kwargs.get("enable_thinking") is False:
            step = dict(step, reasoning="")
        self.model = request.get("model") or MODELS[FLAVOR][0]["id"]
        self.caches = not (
            FLAVOR == "openrouter"
            and self.model.startswith("anthropic/")
            and "cache_control" not in request
        )
        self.stream(step, messages, request.get("return_progress") is True)

    def chunk(self, delta=None, finish=None, usage=None, progress=None):
        obj = {
            "id": "chatcmpl-demo",
            "object": "chat.completion.chunk",
            "created": 1767225600,
            "model": getattr(self, "model", MODEL),
            "choices": [] if usage else [{"index": 0, "delta": delta or {}, "finish_reason": finish}],
        }
        if usage:
            obj["usage"] = usage
        if progress:
            obj["prompt_progress"] = progress
        data = ("data: " + json.dumps(obj) + "\n\n").encode()
        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
        self.wfile.flush()

    def stream(self, step, messages, progress):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        prompt = PROMPT_BASE + 180 * len(messages)
        # What the previous request of the conversation left in the cache: all
        # of it but the messages added since.
        cache = PROMPT_BASE + 180 * max(len(messages) - 2, 0) if len(messages) > 2 else 0
        if not self.caches:
            cache = 0
        if FLAVOR == "default":
            prompt, cache = 2400 + 180 * len(messages), 0
            time.sleep(PREFILL_DELAY)
        elif FLAVOR == "openrouter":
            time.sleep(PREFILL_DELAY)
        else:
            started = time.monotonic()
            processed = cache
            while processed < prompt:
                time.sleep(0.25)
                processed = min(prompt, processed + PREFILL_SPEED // 4)
                if progress:
                    elapsed = int((time.monotonic() - started) * 1000)
                    self.chunk(
                        {},
                        progress={
                            "total": prompt,
                            "cache": cache,
                            "processed": processed,
                            "time_ms": elapsed,
                        },
                    )
        produced = 0
        self.chunk({"role": "assistant", "content": ""})
        reasoning = "reasoning" if FLAVOR == "openrouter" else "reasoning_content"
        for piece in words(step["reasoning"]):
            self.chunk({reasoning: piece})
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
        usage = {"prompt_tokens": prompt, "completion_tokens": produced * 2}
        if cache:
            usage["prompt_tokens_details"] = {"cached_tokens": cache}
        self.chunk(usage=usage)
        data = b"data: [DONE]\n\n"
        self.wfile.write(b"%x\r\n%s\r\n0\r\n\r\n" % (len(data), data))
        self.wfile.flush()


def main():
    global FLAVOR
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 10000
    FLAVOR = sys.argv[2] if len(sys.argv) > 2 else "default"
    if FLAVOR not in MODELS:
        sys.exit("mock-llm: flavor default, llamacpp, omlx or openrouter, not %r" % FLAVOR)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
