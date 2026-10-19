"""A small library for termide plugins in Python: the JSON-RPC on stdin and
stdout, and the protocol of doc/en/plugins.md as functions and decorators.

    from termide_plugin import Plugin

    plugin = Plugin()

    @plugin.command("hello", "Say hello", key="Ctrl+Alt+H")
    def hello():
        plugin.notify("Hello from " + plugin.name)

    plugin.run()

Handlers run on a pool of threads, so one may call termide and wait for the
answer (`plugin.read(path)`, `plugin.confirm(...)`) while termide goes on
talking to the plugin. Standard library only; Python 3.8 or later.
"""

import json
import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor

__all__ = ["Plugin", "RequestError", "utf16_len"]


def utf16_len(text):
    """`text`'s length as termide counts columns (UTF-16 code units, as LSP
    does): what `col` and `end_col` take."""
    return len(text.encode("utf-16-le")) // 2


class RequestError(Exception):
    """termide answered a request with an error."""

    def __init__(self, code, message):
        super().__init__(f"{message} (code {code})")
        self.code = code
        self.message = message


class Plugin:
    """One plugin process. Declare what it does with the decorators, then
    call `run()`; after `initialize`, `name`, `project_root` and `settings`
    say where it runs."""

    def __init__(self, workers=4):
        self.name = None
        self.project_root = None
        self.settings = {}
        self._events = {}
        self._commands = {}
        self._tools = {}
        self._providers = {}
        self._handlers = {}
        self._panel_actions = {}
        self._will_save = None
        self._on_start = []
        self._lock = threading.Lock()
        self._pending = {}
        self._next = 0
        self._pool = ThreadPoolExecutor(max_workers=workers)

    # ----------------------------------------------------------- declaring

    def on(self, event):
        """Handle an event (`file/saved`, `editor/changed`, `git/changed`, …)
        with `handler(params)`; the plugin subscribes to it."""

        def register(handler):
            self._events.setdefault(event, []).append(handler)
            return handler

        return register

    def on_start(self, handler):
        """Run `handler()` once termide has taken the handshake."""
        self._on_start.append(handler)
        return handler

    def command(self, id, title, key=None):
        """A command for the palette and the Commands menu; `handler()`."""

        def register(handler):
            entry = {"id": id, "title": title}
            if key:
                entry["key"] = key
            self._commands[id] = (entry, handler)
            return handler

        return register

    def tool(self, name, description, schema=None, read_only=False):
        """A tool for the agent: `handler(arguments, progress)` returns the
        text; raising makes it an error. `progress(text)` shows the output so
        far."""

        def register(handler):
            entry = {
                "name": name,
                "description": description,
                "inputSchema": schema or {"type": "object", "properties": {}},
                "readOnly": read_only,
            }
            self._tools[name] = (entry, handler)
            return handler

        return register

    def completion(self, languages):
        """`handler(path, line, col)` returns completion items (dicts with
        `label`, `insertText`, …) for files in `languages` (`"*"` for all)."""
        return self._provider("completion", languages)

    def hover(self, languages):
        """`handler(path, line, col)` returns Markdown, or None."""
        return self._provider("hover", languages)

    def code_actions(self, languages):
        """`handler(path, range, diagnostics)` returns fixes:
        `{"title": ..., "edits": [...]}`."""
        return self._provider("codeActions", languages)

    def definition(self, languages):
        """`handler(path, line, col)` returns places: `{"path", "line", "col"}`."""
        return self._provider("definition", languages)

    def references(self, languages):
        """`handler(path, line, col)` returns places, as `definition`."""
        return self._provider("references", languages)

    def will_save(self, handler):
        """`handler(path, version)` returns edits to make before `Ctrl+S`
        writes the file, or None."""
        self._will_save = handler
        return handler

    def _provider(self, kind, languages):
        def register(handler):
            self._providers[kind] = (list(languages), handler)
            return handler

        return register

    # ----------------------------------------------------------- talking

    def request(self, method, params=None, timeout=None):
        """Ask termide and wait for the answer; raises `RequestError`."""
        with self._lock:
            self._next += 1
            rid = f"p{self._next}"
            done = threading.Event()
            self._pending[rid] = [done, None]
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})
        if not done.wait(timeout):
            with self._lock:
                self._pending.pop(rid, None)
            raise RequestError(-1, f"{method}: no answer")
        with self._lock:
            reply = self._pending.pop(rid)[1]
        if "error" in reply:
            raise RequestError(reply["error"].get("code"), reply["error"].get("message"))
        return reply.get("result")

    def send_notification(self, method, params=None):
        self._send({"jsonrpc": "2.0", "method": method, "params": params or {}})

    # The protocol's requests, by name.

    def notify(self, message, level="info"):
        return self.request("window/notify", {"message": message, "level": level})

    def status(self, id, text):
        """Show `text` in the status bar under `id`; an empty text removes it."""
        return self.request("status/set", {"id": id, "text": text})

    def active(self):
        return self.request("editor/active")

    def read(self, path):
        return self.request("editor/read", {"path": path})

    def edit(self, path, edits, version=None):
        params = {"path": path, "edits": edits}
        if version is not None:
            params["version"] = version
        return self.request("editor/edit", params)

    def open_file(self, path, line=0, col=0):
        return self.request("editor/open", {"path": path, "line": line, "col": col})

    def diagnostics(self, path, items):
        return self.request("editor/diagnostics", {"path": path, "items": items})

    def run_in_terminal(self, command, cwd=None):
        params = {"command": command}
        if cwd:
            params["cwd"] = cwd
        return self.request("terminal/run", params)

    def open_panel(self, title, markdown, menu=None, on_action=None):
        """Open a panel; `on_action(action)` is called for its `plugin:`
        links and menu entries. Returns the panel's id."""
        params = {"title": title, "markdown": markdown}
        if menu:
            params["menu"] = [{"label": label, "action": action} for label, action in menu]
        panel = self.request("panel/open", params)["panel"]
        if on_action:
            self._panel_actions[panel] = on_action
        return panel

    def update_panel(self, panel, markdown, title=None):
        params = {"panel": panel, "markdown": markdown}
        if title:
            params["title"] = title
        return self.request("panel/update", params)

    def confirm(self, title, message):
        return self.request("window/confirm", {"title": title, "message": message})

    def choose(self, title, items):
        return self.request("window/choose", {"title": title, "items": items})

    def ask(self, title, prompt, default=""):
        return self.request("window/input", {"title": title, "prompt": prompt, "default": default})

    # ----------------------------------------------------------- running

    def run(self):
        """Serve termide until it says `exit`."""
        out = sys.stdout
        self._out = out
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                message = json.loads(line)
            except ValueError:
                continue
            method, rid = message.get("method"), message.get("id")
            if method is None:
                with self._lock:
                    waiting = self._pending.get(rid)
                if waiting:
                    waiting[1] = message
                    waiting[0].set()
                continue
            if method == "exit":
                break
            if method == "initialize":
                self._initialize(rid, message.get("params") or {})
                continue
            self._pool.submit(self._dispatch, method, rid, message.get("params") or {})
        self._pool.shutdown(wait=False)
        os._exit(0)

    def _send(self, message):
        line = json.dumps(message) + "\n"
        with self._lock:
            sys.stdout.write(line)
            sys.stdout.flush()

    def _answer(self, rid, result=None, error=None):
        if rid is None:
            return
        if error is not None:
            self._send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": str(error)}})
        else:
            self._send({"jsonrpc": "2.0", "id": rid, "result": result})

    def _initialize(self, rid, params):
        self.name = (params.get("plugin") or {}).get("name")
        self.project_root = (params.get("project") or {}).get("root")
        self.settings = params.get("settings") or {}
        subscriptions = list(self._events)
        if self._will_save:
            subscriptions.append("file/will_save")
        providers = {kind: langs for kind, (langs, _) in self._providers.items()}
        self._answer(rid, {
            "subscriptions": subscriptions,
            "commands": [entry for entry, _ in self._commands.values()],
            "tools": [entry for entry, _ in self._tools.values()],
            "providers": providers,
        })

    def _provide(self, kind, *args):
        entry = self._providers.get(kind)
        return entry[1](*args) if entry else None

    def _dispatch(self, method, rid, params):
        try:
            result = self._handle(method, params)
            self._answer(rid, result)
        except Exception as error:  # an answer, not a crash
            self._answer(rid, error=error)

    def _handle(self, method, params):
        if method == "initialized":
            for handler in self._on_start:
                handler()
            return None
        if method == "shutdown":
            return None
        if method == "$/cancelRequest":
            return None
        if method in self._events:
            for handler in self._events[method]:
                handler(params)
            return None
        if method == "file/will_save" and self._will_save:
            edits = self._will_save(params["path"], params["version"])
            return {"edits": edits} if edits else None
        if method == "command/execute":
            self._commands[params["id"]][1]()
            return None
        if method == "tool/call":
            token = params.get("progress")

            def progress(text):
                if token is not None:
                    self.send_notification("tool/progress", {"token": token, "text": text})

            try:
                text = self._tools[params["name"]][1](params.get("arguments") or {}, progress)
                return {"text": str(text), "isError": False}
            except Exception as error:
                return {"text": str(error), "isError": True}
        if method == "panel/action":
            handler = self._panel_actions.get(params["panel"])
            if handler:
                handler(params["action"])
            return None
        if method == "panel/closed":
            self._panel_actions.pop(params["panel"], None)
            return None
        at = (params.get("path"), params.get("line"), params.get("col"))
        if method == "editor/completion":
            return {"items": self._provide("completion", *at) or []}
        if method == "editor/hover":
            markdown = self._provide("hover", *at)
            return {"markdown": markdown} if markdown else None
        if method == "editor/definition":
            return {"locations": self._provide("definition", *at) or []}
        if method == "editor/references":
            return {"locations": self._provide("references", *at) or []}
        if method == "editor/code_actions":
            span = {k: params.get(k) for k in ("line", "col", "end_line", "end_col")}
            actions = self._provide("codeActions", params.get("path"), span, params.get("diagnostics") or [])
            return {"actions": actions or []}
        return None
