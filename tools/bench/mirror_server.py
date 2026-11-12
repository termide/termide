"""Serve tools/bench/mirror/ to the verifiers, falling through to the network.

Local files are served as they are. A simple-index page (/simple/<project>/)
is PyPI's own, with the links to wheels mirror/wheels/ holds pointed at the
local copies: a task that pins another version, or a package the mirror does
not have, still installs, from PyPI. When PyPI does not answer, the local page
is served instead. PyTorch's projects come from its CPU-only index: the
verifiers have no GPU, and the default wheels pull gigabytes of CUDA libraries
that a slow line cannot fetch within a verifier's time limit (torch==2.7.0
matches 2.7.0+cpu). A Python or uv build the mirror lacks is redirected to its
GitHub release.

    python3 mirror_server.py <mirror-dir> <port> <bind>
"""

import functools
import html
import os
import re
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

PYPI = "https://pypi.org/simple/"
PYTORCH_CPU = "https://download.pytorch.org/whl/cpu/"
CPU_ONLY = {"torch", "torchvision", "torchaudio"}
GITHUB = {
    "/python/": "https://github.com/astral-sh/python-build-standalone/releases/download/",
    "/uv/": "https://github.com/astral-sh/uv/releases/download/",
}
ANCHOR = re.compile(r"<a\s[^>]*>")
HREF = re.compile(r'href="([^"]+)"')
# PyPI offers each file's metadata apart (<file>.metadata); the mirror has none.
METADATA = re.compile(r'\s+data-(?:dist-info|core)-metadata="[^"]*"')


def normalise(name):
    return re.sub(r"[-_.]+", "-", name).lower()


class Mirror(SimpleHTTPRequestHandler):
    pages = {}
    lock = threading.Lock()

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        match = re.fullmatch(r"/simple/([^/]+)/?", path)
        if match:
            return self.simple(normalise(match.group(1)))
        if not os.path.exists(self.translate_path(path)):
            for prefix, upstream in GITHUB.items():
                if path.startswith(prefix):
                    self.send_response(302)
                    self.send_header("Location", upstream + path[len(prefix):])
                    self.end_headers()
                    return
        return super().do_GET()

    def simple(self, project):
        with self.lock:
            page = self.pages.get(project)
        if page is None:
            page = self.from_pypi(project)
            if page is not None:
                with self.lock:
                    self.pages[project] = page
        if page is None:
            local = os.path.join(self.directory, "simple", project, "index.html")
            if not os.path.exists(local):
                self.send_error(404)
                return
            page = open(local, "rb").read()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(page)))
        self.end_headers()
        self.wfile.write(page)

    def from_pypi(self, project):
        index = PYTORCH_CPU if project in CPU_ONLY else PYPI
        request = urllib.request.Request(index + project + "/", headers={"Accept": "text/html"})
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                text = response.read().decode()
        except (urllib.error.URLError, OSError) as error:
            self.log_message("PyPI %s: %s", project, error)
            return None
        wheels = os.path.join(self.directory, "wheels")

        def local(anchor):
            tag = anchor.group(0)
            href = HREF.search(tag)
            if not href:
                return tag
            url = html.unescape(href.group(1))
            name = url.split("#", 1)[0].rsplit("/", 1)[-1]
            if not os.path.exists(os.path.join(wheels, name)):
                # PyTorch's index links relative to itself.
                absolute = urllib.parse.urljoin(index + project + "/", url)
                return tag.replace(href.group(0), f'href="{html.escape(absolute)}"')
            fragment = "#" + url.split("#", 1)[1] if "#" in url else ""
            tag = tag.replace(href.group(0), f'href="../../wheels/{name}{fragment}"')
            return METADATA.sub("", tag)

        return ANCHOR.sub(local, text).encode()


def main():
    directory, port, bind = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    handler = functools.partial(Mirror, directory=directory)
    server = ThreadingHTTPServer((bind, port), handler)
    print(f"Serving {directory} on {bind}:{port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
