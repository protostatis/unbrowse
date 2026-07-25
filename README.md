# unbrowser

**The agent-native browser. One static binary. No Chrome.**

[![unbrowser MCP server](https://glama.ai/mcp/servers/protostatis/unbrowser/badges/score.svg)](https://glama.ai/mcp/servers/protostatis/unbrowser)

`unbrowser` is the lightweight open-source browser tier from [Unchained](https://unchainedsky.com): a vertically integrated browser runtime and LLM interface for agents. It gives you stateful web access when `curl`/WebFetch is too dumb and full Chrome is too heavy: JavaScript execution, cookies, forms, clicks, challenge detection, and compact BlockMap output are designed to work together as one unit.

**See the runtime before installing:** [run the live public-web demo](https://unchainedsky.com/unbrowser?utm_source=github&utm_medium=repository&utm_campaign=unbrowser_readme&ref=readme_live_demo). No account is required. The demo accepts only the fixed public source sets shown on the page; do not use it for private data or authenticated browsing.

It is not Chrome made smaller. It is the browser interface rebuilt around what agents need before escalating to Chrome. When a page needs real pixels, V8 compatibility, extensions, or human-in-the-loop auth, escalate to [`unchainedsky-cli`](https://github.com/protostatis/unchainedsky-cli) or [Unchained](https://unchainedsky.com).

**Try it hosted:** Unchained exposes a public Streamable HTTP MCP endpoint at [`https://unchainedsky.com/unbrowser-mcp`](https://unchainedsky.com/unbrowser-mcp) for discovery and smoke tests. Glama also runs a hosted MCP release at [`glama.ai/mcp/servers/protostatis/unbrowser`](https://glama.ai/mcp/servers/protostatis/unbrowser), and the Smithery page is at [`smithery.ai/servers/protostatis-dev/unbrowser`](https://smithery.ai/servers/protostatis-dev/unbrowser). These hosted endpoints are shared infrastructure: do not send private cookies, secrets, or authenticated browsing tasks through them. For production workflows, install the local binary below so sessions and cookies stay on your machine.

### Install

**Python (recommended)** — wheel ships the native binary. **Requires Python 3.10+**:

```bash
pipx install pyunbrowser   # cleanest on macOS Homebrew / modern Linux (handles PEP 668)
pip  install pyunbrowser   # in a venv on python3.10+
```

> **macOS gotcha**: the system `/usr/bin/python3` is 3.9 and the wheel will reject it with "requires Python >=3.10". Use Homebrew's `python3.13` or `pipx` (which manages its own Python). If `pip install` fails with PEP 668 ("externally-managed-environment"), that's the same issue — `pipx install pyunbrowser` is the right call.

```python
from unbrowser import Client       # note: pip name is pyunbrowser, import is unbrowser
with Client() as ub:                # (PyPI's name moderation blocks 'unbrowser';
    r = ub.navigate("https://news.ycombinator.com")   # py- prefix is the standard workaround)
```

**Docker (Linux amd64/arm64)** — minimal distroless image, non-root, ~13 MiB pull:

```bash
docker pull ghcr.io/protostatis/unbrowser:latest

# One-shot navigation
docker run --rm ghcr.io/protostatis/unbrowser:latest \
  navigate https://example.com --json

# MCP over stdio (default)
docker run --rm -i ghcr.io/protostatis/unbrowser:latest
```

For an MCP host that launches Docker:

```json
{
  "mcpServers": {
    "unbrowser": {
      "command": "docker",
      "args": [
        "run", "--rm", "-i",
        "ghcr.io/protostatis/unbrowser:latest"
      ]
    }
  }
}
```

Pin `:vX.Y.Z` or an image digest in production. The image has no shell or package manager, and container state disappears when the container exits.

**Cargo** — binary only, no Python wrapper:

```bash
cargo install unbrowser
unbrowser --mcp
```

**MCP** — add the binary to Claude Code, Claude Desktop, Cursor, Cline, or any MCP host:

```json
{
  "mcpServers": {
    "unchained": {
      "command": "unbrowser",
      "args": ["--mcp"]
    }
  }
}
```

The `unchained` key is only the client-side alias. Use `unbrowser` if you want exact naming, or keep `unchained` as the breadcrumb to the full Unchained browser-agent stack.

**Hosted MCP smoke/discovery endpoint** — for MCP clients that support Streamable HTTP:

```json
{
  "mcpServers": {
    "unbrowser-hosted": {
      "url": "https://unchainedsky.com/unbrowser-mcp"
    }
  }
}
```

Use this hosted route to inspect tools or run public-page smoke tests. It is intentionally unauthenticated and SSRF-guarded, and it is not a place to replay private cookies or secrets.

**Pre-built tarball** — for systems without Python or Rust:

```bash
# macOS Apple Silicon
curl -L https://github.com/protostatis/unbrowser/releases/latest/download/unbrowser-aarch64-apple-darwin.tar.gz | tar xz
# macOS Intel
curl -L https://github.com/protostatis/unbrowser/releases/latest/download/unbrowser-x86_64-apple-darwin.tar.gz | tar xz
# Linux x86_64 (glibc 2.31+ / Ubuntu 20.04+)
curl -L https://github.com/protostatis/unbrowser/releases/latest/download/unbrowser-x86_64-unknown-linux-gnu.tar.gz | tar xz
```

**From source**:

```bash
cargo build --release   # binary at ./target/release/unbrowser
```

### Session CLI

For shell-only agents, use a persistent session instead of heredoc JSON-RPC:

```bash
unbrowser session start --id demo
unbrowser exec demo navigate https://news.ycombinator.com
unbrowser exec demo query '.titleline > a'
unbrowser exec --pretty demo blockmap
unbrowser session stop demo
```

### Bare RPC (low-level escape hatch)

```bash
echo '{"id":1,"method":"navigate","params":{"url":"https://news.ycombinator.com"}}' | unbrowser
```

That's the install. Runs anywhere a native binary runs — laptop, Lambda, Docker, CI, edge containers.

Open source under Apache 2.0. When the cheap path can't handle a page (heavy SPAs, behavioral bot challenges), escalate to a real browser via [`unchainedsky-cli`](https://github.com/protostatis/unchainedsky-cli) (drives your local Chrome via CDP) or the [Unchained desktop app](https://unchainedsky.com).

---

## By the numbers

|                | This binary    | Headless Chrome (Playwright/Puppeteer) |
|----------------|----------------|-----------------------------------------|
| Binary size    | **~10MB**      | 250MB+ Chrome download                  |
| RAM / session  | **~50MB**      | 200–500MB                                |
| Cold start     | **~100ms**     | ~1s                                      |
| Tokens / page (LLM) | **~500** (BlockMap inline) | tens of thousands of HTML, parsed by you |
| Install steps  | `cargo build`  | install Chrome + Node + Playwright + system deps |
| Lambda / Docker / edge containers | ✅      | ❌ Chrome too big                        |
| 100K pages/day cost | $0 (your infra) | $$$ Chrome fleet or hosted API     |

**5–10× lower memory, 25× smaller binary, 10× faster cold start, 70× lower per-page token cost.** That's the tradeoff this product makes — defer JS-rendering (Phase 4/5) and pixel rendering (out of scope) in exchange for a footprint that fits in places Chrome doesn't.

Need broader implementation context? Browse the [source-backed Browser Agents & MCP guide](https://searchagentsky.com/guides/browser-agents-mcp?utm_source=github&utm_medium=repository&utm_campaign=unbrowser_guide) from Search Agent Sky by Unchained.

## Agent-friendly by design

Most browser tools start with a human browser and bolt on an agent interface: Chrome, CDP, Playwright, screenshots, accessibility trees, or raw HTML. `unbrowser` takes the opposite path. The runtime, DOM model, interaction handles, policy hooks, challenge detector, and output format are co-designed for agents.

That vertical integration is the point: the browser can skip the rendering stack and return exactly what the model needs to orient, decide, and act.

- **`navigate` returns a BlockMap** — ~500 tokens of structured page summary (landmarks, headings, interactives, density signals) right in the response. No follow-up call needed to know what's on the page.
- **Stable element refs** (`e:142`) — query, click, type, submit using opaque handles. The LLM never has to scrape the DOM itself.
- **`challenge` field on every blocked navigate** — provider, confidence, and the exact clearance cookie name. The agent reacts intelligently instead of guessing.
- **`density.likely_js_filled` heuristic** — distinguishes "real SSR page" from "SSR shell with JS-filled cells" (the CNBC trap). The agent bails before burning round-trips on a page it can't read.
- **MCP-native** — `unbrowser --mcp` exposes the RPC tool surface to any MCP host (Claude Code, Claude Desktop, Cursor, Cline). 4 lines of config, zero glue code.
- **Real Chrome fingerprint** (Chrome 134 JA4 + Akamai H2 hash) so sites don't block you for being a script.

Use Chrome when you need pixels: screenshots, layout, canvas, WebGL, CAPTCHA solving, extensions, or maximum compatibility. Use `unbrowser` when your agent needs the page to run: JavaScript, cookies, links, forms, clicks, and a compact representation made for LLM context windows.

For pages that *do* need real Chrome (heavy SPAs, JS-challenge bot walls), the binary detects them and accepts cookies via `cookies_set` — so you solve once in Chrome and replay the session here for as long as the clearance cookie lasts.

## Quick demo — Hacker News top 3

```python
from unbrowser import Client

with Client() as ub:
    ub.navigate("https://news.ycombinator.com")
    for s in ub.query(".titleline > a")[:3]:
        print(s["text"], s["attrs"]["href"])
```

5 lines, no headless browser install. Output is structured JSON, not 35KB of HTML. The `Client` wrapper handles subprocess lifecycle (atexit reaper so orphans are impossible), JSON-RPC framing, and surfaces real exceptions instead of silent `result` lookups.

<details>
<summary>Bare-RPC version (if you can't use Python)</summary>

The same demo without the wrapper — useful for languages other than Python or multi-step sessions. The protocol is JSON-RPC over stdin/stdout, one JSON object per line:

```python
import subprocess, json
p = subprocess.Popen(["./target/release/unbrowser"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
i = 0
def call(method, **params):
    global i; i += 1
    p.stdin.write(json.dumps({"id": i, "method": method, "params": params}) + "\n")
    p.stdin.flush()
    return json.loads(p.stdout.readline())["result"]

call("navigate", url="https://news.ycombinator.com")
for s in call("query", selector=".titleline > a")[:3]:
    print(s["text"], s["attrs"]["href"])
```

That's the entire protocol surface. Same shape from any language with subprocess + JSON.

</details>

### One-shot CLI

For shell-friendly calls, use the convenience subcommand:

```bash
unbrowser navigate https://news.ycombinator.com --json
```

That prints one JSON result and exits from any install path (PyPI wheel, Cargo, or release tarball). Use JSON-RPC only when you need a persistent session. Run `unbrowser --help` for the native CLI surface.

### A/B runtime shims

For corpus tests against JS-heavy pages, compare the default stable shims with the opt-in enhanced browser-environment shims:

```bash
unbrowser navigate https://example.com --exec-scripts --json
unbrowser navigate https://example.com --exec-scripts --json --shims enhanced
# or for JSON-RPC / MCP sessions:
UNBROWSER_SHIMS=enhanced unbrowser
```

`enhanced` adds content-positive layout/media/scroll/IndexedDB guesses on top of the stable runtime. It is intentionally opt-in so A/B runs can measure whether more page state materializes without changing the baseline.

Script evaluation is still bounded by `UNBROWSER_SCRIPT_EVAL_BUDGET_MS` (default `5000`); navigate results report `scripts.budget_exhausted` and `scripts.budget_skipped` when the budget stops further script execution. The outer RPC watchdog (`UNBROWSER_TIMEOUT_MS`, default `30000`) still wins if it is lower than the script budget.

For a JSONL corpus sweep:

```bash
python3 scripts/shim_ab.py --url https://nextjs.org/docs --url https://www.npmjs.com/package/playwright
```

## SPA tier — what works, what doesn't

Empirical, not aspirational. Latest matrix: **28/30** on tested categories.

| Page tier | Coverage | What to expect |
|---|---|---|
| **Static + SSR** (Wikipedia, MDN, news, docs, GitHub repo browsing, search engines, archive.org) | ✅ excellent | sub-second navigate; full BlockMap; all selectors work; ~hundreds of tokens vs ~tens of KB raw |
| **SSR + light hydration** (Next.js docs, marketing pages, react.dev's *static* content) | ✅ usable | reads SSR'd content fine; hydration adds nothing but doesn't break either |
| **Bot-walled with cookie handoff** (Zillow, Cloudflare-protected sites) | ✅ via `cookies_set` | solve once in Chrome, replay forever; `challenge.provider` field tells the agent which vendor |
| **Module-loader SPAs** (Ember, AMD apps like crates.io) | ⚠️ partial with `exec_scripts: true` | bundles fetch + execute, modules register, but framework auto-mount needs case-by-case shimming |
| **Heavy React/Vue bundles** (react.dev runtime, large dashboard apps) | ⚠️ bounded — won't hang, won't render | with `exec_scripts: true` the navigate completes inside the 30s wall-clock budget (5s for the script-eval phase, the rest for settle); rendered DOM may not materialize. Tune via `UNBROWSER_TIMEOUT_MS` |
| **Apps requiring Workers / Canvas / IndexedDB / WebGL** | ❌ out of scope by design | use the cookie-handoff path with real Chrome via [`unchainedsky-cli`](https://github.com/protostatis/unchainedsky-cli) (CDP) or the [Unchained desktop app](https://unchainedsky.com) |
| **Hardest-tier anti-bot** (PerimeterX with behavioral, Kasada, Akamai BMP advanced) | ❌ even cookie handoff is fragile | real Chrome via CDP is the right tier |

**Vs the alternatives:**

| | This | curl | Playwright / headless Chrome |
|---|---|---|---|
| Static / SSR pages | ✅ | ✅ but token-heavy | overkill |
| SPA-shell sites | ⚠️ partial via `exec_scripts` | ❌ | ✅ |
| Bot-walled (with cookie handoff) | ✅ | ❌ | ✅ |
| Run in Lambda / Docker / edge containers | ✅ | ✅ | ❌ Chrome too big |
| Per-page cost at 100K/day | ~free | ~free | $$$ |
| LLM-shaped output | ✅ BlockMap inline | DIY parse | DIY parse |

## Verified against (working)

Concrete sites tested with measured times. Cold-start to extracted-result.

| Category | Sites | Time |
|---|---|---|
| Reference / docs | Wikipedia, MDN, docs.rs, PyPI, react.dev (SSR portion) | 0.9 – 5.8s |
| News | Hacker News, BBC, TechCrunch, ArXiv listings | 1 – 1.6s |
| Search | Google `/search`, Bing, Brave, DuckDuckGo (html) | 0.2 – 1.8s |
| Dev | GitHub repo pages, npm, StackOverflow, HuggingFace model cards | 0.7 – 2.4s |
| Crypto / finance | CoinGecko, Yahoo Finance (post-redirect-fix) | 3.5 – 6.9s |
| Social | Lobsters, old.reddit.com | 0.9 – 1.4s |
| Govt / institutional | arXiv, archive.org, gov.uk | 0.6 – 1.0s |
| Interaction primitives | type, click + auto-follow, cookies_set/get/replay, eval, query_text | 0.3 – 1.3s |

**Surprises:** all four major search engines work cleanly. CoinGecko's heavy dashboard SSRs enough that quotes come through. HuggingFace model cards expose model name in `<h1>`.

## Bot-detection diagnostics

Every blocked navigate returns a `challenge` field naming the vendor (`perimeterx_block`, `cloudflare_turnstile`, `aws_waf`, `datadome`, `akamai_bmp`, `imperva`, `arkose_labs`, `recaptcha`, `press_hold`, `yahoo_sad_panda`, `interstitial`, `generic_human_verification`, `unknown_block`) plus the expected clearance cookie name. Agents react with cookie handoff via `cookies_set` instead of guessing.

### Local cookie solver service

For fully transparent cookie handoff, run the local-only solver service backed by `unchained-cli`:

```bash
pip install 'pyunbrowser[solver]'  # or: pip install unchainedsky-cli
python scripts/cookie_service.py --headless --profile unbrowser-cookie-service
export UNBROWSER_COOKIE_SERVICE_URL=http://127.0.0.1:8765
```

Then use `scripts/router.py` (or `RouterConfig(cookie_service_url=...)`) as the agent-facing entry point. On a blocked navigate the router will:

```text
detect challenge -> call local service -> Chrome obtains cookies -> cookies_set -> retry once
```

The service exposes `GET /.well-known/unbrowser-cookie-solver` and `POST /solve`, supports the same challenge providers as `navigate.challenge`, and returns only cookies from the user's local Chrome/unchained session. It does not fabricate challenge tokens. Keep it bound to `127.0.0.1`; non-loopback binds are rejected unless `--allow-remote-bind` is passed because `/solve` is unauthenticated and can return browser cookies. Use `--allow-host` for domain allowlisting when desired, and use `--no-headless --stealth` for sites that reject headless Chrome. Chrome persists across solves by default for the standalone service; pass `--no-keep-chrome` for one-shot use. Solves are serialized per service process because a service instance owns one CDP port/profile pair.

When installed from the Python package, the same pieces are bundled behind the console wrapper:

```bash
unbrowser cookie-service --headless --profile unbrowser-cookie-service
unbrowser router https://example.com/protected
```

`unbrowser router` also auto-starts the local cookie service on first challenge when `unchained` is available and `UNBROWSER_COOKIE_SERVICE_URL` is not set. `--allow-host example.com` allows `example.com` and its subdomains only; broad single-label suffixes like `com` are rejected. Without an allowlist, the service rejects private/reserved IPs, localhost, and internal single-label hosts by default; use `--allow-host` to opt in to a specific internal host for local testing. Router refuses non-loopback `UNBROWSER_COOKIE_SERVICE_URL` values by default because it posts target URLs and challenge metadata to that service; pass `--allow-remote-cookie-service` only for a trusted remote solver.

## SPA-detection diagnostics

Every navigate's `blockmap.density` field signals SPA-ness so agents bail before wasting round-trips:
- `thin_shell: true` — page is < 4KB body text with no headings or interactives (typical React/Ember root). For HTTP errors (`status >= 400`), shell signals are suppressed and `http_error_status` is attached so a 404 is not mistaken for an SPA.
- `likely_js_filled: true` — table/list/cell shells are empty, or the page has many scripts with little visible UI (CNBC / YouTube-class trap)
- `json_scripts: N` — count of `<script type="application/json">` (often holds the data the JS would render — try `eval()` on those before escalating)
- `script_heavy_shell: true` — many scripts, little text, few links; usually browser-rendered UI rather than useful SSR

## Three ways agents talk to it

### Session CLI (persistent shell workflow)

When an agent can only shell out but needs incremental state, start a local daemon-backed session. Cookies, DOM, JS globals, and element refs persist until `stop`:

```bash
unbrowser session start --id golf
unbrowser exec golf navigate https://news.ycombinator.com
unbrowser exec golf query '.titleline > a'
unbrowser exec --pretty golf blockmap
unbrowser exec golf eval 'document.title'
unbrowser session stop golf
```

`session exec` and the shorter `exec` alias accept either shorthand args for common methods or a raw JSON params object:

```bash
unbrowser exec golf navigate https://example.com --exec-scripts
unbrowser exec golf query_debug '.product-card' --limit 5
unbrowser exec golf extract_cards '{"kind":"product","limit":20}'
unbrowser session prune   # remove dead sockets
```

### MCP (no glue)

```json
{
  "mcpServers": {
    "unchained": {
      "command": "unbrowser",
      "args": ["--mcp"]
    }
  }
}
```

Tools are auto-discovered by Claude Code, Claude Desktop, Cursor, Cline.

### Subprocess (custom runtimes)

13 lines of Python (above). Or any language with subprocess + JSON.

### Auto-escalation router (`scripts/router.py`)

```python
from scripts.router import Router, RouterConfig, cached_cookies_solver

with Router(RouterConfig(
    binary="./target/release/unbrowser",
    chrome_solver=cached_cookies_solver("cookies.json"),
)) as r:
    r.navigate("https://www.zillow.com/homes/for_rent/")  # auto-handles 403 + cookie replay
```

### Live event watcher (`scripts/watch.py`)

The binary emits NDJSON events (`ready`, `navigate`, `challenge`) on stderr. Pipe them through `watch.py` for color-coded one-liners:

```bash
unbrowser 2> >(python3 scripts/watch.py)
```

## RPC methods

| | |
|---|---|
| `navigate {url}` | fetch + parse + return `{status, url, bytes, headers, blockmap, challenge, tool_confidence, tool_margin, tool_likelihoods, tool_recommendations}` |
| `query {selector}` | CSS query → `[{ref, tag, attrs, text, text_chars, text_truncated}]` |
| `query_debug {selector, limit?}` | explain selector misses: match count, sample matches, DOM summary, top tags/classes/data attrs/ids, and hints like `selector_miss`, `thin_shell`, `embedded_json` |
| `text {selector?}` | textContent of FIRST match (default `body`). On Wikipedia/MDN/news sites the first `<p>` is often a hatnote — prefer `text_main` for article body. |
| `text_main` | textContent of `<main>` / `[role=main]` / single `<article>` / longest non-chrome subtree. Use this for reading article/docs/blog content. |
| `discover {url?, goal?, exec_scripts?, same_origin?, include_network?, limit?, debug?}` | Cheap-first information discovery. Merges DOM routes, inferred form/query URLs, and network JSON routes into one ranked graph with provenance and escalation hints. Defaults to static discovery; set `exec_scripts: true` when fetch-visible routes are insufficient. |
| `extract_cards {selector?, limit?, kind?}` | auto-detect repeated product/listing/article cards and return normalized fields including `title`, `price`, `condition`, `url`, `availability`, `snippet`, `meta`, `image_alt`, `score` |
| `extract_table {selector}` / `table_to_json {selector?}` | normalize an HTML table into headers, rows, and row count. `table_to_json` defaults to the first `table`. |
| `click {ref}` | dispatch click; auto-follows `<a href>` (returns `{status, url, bytes, headers, blockmap, challenge}` — same shape as `navigate`) |
| `type {ref, text}` | set value + dispatch input/change events |
| `submit {ref}` | gather form fields and navigate. Supports GET and `application/x-www-form-urlencoded` POST; multipart is not supported. |
| `eval {code}` | run JS in embedded QuickJS. Raw JSON-RPC also accepts `script` or `expression` aliases and now errors instead of silently returning `null` when no code-like param is present. |
| `cookies_set / cookies_get / cookies_clear` | session jar |
| `blockmap` | recompute the page summary |
| `body` | raw HTML of last navigation |

`blockmap.selectors` surfaces concrete selector hints for the current page (`data-testid`, `aria-label`, `role`) so agents can bias toward `query` or `query_text` without guessing.

`discover` is the route-finding layer to use before extraction when you need to learn where information lives. By default it returns compact `navigate_summary`, `route_discover_summary`, and `network_extract_summary` fields plus the merged `routes`, `forms`, `api_endpoints`, `network_sources`, and `escalations`. Pass `debug: true` only when you need the full nested `navigate`, `route_discover`, and `network_extract` payloads for diagnosis. `limit` must be between 1 and 200; invalid `url` / `limit` inputs fail at the RPC boundary.

```json
{"jsonrpc":"2.0","id":1,"method":"discover","params":{"url":"https://example.com","goal":"find pricing docs api status","same_origin":true,"limit":25}}
```

Use `exec_scripts: true` as an opt-in second pass for pages whose static HTML does not expose enough routes. In that mode, routes already present before scripts are labeled `static_dom`; routes that only appear after JavaScript/timers/fetches are labeled `js_dom`.

CSS selector engine: tag, id, class, `[attr=val]` (also `^=`, `$=`, `*=`, `~=`), all four combinators (` `, `>`, `+`, `~`), `:first/last/nth-child/of-type` including An+B formulas, `:only-child/of-type`, `:not()`, and `:has()`.

## When to escalate to real Chrome

This binary is the cheap path. For the cases it can't handle (heavy framework hydration, behavioral bot challenges, Workers/Canvas/IndexedDB), the next tier is a real Chrome instance driven via CDP. Two ways to get there:

| | This binary | [`unchainedsky-cli`](https://github.com/protostatis/unchainedsky-cli) | [Unchained desktop app](https://unchainedsky.com) |
|---|---|---|---|
| Runs JS | QuickJS (no V8 JIT) | real Chrome via CDP | real Chrome (the user's, with their logins) |
| SPA hydration | partial | ✅ | ✅ |
| Bot challenges | cookie handoff only | active solving via real browser | manual / interactive |
| Setup | `pip install pyunbrowser` | `pip install unchainedsky-cli` | desktop install |
| Audience | agent / pipeline | agent / pipeline | end user |
| Per-page footprint | ~50MB | full Chrome | full Chrome |

The escalation path is a deliberate choice, not an automatic fallback — you ship `pyunbrowser` for the 80% of pages that work cheap, then route the 20% to `unchainedsky-cli` (or to a human via the desktop app). The vocabulary (`navigate`, `query`, `click`, `cookies_set`, BlockMap) is shared so code transfers cleanly.

## Honest limits

- **Script execution is opt-in via `exec_scripts: true`.** Default navigate skips it (the SSR/static path is what most agents want). With it on, inline + external `<script>` tags run in QuickJS — works for many SPAs, but heavy framework bootstraps (Ember, big React) often don't auto-mount because shims can't fake every browser-specific signal. The blockmap's `density.likely_js_filled` flag tells agents in one call when to escalate instead of burning round-trips.
- **All eval is wall-clock bounded.** A 30s watchdog (configurable via `UNBROWSER_TIMEOUT_MS`, clamped to 1s..10min) covers script execution AND every subsequent settle/microtask/timer callback, so a hostile site can never wedge the binary or strand a CPU-pegged orphan process.
- **Form submit is intentionally narrow.** GET and `application/x-www-form-urlencoded` POST are supported, including checked checkbox/radio values. Multipart upload forms are not supported — construct the request manually via `eval` or escalate.
- **Hardest-tier bot detection** (PerimeterX with behavioral telemetry, advanced Akamai BMP, Kasada) needs the cookie-handoff path. The binary detects and labels the challenge for you, but solving it requires real Chrome (or a token vendor).
- **No screenshots.** Out of scope by design.

## Build

Rust 1.95+ via [rustup](https://rustup.rs). On macOS, also `brew install cmake ninja` (BoringSSL dependency).

```bash
cargo build --release
```

~2 min first build (BoringSSL compiles), instant after.

## Architecture in one diagram

```
JSON-RPC stdin ─┐    ┌─ stdout
                ▼    ▲
         ┌────────────────────┐
          │  request (Chrome  │   ┌──────────┐    ┌──────────────────┐
          │  TLS+H2 fingerprint)├──▶ html5ever ├───▶ rquickjs +       │
         │                    │   │  parser  │    │  dom.js +        │
         │  cookie_store      │   └──────────┘    │  blockmap.js +   │
         │  (jar)             │                   │  interact.js     │
         └────────────────────┘                   └──────────────────┘
```

## License

Apache 2.0 — see [LICENSE](./LICENSE).

---

For the cases this binary can't handle (heavy framework hydration, behavioral bot challenges, anything needing real Chrome), the next tier is [`unchainedsky-cli`](https://github.com/protostatis/unchainedsky-cli) — drives a real Chrome via CDP, same vocabulary. End-users who want a point-and-click agent can skip the CLI entirely and use the [Unchained desktop app](https://unchainedsky.com).
