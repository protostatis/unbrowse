#!/usr/bin/env python3
"""Smoke test cheap-first discovery routing decisions.

This covers the benchmark failure modes without live network dependencies:
- usable page with search-like text and links must not browser-route;
- dictionary/search form must expose a query URL and not browser-route;
- enable-JS shell must browser-route;
- AWS WAF-like interstitial must challenge-route.
- PerimeterX-like interstitial must keep the PX vendor/cookie hint even when
  generic human-verification copy is present.
- same-origin filtering that removes all routes must return a no-routes signal.
"""
from __future__ import annotations

import http.server
import json
import os
import socketserver
import subprocess
import threading
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


def _resolve_bin() -> Path:
    env = os.environ.get("UNBROWSER_BIN")
    if env:
        return Path(env)
    rel = REPO / "target" / "release" / "unbrowser"
    if rel.exists():
        return rel
    return REPO / "target" / "debug" / "unbrowser"


BIN = _resolve_bin()

USABLE_HTML = """<!doctype html><html><head><title>Usable News</title></head><body>
  <header><a href="/news">News</a><a href="/search">Search</a></header>
  <main>
    <h1>Usable News</h1>
    <p>Search our archive or continue reading these articles.</p>
    <article><a href="/news/climate-guide">What is climate change? A simple guide</a></article>
    <article><a href="/news/energy">Energy transition explained</a></article>
  </main>
</body></html>"""

DICTIONARY_HTML = """<!doctype html><html><head><title>Dictionary</title></head><body>
  <main>
    <h1>Dictionary</h1>
    <form action="/search/direct/" method="get">
      <label for="q">Search dictionary</label>
      <input id="q" name="q" type="search" placeholder="Search English">
      <button type="submit">Search</button>
    </form>
    <a href="/dictionary/english/">English Dictionary</a>
  </main>
</body></html>"""

SHELL_HTML = """<!doctype html><html><head><title>Enable JavaScript</title></head><body>
  <main>Please enable JavaScript to continue. This application requires JavaScript to be enabled.</main>
</body></html>"""

EXTERNAL_ONLY_HTML = """<!doctype html><html><head><title>External Only</title></head><body>
  <main>
    <h1>External Only</h1>
    <p>This page links out but has no page-owned route surface.</p>
    <a href="https://external.example/docs">External docs</a>
  </main>
</body></html>"""

AWS_WAF_HTML = """<!doctype html><html><head><title>Checking</title></head><body>
  <script>window.awsWafCookieDomainList = [];</script>
  <script src="/awswaf/challenge.js"></script>
</body></html>"""

PERIMETERX_HTML = """<!doctype html><html><head><title>Access Denied</title>
  <script>window._pxAppId = "PXfake";</script>
  <script src="/_px/PXfake/init.js"></script>
</head><body>
  <div id="px-captcha"></div>
  <h1>Are you a human?</h1>
  <p>Pardon our interruption.</p>
</body></html>"""


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        status = 200
        if self.path.startswith("/usable"):
            body = USABLE_HTML
        elif self.path.startswith("/dictionary"):
            body = DICTIONARY_HTML
        elif self.path.startswith("/shell"):
            body = SHELL_HTML
        elif self.path.startswith("/external-only"):
            body = EXTERNAL_ONLY_HTML
        elif self.path.startswith("/aws"):
            # Synthetic AWS WAF smoke: real AWS WAF JavaScript challenges often
            # use 202 Accepted while the browser computes/replays a token.
            status = 202
            body = AWS_WAF_HTML
        elif self.path.startswith("/perimeterx"):
            status = 403
            body = PERIMETERX_HTML
        else:
            body = "<!doctype html><title>OK</title><h1>OK</h1>"
        payload = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_):
        pass


class Unbrowser:
    def __init__(self):
        self.proc = subprocess.Popen(
            [str(BIN)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self.next_id = 0

    def call(self, method: str, **params):
        assert self.proc.stdin is not None
        assert self.proc.stdout is not None
        self.next_id += 1
        self.proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
            + "\n"
        )
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise AssertionError("unbrowser exited without response")
        out = json.loads(line)
        if "error" in out:
            raise AssertionError(out["error"])
        return out.get("result")

    def close(self):
        try:
            self.call("close")
        except Exception:
            pass
        try:
            self.proc.communicate(timeout=2)
        except Exception:
            self.proc.kill()


def check(label: str, condition: bool) -> bool:
    status = "PASS" if condition else "FAIL"
    print(f"  {status}  {label}")
    return condition


def top_tools(discovery) -> list[str]:
    nav = discovery.get("navigate_summary") or discovery.get("navigate") or {}
    return nav.get("tool_recommendations") or []


def main() -> int:
    httpd = socketserver.TCPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{httpd.server_address[1]}"
    ok = True

    ub = Unbrowser()
    try:
        usable = ub.call(
            "discover",
            url=base + "/usable",
            goal="find climate change guide",
            same_origin=True,
            limit=20,
        )
        ok &= check("usable page is not browser-routed", usable.get("escalations") == [])
        ok &= check("usable page discovers routes", usable.get("summary", {}).get("routes", 0) >= 2)
        ok &= check("usable page does not top-rank Chrome", top_tools(usable)[0] != "chrome_escalation")

        dictionary = ub.call(
            "discover",
            url=base + "/dictionary",
            goal="look up sustainability definition",
            same_origin=True,
            limit=20,
        )
        forms = dictionary.get("forms") or []
        query_urls = [f.get("query_url") for f in forms if f.get("query_url")]
        ok &= check("dictionary page is not browser-routed", dictionary.get("escalations") == [])
        ok &= check("dictionary form exposes query_url", any("sustainability" in u for u in query_urls))
        ok &= check("route_discover is recommended before Chrome", "route_discover" in top_tools(dictionary)[:6] and top_tools(dictionary)[0] != "chrome_escalation")

        shell = ub.call("discover", url=base + "/shell", goal="continue", limit=20)
        shell_reasons = [e.get("reason") for e in shell.get("escalations") or []]
        ok &= check("enable-JS shell browser-routes", "enable_js_interstitial" in shell_reasons)
        ok &= check("enable-JS shell top-ranks Chrome", top_tools(shell)[0] == "chrome_escalation")

        external_only = ub.call("discover", url=base + "/external-only", same_origin=True, limit=20)
        external_reasons = [e.get("reason") for e in external_only.get("escalations") or []]
        ok &= check("same-origin empty result signals no routes", "no_routes_found" in external_reasons)

        waf = ub.call("discover", url=base + "/aws", goal="search controller", limit=20)
        challenge = (waf.get("navigate_summary") or waf.get("navigate") or {}).get("challenge") or {}
        ok &= check("AWS WAF page challenge-routes", challenge.get("provider") == "aws_waf")
        ok &= check("AWS WAF top-ranks Chrome", top_tools(waf)[0] == "chrome_escalation")

        px = ub.call("discover", url=base + "/perimeterx", goal="view products", limit=20)
        challenge = (px.get("navigate_summary") or px.get("navigate") or {}).get("challenge") or {}
        ok &= check("PerimeterX page keeps PX provider", challenge.get("provider") == "perimeterx_block")
        ok &= check("PerimeterX exposes clearance cookie hint", challenge.get("clearance_cookie") == "_px3")
        ok &= check("PerimeterX top-ranks Chrome", top_tools(px)[0] == "chrome_escalation")
    finally:
        ub.close()
        httpd.shutdown()
        httpd.server_close()

    print("ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
