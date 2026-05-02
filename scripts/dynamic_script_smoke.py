#!/usr/bin/env python3
"""Smoke test for dynamic <script src> insertion.

Synthetic page: an inline bootstrap that creates a <script src=...>
and appendChild's it. We expect:
  - the dynamic script's body actually evaluates (variable set)
  - the script's onload callback runs (a marker is set)
  - tracker URLs (added to the policy blocklist) emit policy_blocked
    and STILL fire onload (so app boot doesn't stall)
"""
import http.server
import json
import socketserver
import subprocess
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
BIN = REPO / "target" / "release" / "unbrowser"

# --- Local synthetic test page ---
# Phases:
#   1. first-party JS at /sub/page.html → fetched via relative `../js/first.js`
#      (tests __host_resolve_url handling of `../` segments)
#   2. blocked tracker (Google Analytics) — must STILL fire load (stub_success)
#   3. broken script that throws at eval — must fire ERROR (not load) — PR #6
#      review HIGH: previously swallowed eval errors and fired load.
HTML = """<!DOCTYPE html>
<html><head><title>dyn</title></head>
<body><div id="boot-status">starting</div>
<script>
  function setStatus(s) { document.getElementById('boot-status').textContent = s; }
  function appendStatus(s) { setStatus(document.getElementById('boot-status').textContent + s); }

  // Phase 1: dynamically insert a first-party script. Use a relative path
  // with `../` to exercise URL resolution.
  var s = document.createElement('script');
  s.src = '../js/first.js';
  s.onload = function() {
    setStatus('first_party_loaded:' + (typeof window.__dyn_first_party));

    // Phase 2: blocked tracker. Load should fire (stub_success).
    var t = document.createElement('script');
    t.src = 'https://www.google-analytics.com/analytics.js';
    t.onload = function() {
      appendStatus('|tracker_load_fired');

      // Phase 3: a script that throws on eval. Error MUST fire (not load).
      var b = document.createElement('script');
      b.src = '/broken.js';
      b.onload = function() { appendStatus('|broken_load_BUG'); };
      b.onerror = function() { appendStatus('|broken_error_fired'); };
      document.head.appendChild(b);
    };
    t.onerror = function() { appendStatus('|tracker_error'); };
    document.head.appendChild(t);
  };
  s.onerror = function() { setStatus('first_party_error'); };
  document.head.appendChild(s);
</script>
</body></html>
"""

DYN_FIRST_PARTY_JS = """
window.__dyn_first_party = 'set_at_' + Date.now();
"""

DYN_BROKEN_JS = """
// Throws at eval — should cause the dynamic-script handler to dispatch error.
throw new Error('intentional eval failure');
"""


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        # HTML lives at /sub/page.html so the in-page `../js/first.js`
        # exercises real ../ resolution against base /sub/page.html.
        if self.path in ("/", "/sub/page.html"):
            body = HTML.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/js/first.js":
            body = DYN_FIRST_PARTY_JS.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/javascript")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/broken.js":
            body = DYN_BROKEN_JS.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/javascript")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, *_args):
        pass  # quiet


def serve():
    httpd = socketserver.TCPServer(("127.0.0.1", 0), Handler)
    port = httpd.server_address[1]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd, port


def main():
    httpd, port = serve()
    base = f"http://127.0.0.1:{port}/sub/page.html"
    print(f"server target: {base}")

    p = subprocess.Popen(
        [str(BIN), "--policy=blocklist"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True,
    )

    def call(method, **params):
        msg = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
        p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()
        return json.loads(p.stdout.readline())

    nav = call("navigate", url=base, exec_scripts=True)
    nav_result = nav.get("result", {})
    nav_id = nav_result.get("navigation_id")
    print(f"nav_id: {nav_id}, status: {nav_result.get('status')}")

    # Read boot-status text after settle
    txt = call("text", selector="#boot-status")
    boot_status = txt.get("result", "(none)")
    print(f"boot-status text: {boot_status!r}")

    # Inspect the global the dynamic script should have set.
    eval_result = call("eval", code="typeof window.__dyn_first_party")
    print(f"window.__dyn_first_party type: {eval_result.get('result')!r}")

    call("close")
    _, stderr = p.communicate(timeout=2)
    httpd.shutdown()

    events = [json.loads(l) for l in stderr.splitlines() if l.strip()]
    by_kind = {}
    for ev in events:
        by_kind.setdefault(ev["event"], []).append(ev["data"])

    print(f"\nEvents:")
    for k in sorted(by_kind):
        print(f"  {k:25s} {len(by_kind[k])}")

    blocked = by_kind.get("policy_blocked", [])
    if blocked:
        print(f"\nblocked URLs:")
        for b in blocked:
            print(f"  [{b.get('category', '?')}] {b.get('url', '?')}")

    # Verifications
    print()
    ok = True
    if "first_party_loaded:string" not in boot_status:
        print(f"FAIL: dynamic first-party script did not eval+fire onload")
        ok = False
    else:
        print("PASS: dynamic first-party script eval'd + onload fired")
    if "tracker_load_fired" not in boot_status:
        print(f"FAIL: blocked tracker did NOT fire load (stub_success broken)")
        ok = False
    else:
        print("PASS: blocked tracker fired load (stub_success works)")
    if not blocked or not any("google-analytics.com" in (b.get("url") or "") for b in blocked):
        print(f"FAIL: tracker URL was not blocked by policy")
        ok = False
    else:
        print("PASS: tracker URL was blocked by policy")
    if "broken_load_BUG" in boot_status:
        print(f"FAIL: broken script (eval throws) fired LOAD instead of ERROR")
        ok = False
    elif "broken_error_fired" not in boot_status:
        print(f"FAIL: broken script did not dispatch error (boot stalled or never reached phase 3)")
        ok = False
    else:
        print("PASS: eval-failing script correctly dispatched ERROR (not load)")

    print()
    print("ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
