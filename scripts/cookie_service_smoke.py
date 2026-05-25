#!/usr/bin/env python3
"""Smoke test for the local cookie service + router replay path.

This uses only local fixtures:
  fake PX block -> headless unchained fetches _px3 -> router replays cookie
"""
from __future__ import annotations

import http.server
import json
import shutil
import socket
import socketserver
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "scripts"))

from router import Router, RouterConfig  # noqa: E402
from cookie_service import SolveError, _is_private_or_reserved_host, _validate_allow_hosts  # noqa: E402


BIN = REPO / "target" / "debug" / "unbrowser"

PX_BLOCK = """<!doctype html><html><head><title>Access Denied</title>
<script>window._pxAppId = "PXfake";</script>
<script src="/_px/PXfake/init.js"></script>
</head><body>
<div id="px-captcha"></div>
<h1>Robot or human?</h1>
<p>Fake PerimeterX gate. Waiting for browser sensor.</p>
</body></html>"""

PX_JS = """(() => {
  const payload = {
    appId: window._pxAppId,
    webdriver: navigator.webdriver,
    language: navigator.language,
    ts: Date.now()
  };
  fetch('/_px/collector', {
    method: 'POST',
    credentials: 'same-origin',
    headers: {'content-type': 'application/json'},
    body: JSON.stringify(payload)
  }).then(() => setTimeout(() => location.reload(), 100));
})();"""

OK = """<!doctype html><html><head><title>PX Cleared</title></head><body>
<main><h1>Protected content</h1><p>Fake _px3 clearance accepted.</p></main>
</body></html>"""

collector_payloads: list[dict] = []


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/_px/PXfake/init.js"):
            body = PX_JS.encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/javascript; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        cookie = self.headers.get("Cookie", "")
        if self.path.startswith("/protected") and "_px3=fake-clearance" in cookie:
            status, html = 200, OK
        elif self.path.startswith("/protected"):
            status, html = 403, PX_BLOCK
        else:
            status, html = 200, "<!doctype html><title>OK</title><h1>OK</h1>"
        body = html.encode()
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.path.startswith("/_px/collector"):
            n = int(self.headers.get("Content-Length", "0") or "0")
            raw = self.rfile.read(n).decode("utf-8", "replace")
            try:
                collector_payloads.append(json.loads(raw))
            except json.JSONDecodeError:
                collector_payloads.append({"raw": raw})
            self.send_response(204)
            self.send_header("Set-Cookie", "_px3=fake-clearance; Path=/; SameSite=Lax")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, *_):
        pass


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def wait_health(url: str, timeout: float = 10.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url + "/healthz", timeout=1) as resp:
                if resp.status == 200:
                    return
        except Exception:
            time.sleep(0.1)
    raise AssertionError("cookie service did not become healthy")


def get_json(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=5) as resp:
        return json.loads(resp.read().decode("utf-8"))


def check(label: str, condition: bool) -> bool:
    status = "PASS" if condition else "FAIL"
    print(f"  {status}  {label}")
    return condition


def main() -> int:
    ok = True
    ok &= check("localhost is private by default", _is_private_or_reserved_host("localhost"))
    ok &= check("loopback is private by default", _is_private_or_reserved_host("127.0.0.1"))
    ok &= check("loopback allow-host is accepted", _validate_allow_hosts(["127.0.0.1"]) == ["127.0.0.1"])
    try:
        _validate_allow_hosts(["com"])
        broad_rejected = False
    except SolveError:
        broad_rejected = True
    ok &= check("broad allow-host is rejected", broad_rejected)

    if not BIN.exists():
        print(f"SKIP: missing debug binary at {BIN}; run cargo build")
        return 0 if ok else 1
    if shutil.which("unchained") is None:
        print("SKIP: unchained CLI is not installed")
        return 0 if ok else 1

    httpd = socketserver.TCPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    target = f"http://127.0.0.1:{httpd.server_address[1]}/protected"

    service_port = free_port()
    cdp_port = free_port()
    profile = f"px-service-smoke-{service_port}-{cdp_port}"
    service_url = f"http://127.0.0.1:{service_port}"
    service = subprocess.Popen(
        [
            sys.executable,
            str(REPO / "scripts" / "cookie_service.py"),
            "--port",
            str(service_port),
            "--cdp-port",
            str(cdp_port),
            "--profile",
            profile,
            "--headless",
            "--no-keep-chrome",
            "--max-wait-seconds",
            "10",
            "--allow-host",
            "127.0.0.1",
            "--quiet",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )

    try:
        wait_health(service_url)
        caps = get_json(service_url + "/.well-known/unbrowser-cookie-solver")
        ready = get_json(service_url + "/readyz")
        ok &= check("capabilities include protocol_version", caps.get("protocol_version") == 1)
        ok &= check("service readiness is ok", ready.get("ok") is True)
        with Router(
            RouterConfig(
                binary=str(BIN),
                cookie_service_url=service_url,
                cookie_service_timeout=30,
                verbose=False,
            )
        ) as router:
            result = router.navigate(target)
        bm = result.get("blockmap") or {}
        ok &= check("router returns cleared page", result.get("status") == 200)
        ok &= check("challenge is gone after replay", result.get("challenge") is None)
        ok &= check("cleared title returned", bm.get("title") == "PX Cleared")
        ok &= check("headless Chrome sent fake PX payload", len(collector_payloads) == 1)
        if collector_payloads:
            ok &= check("stealth hid webdriver", collector_payloads[0].get("webdriver") is False)

        collector_payloads.clear()
        with Router(
            RouterConfig(
                binary=str(BIN),
                cookie_service_timeout=30,
                auto_start_cookie_service=True,
                verbose=False,
            )
        ) as router:
            result = router.navigate(target)
        bm = result.get("blockmap") or {}
        ok &= check("router auto-start returns cleared page", result.get("status") == 200)
        ok &= check("auto-start challenge is gone", result.get("challenge") is None)
        ok &= check("auto-start title returned", bm.get("title") == "PX Cleared")
    finally:
        service.terminate()
        try:
            service.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            service.kill()
            service.wait(timeout=5)
        subprocess.run(["unchained", "--port", str(cdp_port), "kill"], capture_output=True, timeout=10)
        httpd.shutdown()
        httpd.server_close()

    print("ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
