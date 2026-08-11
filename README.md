# unbrowser

**The cheap browser pass for agents. One native binary. No Chrome.**

[![unbrowser MCP server](https://glama.ai/mcp/servers/protostatis/unbrowser/badges/score.svg)](https://glama.ai/mcp/servers/protostatis/unbrowser)

**Official MCP Registry identity:** `mcp-name: io.github.protostatis/unbrowser`

`unbrowser` is a stateful, non-visual web runtime for agents. It sits between `curl`/WebFetch and a real browser: it retains cookies and DOM state, returns queryable element refs, handles links and HTML forms, detects challenge and SPA signals, and can run bounded QuickJS page scripts when static HTML is not enough.

> **Default `navigate` is a fast static/SSR pass.** Set `exec_scripts: true` only when you need bounded QuickJS execution. Heavy SPAs, pixels, V8 fidelity, extensions, and interactive anti-bot challenges belong in a real Chrome tier.

Try the [live public-web demo](https://unchainedsky.com/unbrowser?utm_source=github&utm_medium=repository&utm_campaign=unbrowser_readme&ref=readme_live_demo) before installing. It accepts only the fixed public source sets shown on the page; do not send private data, cookies, or authenticated tasks through it. A shared Streamable HTTP MCP endpoint is available at [`https://unchainedsky.com/unbrowser-mcp`](https://unchainedsky.com/unbrowser-mcp) for public smoke tests; production sessions should use a local install.

Use `unbrowser` only on sites, accounts, and data you are permitted to access.
Challenge detection and cookie import do not grant permission to bypass access
controls or site terms. Treat session cookies as credentials: keep them local,
scope them to the authorized task, and never send them to the public demo.

## Pick the right tier

| Need | curl / WebFetch | unbrowser | Real Chrome |
|---|---|---|---|
| Static / SSR HTML | raw response | structured BlockMap + DOM queries | full browser |
| Cookies, links, HTML forms | DIY | built in | built in |
| Client-side page scripts | ❌ | bounded QuickJS, opt-in | V8 |
| Pixels, Canvas, WebGL, Workers, extensions | ❌ | ❌ | ✅ |
| Agent-oriented output | DIY parsing | element refs, page signals, structured extraction | DIY CDP / DOM parsing |
| Interactive access challenge | ❌ | detect + stop or escalate | authorized browser / human confirmation |

Use `unbrowser` when HTTP alone is too dumb and a full browser is too expensive. When the page needs Chrome, the output tells the agent to escalate rather than pretending compatibility it does not have.

## Quick start

### Docker — Linux amd64/arm64, ~13 MiB pull

```bash
docker pull ghcr.io/protostatis/unbrowser:latest

# One-shot navigation
docker run --rm ghcr.io/protostatis/unbrowser:latest \
  navigate https://example.com --json

# Default mode: MCP over stdio
docker run --rm -i ghcr.io/protostatis/unbrowser:latest
```

The image is distroless and runs as non-root: no shell, package manager, or persistent state. Pin `:vX.Y.Z` or an image digest in production.

### Python

```bash
pipx install pyunbrowser   # recommended on macOS / modern Linux
# or, inside a Python 3.10+ virtual environment:
pip install pyunbrowser
```

```python
from unbrowser import Client

with Client() as ub:
    ub.navigate("https://news.ycombinator.com")
    for link in ub.query(".titleline > a")[:3]:
        print(link["text"], link["attrs"]["href"])
```

On macOS, `/usr/bin/python3` is 3.9 and cannot install the wheel; use `pipx` or a Homebrew Python. The PyPI distribution is `pyunbrowser`, while the import and executable remain `unbrowser`.

### MCP

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

<details>
<summary>Docker MCP configuration</summary>

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

</details>

See [installation and interface reference](docs/usage.md#installation) for Cargo, release archives, source builds, persistent shell sessions, raw JSON-RPC, and all MCP options.

## What an agent gets

- **`navigate` returns a BlockMap**: page title, landmarks, headings, interactives, density signals, and an ASCII outline. Its size is page-dependent; it is structured for planning rather than a fixed-token promise.
- **Stable element refs** (`e:142`): query an element once, then `click`, `type`, or `submit` it without re-parsing HTML.
- **Stateful cookies and forms**: cookie jar, GET and URL-encoded POST form submission, links, and redirects persist within a session.
- **Page and challenge signals**: `density.likely_js_filled`, `thin_shell`, and `challenge.provider` tell an agent whether to run scripts, inspect embedded data, stop, or escalate to an authorized browser session.
- **Structured helpers**: route discovery, card extraction, table normalization, `text_main`, and selector debugging cover common extraction workflows.

## Script mode and escalation

```json
{"id":1,"method":"navigate","params":{"url":"https://example.com","exec_scripts":true}}
```

With `exec_scripts: true`, inline and external scripts run in QuickJS under a bounded watchdog. This can materialize light hydration and fetch-visible data; it is not V8 or a rendering engine. Heavy React/Vue/Ember apps may still leave an empty shell.

Escalate to [`unchainedsky-cli`](https://github.com/protostatis/unchainedsky-cli) or [Unchained](https://unchainedsky.com) when a permitted task needs real pixels, Canvas/WebGL, Workers, browser extensions, V8 compatibility, an authenticated profile, or human confirmation. When the user has already established an authorized session in Chrome, `cookies_set` can import the required session cookie locally until it expires; it is not a license to circumvent a site's controls.

## Documentation

| Need | Read |
|---|---|
| Install paths, session CLI, one-shot CLI, raw RPC, MCP, shims, full RPC table | [Usage reference](docs/usage.md) |
| Script compatibility, SPA signals, challenge handling, authorized cookie handoff, escalation | [Compatibility and escalation](docs/compatibility.md) |
| Distribution and supported directory listings | [Distribution notes](docs/distribution.md) |
| Build the native binary | [Build instructions](docs/usage.md#build-from-source) |

## License

Apache-2.0 — see [LICENSE](./LICENSE).
