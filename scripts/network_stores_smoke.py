#!/usr/bin/env python3
"""Smoke test for network_stores capture.

Two scenarios:
  1. Local synthetic — navigate to a JSON endpoint, verify capture
  2. Real-site — npm package page (Next.js + JSON route data), verify
     captures from the navigate response (the html itself isn't JSON,
     but a Next-data preload usually fires)

For the real-site scenario we just check shape and counts; we don't
assert specific URLs because they change with deploys.
"""
import http.server, json, socketserver, subprocess, threading
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
BIN = REPO / "target" / "release" / "unbrowser"

# --- Local scenario: serve a JSON endpoint at the navigate URL itself ---
JSON_BODY = json.dumps({
    "items": [{"id": i, "name": f"item-{i}"} for i in range(50)],
    "total": 50,
    "page": 1,
})


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path in ("/", "/api/items.json"):
            b = JSON_BODY.encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)
        else:
            self.send_response(404); self.end_headers()
    def log_message(self, *_): pass


def call(p, method, **params):
    msg = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
    p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()
    return json.loads(p.stdout.readline())


def scenario_local():
    print("\n=== Scenario 1: local JSON endpoint ===")
    httpd = socketserver.TCPServer(("127.0.0.1", 0), H)
    port = httpd.server_address[1]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{port}/api/items.json"

    p = subprocess.Popen([str(BIN)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL, text=True)
    nav = call(p, "navigate", url=base, exec_scripts=False)
    res = nav.get("result", {})
    ns_summary = res.get("network_stores")
    print(f"navigate result network_stores summary: {json.dumps(ns_summary, indent=2)[:400]}")

    full = call(p, "network_stores", limit=10)
    captures = full.get("result", []) or []
    print(f"network_stores RPC returned {len(captures)} captures")
    for c in captures:
        print(f"  [{c.get('kind'):>15s}] score={c.get('score'):3d} bytes={c.get('body_bytes'):6d} {c.get('url')}")

    call(p, "close")
    p.communicate(timeout=2)
    httpd.shutdown()

    ok = (
        ns_summary
        and ns_summary.get("count", 0) >= 1
        and len(captures) >= 1
        and any(c.get("kind") == "json" for c in captures)
    )
    print("PASS" if ok else "FAIL: expected ≥1 capture with kind=json")
    return ok


def scenario_real_json_endpoint():
    """A URL that IS a JSON document — exercises the navigate-body capture path."""
    print("\n=== Scenario 2: real JSON endpoint (GitHub API) ===")
    p = subprocess.Popen([str(BIN)],
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL, text=True)
    nav = call(p, "navigate",
              url="https://api.github.com/repos/anthropics/anthropic-sdk-python",
              exec_scripts=False)
    res = nav.get("result", {})
    ns_summary = res.get("network_stores", {}) or {}
    print(f"network_stores summary: count={ns_summary.get('count')} total_bytes={ns_summary.get('total_bytes')}")
    for t in (ns_summary.get("top") or [])[:3]:
        print(f"  [{t.get('kind'):>15s}] score={t.get('score'):3d} bytes={t.get('body_bytes'):8d} {t.get('url')[:80]}")

    full = call(p, "network_stores", limit=5)
    captures = full.get("result", []) or []
    print(f"\nfull captures: {len(captures)}")
    for c in captures:
        body_preview = (c.get("body") or "")[:60].replace("\n", " ")
        print(f"  [{c.get('kind'):>15s}] score={c.get('score')} bytes={c.get('body_bytes')}")
        print(f"                   body[:60]={body_preview!r}")

    call(p, "close")
    p.communicate(timeout=2)
    ok = ns_summary.get("count", 0) >= 1 and any(c.get("kind") == "json" for c in captures)
    print("PASS" if ok else "FAIL: expected ≥1 json capture from GitHub API")
    return ok


def scenario_spa_report_only():
    """A real SPA — captures vary by site behavior; report only, don't gate on count."""
    print("\n=== Scenario 3: real SPA (npm package page) — report-only ===")
    p = subprocess.Popen([str(BIN), "--policy=blocklist"],
                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL, text=True)
    nav = call(p, "navigate", url="https://www.npmjs.com/package/react", exec_scripts=True)
    res = nav.get("result", {})
    ns_summary = res.get("network_stores", {}) or {}
    print(f"network_stores summary: count={ns_summary.get('count')} total_bytes={ns_summary.get('total_bytes')}")
    for t in (ns_summary.get("top") or [])[:3]:
        print(f"  [{t.get('kind'):>15s}] score={t.get('score'):3d} bytes={t.get('body_bytes'):8d} {t.get('url')[:80]}")
    print("(report-only — no pass/fail gate; npm inlines data via __NEXT_DATA__ so captures may be 0)")
    call(p, "close")
    p.communicate(timeout=2)
    return True


def main():
    r1 = scenario_local()
    r2 = scenario_real_json_endpoint()
    scenario_spa_report_only()
    print()
    print("ALL PASS" if (r1 and r2) else "FAILURES")
    return 0 if (r1 and r2) else 1


if __name__ == "__main__":
    raise SystemExit(main())
