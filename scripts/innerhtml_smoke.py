#!/usr/bin/env python3
"""Smoke test for HTML fragment parsing — innerHTML + insertAdjacentHTML.

Synthetic page mutates the DOM via:
  - innerHTML: replaces a container's children with a parsed fragment
  - insertAdjacentHTML: each of the 4 positions
Then queries the resulting DOM via the same RPC API a driver would.
"""
import http.server, json, os, socketserver, subprocess, threading
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

HTML = """<!DOCTYPE html>
<html><body>
<div id="container">existing</div>
<div id="anchor">anchor</div>
<script>
  // 1. innerHTML replace
  document.getElementById('container').innerHTML =
    '<p class="inner-p">parsed para</p><span data-x="42">parsed span</span>';

  // 2. insertAdjacentHTML — all four positions relative to #anchor
  var anchor = document.getElementById('anchor');
  anchor.insertAdjacentHTML('beforebegin',
    '<div class="adj" id="adj-beforebegin">bb</div>');
  anchor.insertAdjacentHTML('afterbegin',
    '<span class="adj" id="adj-afterbegin">ab</span>');
  anchor.insertAdjacentHTML('beforeend',
    '<span class="adj" id="adj-beforeend">be</span>');
  anchor.insertAdjacentHTML('afterend',
    '<div class="adj" id="adj-afterend">ae</div>');

  // 3. innerHTML with nested elements + attributes
  document.body.insertAdjacentHTML('beforeend',
    '<section id="nested"><h2>hdr</h2><ul><li class="i">a</li><li class="i">b</li></ul></section>');
</script>
</body></html>
"""


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        b = HTML.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def log_message(self, *_): pass


def main():
    httpd = socketserver.TCPServer(("127.0.0.1", 0), H)
    port = httpd.server_address[1]
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{port}/"
    print(f"server: {base}")

    p = subprocess.Popen([str(BIN)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL, text=True)

    def call(method, **params):
        msg = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
        p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()
        return json.loads(p.stdout.readline())

    nav = call("navigate", url=base, exec_scripts=True)
    print(f"navigate status={nav.get('result', {}).get('status')}")

    # 1. innerHTML replaced container — should have <p class="inner-p"> and <span data-x>
    inner_p = call("query", selector="#container .inner-p").get("result", [])
    inner_span = call("query", selector="#container span[data-x='42']").get("result", [])

    # 2. All four insertAdjacentHTML targets exist with right tags + classes
    bb = call("query", selector="#adj-beforebegin").get("result", [])
    ab = call("query", selector="#anchor #adj-afterbegin").get("result", [])
    be = call("query", selector="#anchor #adj-beforeend").get("result", [])
    ae = call("query", selector="#adj-afterend").get("result", [])
    all_adj = call("query", selector=".adj").get("result", [])

    # 3. Nested fragment built correctly
    nested = call("query", selector="#nested h2").get("result", [])
    nested_li = call("query", selector="#nested ul li.i").get("result", [])

    call("close")
    p.communicate(timeout=2)
    httpd.shutdown()

    print()
    ok = True

    def check(label, results, expected):
        global_ok = True
        actual = len(results)
        status = "PASS" if actual == expected else "FAIL"
        print(f"  {status}  {label}: got {actual}, expected {expected}")
        return actual == expected

    if not check("#container .inner-p (innerHTML)", inner_p, 1): ok = False
    if not check("#container span[data-x='42'] (innerHTML attrs preserved)", inner_span, 1): ok = False
    if not check("#adj-beforebegin (insertAdjacentHTML beforebegin)", bb, 1): ok = False
    if not check("#anchor > #adj-afterbegin (insertAdjacentHTML afterbegin)", ab, 1): ok = False
    if not check("#anchor > #adj-beforeend (insertAdjacentHTML beforeend)", be, 1): ok = False
    if not check("#adj-afterend (insertAdjacentHTML afterend)", ae, 1): ok = False
    if not check(".adj (all 4 insertions)", all_adj, 4): ok = False
    if not check("#nested h2 (nested fragment)", nested, 1): ok = False
    if not check("#nested ul li.i (nested list items)", nested_li, 2): ok = False

    print()
    print("ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
