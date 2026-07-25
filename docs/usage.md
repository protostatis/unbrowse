# Using unbrowser

This is the operational reference for `unbrowser`: installation, command surfaces, MCP, and the full RPC table. For compatibility, challenge handling, and escalation, see [Compatibility and escalation](compatibility.md).

## Installation

### Docker

The public multi-arch image supports Linux amd64 and arm64:

```bash
docker pull ghcr.io/protostatis/unbrowser:latest
docker run --rm ghcr.io/protostatis/unbrowser:latest \
  navigate https://example.com --json
```

The image is distroless and runs as non-root. Its default command is `unbrowser --mcp`; pass a subcommand such as `navigate` to replace that default. State lives only for the lifetime of a container, so keep one `docker run -i` MCP process alive for a multi-step session. Pin `:vX.Y.Z` or an image digest in production.

### Python

The `pyunbrowser` wheel bundles the native binary. Python 3.10+ is required.

```bash
pipx install pyunbrowser   # recommended on macOS / modern Linux
# or, in a virtual environment:
pip install pyunbrowser
```

The PyPI name is `pyunbrowser`; the import and executable are both `unbrowser`.

> macOS system Python is 3.9. Use `pipx` or a Homebrew Python. `pipx` also avoids PEP 668 externally-managed-environment errors.

### Cargo

```bash
cargo install unbrowser
unbrowser --mcp
```

This compiles the native binary locally and needs Rust plus the BoringSSL build dependencies (`cmake` and `ninja`; on macOS, `brew install cmake ninja`).

### Release archive

```bash
# macOS Apple Silicon
curl -L https://github.com/protostatis/unbrowser/releases/latest/download/unbrowser-aarch64-apple-darwin.tar.gz | tar xz

# macOS Intel
curl -L https://github.com/protostatis/unbrowser/releases/latest/download/unbrowser-x86_64-apple-darwin.tar.gz | tar xz

# Linux x86_64 (glibc 2.31+ / Ubuntu 20.04+)
curl -L https://github.com/protostatis/unbrowser/releases/latest/download/unbrowser-x86_64-unknown-linux-gnu.tar.gz | tar xz

# Linux arm64 (Ubuntu 22.04+ / Graviton)
curl -L https://github.com/protostatis/unbrowser/releases/latest/download/unbrowser-aarch64-unknown-linux-gnu.tar.gz | tar xz
```

### Build from source

```bash
cargo build --release
# ./target/release/unbrowser
```

Use Rust 1.95+ and install `cmake` plus `ninja` for BoringSSL (`brew install cmake ninja` on macOS).

## Choose an interface

### One-shot CLI

Run one command, print one JSON result, then exit:

```bash
unbrowser navigate https://news.ycombinator.com --json
```

### Session CLI

For shell-only agents that need persistent cookies, DOM state, JS globals, and element refs:

```bash
unbrowser session start --id demo
unbrowser exec demo navigate https://news.ycombinator.com
unbrowser exec demo query '.titleline > a'
unbrowser exec --pretty demo blockmap
unbrowser session stop demo
```

`exec` accepts shorthand arguments for common methods or a raw JSON params object:

```bash
unbrowser exec demo navigate https://example.com --exec-scripts
unbrowser exec demo query_debug '.product-card' --limit 5
unbrowser exec demo extract_cards '{"kind":"product","limit":20}'
unbrowser session prune
```

### MCP over stdio

Local binary:

```json
{
  "mcpServers": {
    "unbrowser": {
      "command": "unbrowser",
      "args": ["--mcp"]
    }
  }
}
```

Docker:

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

For public discovery only, a shared Streamable HTTP endpoint is available at `https://unchainedsky.com/unbrowser-mcp`. Do not send private cookies, secrets, or authenticated tasks through it.

### Raw JSON-RPC

Use the default binary mode for a long-lived stdin/stdout session:

```bash
echo '{"id":1,"method":"navigate","params":{"url":"https://news.ycombinator.com"}}' | unbrowser
```

The protocol is one JSON object per line. Send subsequent requests to the same process when a task needs state.

## Script mode and runtime shims

Default navigation does not run page scripts. Opt in when SSR HTML or embedded JSON is insufficient:

```bash
unbrowser navigate https://example.com --exec-scripts --json
```

The default `stable` shims favor predictable behavior. `enhanced` adds content-positive guesses for layout, media, scroll, and IndexedDB for corpus comparison:

```bash
unbrowser navigate https://example.com --exec-scripts --json --shims enhanced
UNBROWSER_SHIMS=enhanced unbrowser --mcp
python3 scripts/shim_ab.py --url https://nextjs.org/docs
```

Script evaluation is bounded by `UNBROWSER_SCRIPT_EVAL_BUDGET_MS` (default `5000`). The outer watchdog is `UNBROWSER_TIMEOUT_MS` (default `30000`, clamped to 1s–10min).

## RPC methods

| Method | Purpose |
|---|---|
| `navigate {url, exec_scripts?, shims?}` | Fetch, parse, and return status, headers, BlockMap, `challenge`, and route/tool signals. |
| `query {selector}` | CSS query → stable refs plus tag, attributes, and text. |
| `query_debug {selector, limit?}` | Diagnose selector misses with sample matches, DOM summary, and hints. |
| `text {selector?}` / `text_main` | Read a selected node or the likely article/main subtree. |
| `discover {url?, goal?, exec_scripts?, same_origin?, include_network?, limit?, debug?}` | Find DOM/form/network routes and rank them for an information goal. |
| `extract_cards {selector?, limit?, kind?}` | Normalize repeated article, product, or listing cards. |
| `extract_table {selector}` / `table_to_json {selector?}` | Normalize an HTML table into headers and rows. |
| `click {ref}` / `type {ref, text}` / `submit {ref}` | Follow links and interact with HTML forms. GET and URL-encoded POST are supported; multipart is not. |
| `eval {code}` | Execute JavaScript in the embedded QuickJS runtime. |
| `cookies_set` / `cookies_get` / `cookies_clear` | Manage the in-process cookie jar. |
| `blockmap` / `body` | Recompute the page map or return raw last-navigation HTML. |
| `close` | Exit a raw JSON-RPC process. MCP hosts own lifecycle and do not expose this tool. |

Blocked navigations include `challenge.provider`, the HTTP status, a confidence signal, and (when known) the clearance-cookie name.

CSS selectors support tags, IDs, classes, attribute operators, all four combinators, `:first/last/nth-child/of-type`, `:only-child/of-type`, `:not()`, and `:has()`.

## Events

The binary emits NDJSON events to stderr. For local colorized output:

```bash
unbrowser 2> >(python3 scripts/watch.py)
```

Events cover navigation, script execution, policy decisions, challenges, and outcomes. Treat stdout as protocol output; do not mix stderr into an MCP or JSON-RPC parser.
