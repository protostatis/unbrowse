# unbrowser Distribution

`unbrowser` is the developer wedge for Unchained. Lead with the lightweight open-source tool, then leave a clear path to `https://unchainedsky.com` when users need real Chrome, auth, extensions, or hosted infrastructure.

## Positioning

Primary name: `unbrowser`

Breadcrumb name: `unbrowser by Unchained`

## Submission Status

Updated: 2026-05-25

Done:

- `mcp.so`: submission issue opened at `https://github.com/chatmcp/mcpso/issues/2500`.
- `mcpservers.org`: free submission completed with contact email `protostatis.dev@gmail.com`; site reported successful submission and review within 12 hours.
- Glama: accepted, claimed, author-verified, and released at `https://glama.ai/mcp/servers/protostatis/unbrowser`.
- `punkpeye/awesome-mcp-servers`: PR opened at `https://github.com/punkpeye/awesome-mcp-servers/pull/6860`; Glama badge added and checks passing.
- `ComposioHQ/awesome-claude-skills`: PR opened at `https://github.com/ComposioHQ/awesome-claude-skills/pull/908`; checks passing after switching to a README-only external listing.

Pending or blocked:

- `mcpservers.org`: public listing not yet visible after submission.
- `appcypher/awesome-mcp-servers`: branch pushed to `https://github.com/protostatis/awesome-mcp-servers/tree/add-unbrowser`; `gh pr create` failed with `CreatePullRequest` permissions. Manual compare URL: `https://github.com/appcypher/awesome-mcp-servers/compare/main...protostatis:add-unbrowser`.
- Smithery: not submitted yet; do after basic directory/listing links are live.

One-liner:

> Lightweight MCP browser for LLM agents. One native binary, no Chrome, low-token page maps, stateful sessions, and cookie handoff when pages need real Chrome.

Short description:

> `unbrowser` gives agents browser-like web access without launching Chrome. It fetches pages, runs bounded JavaScript, returns low-token BlockMaps, supports query/click/type/forms/cookies, and exposes an MCP server with `unbrowser --mcp`.

Long description:

> `unbrowser` is a lightweight open-source browser runtime for LLM agents. It sits between `curl`/WebFetch and full Playwright/Chrome: stateful enough to navigate, query, interact, run bounded JS, and detect bot/SPAs, but small enough to run in CI, Lambda, edge workers, and local agent workflows. When a task needs real Chrome, it points users to Unchained via `unchainedsky-cli` or `https://unchainedsky.com`.

## Install Snippets

Python:

```bash
pipx install pyunbrowser
```

Cargo:

```bash
cargo install unbrowser
```

MCP config:

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

Use `unchained` as the alias when we want the parent brand breadcrumb. Use `unbrowser` as the alias when a directory requires exact package naming.

## Directory Submissions

### mcpservers.org

URL: `https://mcpservers.org/submit`

Server Name: `unbrowser by Unchained`

Short Description: `Lightweight MCP browser for LLM agents: no Chrome, low-token BlockMaps, stateful sessions, JS execution, forms, cookies, and escalation hints.`

Link: `https://github.com/protostatis/unbrowser`

Category: `Web Scraping` or `Development`

Contact Email: use project/contact email.

### Glama

URL: `https://glama.ai/mcp/servers`

Category targets: `Browser Automation`, `Search`, `Web Scraping`, `Developer Tools`, `Local`.

Listing title: `unbrowser by Unchained`

Description: use the short description above.

Status: public listing claimed and author-verified. Latest Glama release `v0.1.1` published from commit `eff1674a7bd7ee466aedc1a50eda038f8eb2c908`; build test `019e60f6-34e2-7c51-a26f-eb5d63d26ac4` succeeded and initialized `unbrowser` `0.0.14` through `mcp-proxy`.

Glama build settings:

```json
{
  "baseImage": "debian:trixie-slim",
  "pythonVersion": "3.14",
  "nodeVersion": "26",
  "buildSteps": [
    "uv venv /opt/unbrowser --python 3.14 && VIRTUAL_ENV=/opt/unbrowser uv pip install pyunbrowser==0.0.14"
  ],
  "cmdArguments": [
    "/opt/unbrowser/bin/unbrowser",
    "--mcp"
  ]
}
```

Note: Glama wraps `cmdArguments` with `mcp-proxy --` in the generated Dockerfile, producing `CMD ["mcp-proxy","--","/opt/unbrowser/bin/unbrowser","--mcp"]`.

### mcp.so

URL: `https://mcp.so/submit`

Submission path: create an issue from the site/GitHub flow.

Title: `unbrowser by Unchained`

Description: use the short description above.

Tags: `browser-automation`, `web-scraping`, `mcp`, `llm-agent`, `headless-browser`, `developer-tools`.

### Smithery

URL: `https://smithery.ai/servers/new`

Do after basic directory submissions. Smithery is higher effort because it expects a packaged/published server experience. Use it once install and config are polished.

Title: `unbrowser`

Subtitle: `Lightweight browser access for agents, by Unchained.`

## Awesome List PRs

### punkpeye/awesome-mcp-servers

Repo: `https://github.com/punkpeye/awesome-mcp-servers`

Best section: `Browser Automation` or `Search & Data Extraction`.

Entry:

```markdown
- [unbrowser](https://github.com/protostatis/unbrowser) 🦀 🏠 🍎 🐧 - Lightweight MCP browser for LLM agents. One native binary, no Chrome, low-token BlockMaps, stateful sessions, bounded JS execution, forms, cookies, and escalation hints to real Chrome via Unchained.
```

### appcypher/awesome-mcp-servers

Repo: `https://github.com/appcypher/awesome-mcp-servers`

Best section: `Search & Web`.

Entry:

```markdown
- **[unbrowser](https://github.com/protostatis/unbrowser)** - Lightweight browser-like MCP server for LLM agents. Runs without Chrome, returns low-token page maps, supports query/click/type/forms/cookies, and escalates to Unchained when real Chrome is needed.
```

### ComposioHQ/awesome-claude-skills

Repo: `https://github.com/ComposioHQ/awesome-claude-skills`

Best section: `Development & Code Tools`.

Entry:

```markdown
- [unbrowser](https://github.com/protostatis/unbrowser) - Lightweight web access for LLM agents. Runs as one native binary with MCP support, no Chrome dependency, low-token BlockMaps, stateful navigation, forms, cookies, and a clean escalation path to Unchained for real Chrome workflows.
```

## Launch Hooks

Use these as blog/HN/Reddit titles:

- `Show HN: unbrowser - a tiny browser for LLM agents, no Chrome required`
- `A lightweight MCP browser for agents that do not need full Playwright`
- `curl is too dumb, Chrome is too heavy: unbrowser for agent web access`
- `Run agent web access in CI/Lambda without shipping Chromium`

## Comparison Frame

Do not pitch as a Playwright replacement.

Pitch as:

> The cheap first browser tier before Playwright, Browserbase, or a real Unchained Chrome session.

Comparison copy:

| Alternative | What to say |
|---|---|
| `curl` / Fetch MCP | Great for static HTML, but no state, JS, forms, cookies, or agent-shaped page maps. |
| Playwright MCP | Best for full browser fidelity, but Chrome is heavy for simple agent browsing and extraction. |
| Firecrawl / hosted scraping APIs | Useful hosted extraction, but not a local stateful browser primitive. |
| Unchained | The escalation tier for real Chrome, auth, extensions, and visual/browser-hosted workflows. |

## First Week Checklist

1. Monitor `mcpservers.org` approval email at `protostatis.dev@gmail.com`.
2. Submit to `mcp.so`.
3. Submit/claim on Glama.
4. Open PR to `punkpeye/awesome-mcp-servers`.
5. Open PR to `ComposioHQ/awesome-claude-skills`.
6. Do not add a Composio skill folder unless their validator changes; current PR validation allows `README.md` only.
7. Launch on HN only after two directory/listing links are live.
