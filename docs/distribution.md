# unbrowser Distribution

`unbrowser` is the developer wedge for Unchained. Lead with the lightweight open-source tool, then leave a clear path to `https://unchainedsky.com` when users need real Chrome, auth, extensions, or hosted infrastructure.

## Positioning

Primary name: `unbrowser`

Breadcrumb name: `unbrowser by Unchained`

## Submission Status

Updated: 2026-08-11

Done:

- `mcp.so`: submission issue opened at `https://github.com/chatmcp/mcpso/issues/2500`; hosted endpoint follow-up posted in `https://github.com/chatmcp/mcpso/issues/2500#issuecomment-4538473171`.
- Glama: accepted, claimed, author-verified, released at `https://glama.ai/mcp/servers/protostatis/unbrowser`, and related-server suggestions added.
- Hosted Unchained MCP endpoint: deployed at `https://unchainedsky.com/unbrowser-mcp` for public discovery and smoke tests.
- Smithery: published with `Unlisted` off at `https://smithery.ai/servers/protostatis-dev/unbrowser` using the hosted Unchained MCP endpoint; homepage/repository/license/backlink/icon are set.
- `punkpeye/awesome-mcp-servers`: PR merged at `https://github.com/punkpeye/awesome-mcp-servers/pull/6860`.
- GitHub Release: `v0.0.19` published at `https://github.com/protostatis/unbrowser/releases/tag/v0.0.19`.
- PyPI: `pyunbrowser` v0.0.19 published at `https://pypi.org/project/pyunbrowser/`.
- crates.io: `unbrowser` v0.0.19 published at `https://crates.io/crates/unbrowser`.
- Homebrew: v0.0.18 remains live; the four-architecture v0.0.19 formula update is under owner review at `https://github.com/protostatis/homebrew-tap/pull/1`.
- ClawHub: skill v0.0.18 remains live; the versioned source-card repair needed for a safe v0.0.19 publish is under owner review at `https://github.com/protostatis/unbrowser/pull/45`.
- Official MCP Registry: `io.github.protostatis/unbrowser` v0.0.18 remains active. The v0.0.19 manifest is prepared in source but has not been published.
- v0.0.20 is prepared in source (version bump + new release section below); publish pending tag `v0.0.20`.

Pending or blocked:

- `mcpservers.org`: the server-rendered listing is public and indexed, but a real browser still redirects its route to `/submit`. A correction request is pending separate owner review.
- `ComposioHQ/awesome-claude-skills`: PR `https://github.com/ComposioHQ/awesome-claude-skills/pull/908` was closed by the owner account on 2026-07-03 without merge; do not reopen it without new owner direction.
- `appcypher/awesome-mcp-servers`: branch pushed to `https://github.com/protostatis/awesome-mcp-servers/tree/add-unbrowser`; `gh pr create` failed with `CreatePullRequest` permissions. Manual compare URL: `https://github.com/appcypher/awesome-mcp-servers/compare/main...protostatis:add-unbrowser`.
- Smithery exact namespace: UI created `protostatis-dev/unbrowser`; pursue `protostatis/unbrowser` only if exact namespace ownership matters.
- Smithery verification: not verified yet. The checklist now passes successful release, homepage, and Smithery backlink; it still fails quality score because the score is exactly 80/100, exact-homepage-host TXT verification, and paid developer plan.

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

Hosted Streamable HTTP endpoint for public smoke/discovery:

```json
{
  "mcpServers": {
    "unbrowser-hosted": {
      "url": "https://unchainedsky.com/unbrowser-mcp"
    }
  }
}
```

The hosted endpoint is shared and unauthenticated. Use it for public-page demos, directory scans, and smoke tests only; do not send secrets, private cookies, or authenticated browsing tasks through it.

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

Prepared Glama build settings for the next package release (not deployed):

```json
{
  "baseImage": "debian:trixie-slim",
  "pythonVersion": "3.14",
  "nodeVersion": "26",
  "buildSteps": [
    "uv venv /opt/unbrowser --python 3.14 && VIRTUAL_ENV=/opt/unbrowser uv pip install pyunbrowser==0.0.19"
  ],
  "cmdArguments": [
    "/opt/unbrowser/bin/unbrowser",
    "--mcp"
  ]
}
```

Note: Glama wraps `cmdArguments` with `mcp-proxy --` in the generated Dockerfile, producing `CMD ["mcp-proxy","--","/opt/unbrowser/bin/unbrowser","--mcp"]`.

Hosted smoke checklist for each Glama release:

1. Start a fresh hosted instance from **Try in Browser**.
2. Confirm MCP `initialize` reports `serverInfo.name = "unbrowser"` and the expected package version.
3. Confirm `tools/list` returns the expected tool count.
4. Call `navigate` on `https://news.ycombinator.com` and expect status `200`, title `Hacker News`, and `challenge: null`.
5. Call `query` with `.titleline > a` and expect 30 story links.
6. Call `text` or `text_clean` on the current page and confirm readable page text for SSR pages.
7. Call `query` for `input[name=q]` and `form`, then `type` `wreq` into the input and `submit` the form.
8. Expect the submit result to land on `https://hn.algolia.com/?q=wreq` with status `200`; `likely_js_filled: true` and sparse text there is expected because Algolia is an SPA shell.

Latest package-version-verified hosted smoke result: Glama release `v0.1.2` built on `pyunbrowser==0.0.16` (`019f3405`), status: published. On 2026-08-11, the anonymous directory surface reported repository release `0.1.4`, 32 tools, and A scores for license, quality, and maintenance, but did not expose the hosted `pyunbrowser` pin. Do not infer a hosted package upgrade from the repository release number. Previous: `v0.1.1` initialized `unbrowser` `0.0.14`, exposed 32 tools, navigated Hacker News, returned 30 `.titleline > a` links, typed into HN search, and submitted to Algolia successfully.

Latest score result: Server Quality A, Server Coherence A, Tool Definition Quality A, Maintenance A, related servers present, and 32 of 32 tools scored.

## v0.0.20 Release

Prepared 2026-08-21. Publish pending tag `v0.0.20`. Distribution streams:

- **GitHub Release**: binary tarballs for 4 platforms + SHA256 checksums at `https://github.com/protostatis/unbrowser/releases/tag/v0.0.20` (created by the release workflow on tag)
- **PyPI**: `pip install pyunbrowser==0.0.20` — platform wheels bundling the native binary
- **crates.io**: `cargo install unbrowser` — v0.0.20
- **Homebrew**: tap update to follow publish (v0.0.18 live)
- **ClawHub**: skill source card bumped to 0.0.20 in source; publish follows tag
- **Official MCP Registry**: `server.json` manifest bumped to 0.0.20 in source; publish follows tag
- **GHCR**: `ghcr.io/protostatis/unbrowser:v0.0.20` (+ `0.0.20`, then serialized `:latest` promotion) — multi-arch linux/amd64 + linux/arm64 distroless image, built by the release workflow's `publish-ghcr` job from the prebuilt Linux binaries, with MCP-init / navigate / nonroot smoke gates before push
- **Glama / Smithery / mcp.so**: hosted endpoint unaffected by version bump

Highlights over 0.0.19: smart MCP surface (`search`/`open`/`help`) with calibrated `micro_hint`, `avoid`, and `tool_entropy` routing aids; clig.dev-style CLI discovery (`--help` groups, `help <topic>`, `search`/`open` subcommands, did-you-mean); MCP spec annotations and handshake `instructions`; Python packaging with `unbrowser-smart` console script.

## v0.0.19 Release

Released 2026-08-05. Distribution streams:

- **GitHub Release**: binary tarballs for 4 platforms + SHA256 checksums at `https://github.com/protostatis/unbrowser/releases/tag/v0.0.19`
- **PyPI**: `pip install pyunbrowser==0.0.19` — 4 platform wheels bundling the native binary
- **crates.io**: `cargo install unbrowser` — v0.0.19
- **Homebrew**: live tap remains v0.0.18; v0.0.19 is in owner-review PR `protostatis/homebrew-tap#1`
- **ClawHub**: live skill remains v0.0.18; v0.0.19 source integrity is in owner-review PR `protostatis/unbrowser#45`
- **Official MCP Registry**: active record remains v0.0.18; v0.0.19 is prepared, not published
- **Glama**: latest package-version-verified hosted release remains v0.1.2 on `pyunbrowser==0.0.16`; the anonymous listing reports repository release `0.1.4` but not the hosted package pin
- **Smithery**: uses the hosted Unchained MCP endpoint and is not package-version-specific

## v0.0.18 Release

Released 2026-07-28. Distribution streams:

- **GitHub Release**: binary tarballs for 4 platforms + SHA256 checksums at `https://github.com/protostatis/unbrowser/releases/tag/v0.0.18`
- **PyPI**: `pip install pyunbrowser==0.0.18` — platform wheels bundling the native binary
- **crates.io**: `cargo install unbrowser` — v0.0.18
- **Homebrew**: `brew install protostatis/tap/unbrowser` — v0.0.18
- **ClawHub**: skill v0.0.18 published at `https://clawhub.ai/protostatis/skills/unbrowser`
- **Official MCP Registry**: active package-only record for `io.github.protostatis/unbrowser` v0.0.18
- **Glama**: latest recorded hosted release remains v0.1.2 on `pyunbrowser==0.0.16`; the documented next build settings target `pyunbrowser==0.0.19`
- **Smithery**: uses the hosted Unchained MCP endpoint and is not package-version-specific

## v0.0.16 Release

Released 2026-07-05. Distribution streams:

- **GitHub Release**: binary tarballs for 4 platforms + SHA256 checksums at `https://github.com/protostatis/unbrowser/releases/tag/v0.0.16`
- **PyPI**: `pip install pyunbrowser==0.0.16` — platform wheels bundling native binary
- **crates.io**: `cargo install unbrowser` — v0.0.16
- **Homebrew**: `brew install protostatis/tap/unbrowser` — v0.0.16
- **ClawHub**: skill v0.0.16 published at `https://clawhub.ai/protostatis/skills/unbrowser`
- **Glama**: release v0.1.2 used `pyunbrowser==0.0.16`
- **Smithery**: uses hosted Unchained MCP endpoint (not version-specific)

Changes:
- Bump default Chrome profile 134→147 (fresh UA/Sec-CH-UA headers)
- Add `reddit_network_block` challenge detector for old.reddit.com TLS-fingerprint 403s
- Update old.reddit.com prefit entry with routing hint
- Document wreq BoringSSL TLS/H2 fingerprint drift as known limitation

### mcp.so

URL: `https://mcp.so/submit`

Submission path: create an issue from the site/GitHub flow.

Title: `unbrowser by Unchained`

Description: use the short description above.

Tags: `browser-automation`, `web-scraping`, `mcp`, `llm-agent`, `headless-browser`, `developer-tools`.

### Smithery

URL: `https://smithery.ai/servers/new`

Status: published with `Unlisted` off at `https://smithery.ai/servers/protostatis-dev/unbrowser`.

Smithery URL publishing requires a public HTTPS Streamable HTTP transport. Use the stable hosted endpoint:

```text
https://unchainedsky.com/unbrowser-mcp
```

Do not submit ephemeral Glama hosted instance URLs. CLI/API publishing requires Smithery auth, e.g. `npx -y smithery mcp publish "https://unchainedsky.com/unbrowser-mcp" -n protostatis/unbrowser`. The CLI prompts for a Smithery API key from `https://smithery.ai/account/api-keys`; this run used the logged-in Smithery UI instead of storing an API key.

Title: `unbrowser by Unchained`

Subtitle: `Lightweight browser access for agents, by Unchained.`

Release result: Smithery created `protostatis-dev/unbrowser`; deployment `17045078-887b-4e70-80c6-a13a550f8c5c` completed with `SUCCESS`, retrieved `serverInfo.name = "unbrowser"`, and found 32 tools. Follow-up deployment `5ece747a-2ef8-42f6-8af5-363889151826` also completed with `SUCCESS` after republishing the same hosted endpoint with an explicit empty config schema. Settings now show the Unchained landing page, GitHub repository, `Apache-2.0` license, same-domain backlink, custom icon, and `Unlisted` off. On 2026-08-11, the public listing still showed 32 tools, a last deployment 14 days earlier, and a quality score of 80/100; verification requires greater than 80.

Verification blockers: Smithery's public verification page currently marks successful release, homepage, and backlink as passing. It still marks quality score, exact-host TXT record for `unchainedsky.com`, and paid developer plan as incomplete. The latest scan log still warns that resources and prompts could not be listed. Local MCP stdio now advertises `resources`/`prompts` and returns empty arrays for `resources/list` and `prompts/list`; this needs a new packaged/deployed release before the hosted endpoint can remove those scan warnings.

Latest hosted smoke result: on 2026-08-11 the production endpoint initialized MCP over both `/unbrowser-mcp` and `/unbrowser-mcp/mcp`, exposed 32 tools, navigated Hacker News successfully, blocked internal/private metadata targets with 403, and returned 404 for `/unbrowser-mcp/sse`. Verified by `python3 scripts/hosted_mcp_smoke.py` after accepting both the legacy deployment-map status payload and the current bounded-capacity status payload.

Version note: the hosted endpoint reported `serverInfo.version = "1.29.0"` on 2026-08-11 because `mcp-proxy` constructs the proxied server with the upstream `serverInfo.name` but does not pass through the upstream `serverInfo.version`; the Python MCP SDK then defaults to its own package version. This value does not prove the hosted `unbrowser` package version. Fix options are to patch/vendor `mcp-proxy` to pass `version=response.serverInfo.version`, or replace the bridge with native Streamable HTTP support in `unbrowser`.

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
2. Monitor `mcp.so` issue `https://github.com/chatmcp/mcpso/issues/2500` after the hosted endpoint follow-up.
3. Monitor `ComposioHQ/awesome-claude-skills` PR `https://github.com/ComposioHQ/awesome-claude-skills/pull/908` for review.
4. Do not add a Composio skill folder unless their validator changes; current PR validation allows `README.md` only.
5. Launch on HN only after enough directory/listing links are live.
