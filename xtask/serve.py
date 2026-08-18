"""The static server behind `cargo xtask web`.

`python3 -m http.server` would almost do, but it sends no cache headers at all,
so a browser applies *heuristic* freshness and happily reuses a stale
`editor.js` for minutes after you edit it — the dev loop then shows you the
previous build with no sign that it did. Everything served here is a build
output being iterated on, so nothing should be cached, ever.

Usage: serve.py <port> <document-root>
"""

import sys
from functools import partial
from http.server import HTTPServer, SimpleHTTPRequestHandler


class NoCacheHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store, must-revalidate")
        super().end_headers()


port, root = int(sys.argv[1]), sys.argv[2]
handler = partial(NoCacheHandler, directory=root)
HTTPServer(("127.0.0.1", port), handler).serve_forever()
