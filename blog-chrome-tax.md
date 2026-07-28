# The Chrome Tax: What Heavy Browsers Cost Your Agent

**TL;DR:** Every time your AI agent launches Chrome to browse a page, you're paying a tax — in memory, cold starts, tokens, and infra complexity. Here's the math.

---

## The Hidden Cost

You're building an AI agent. It needs to browse the web. You reach for Playwright or Puppeteer because that's what everyone uses.

But have you measured what it actually costs?

Let's do the math.

---

## The Numbers

### Memory

Chrome is a memory hog. Each tab runs in its own process, with its own V8 instance, its own DOM engine, its own GPU context.

**Per session:**
- Chromium base: ~150MB
- Playwright context: +30MB
- Page with JS: +100–200MB
- **Total: 280–380MB per tab**

Run 10 concurrent agent sessions? That's **2.8–3.8 GB RAM** just for browsers.

### Cold Starts

On Lambda, adding Chrome means:
- Layer size: +200–300MB
- Cold start: +8–15 seconds
- Memory limit: 10GB minimum for stable operation

On Fly.io or Railway, you're still looking at 3–5 second cold starts with a Chrome container.

### Token Waste

Raw HTML from a typical page: **8,000–15,000 tokens**.

Your LLM is paying to process:
- CSS class names
- HTML attributes
- Script tags
- Navigation boilerplate
- Ad markup

None of that helps your agent accomplish its task.

### Container Bloat

Your Docker image with Chrome: **500MB–1GB**.

Without Chrome: **10–50MB**.

That's a 10–50x difference in deployment size, pull time, and startup.

---

## The Escalation Ladder

Not every page needs Chrome. Here's the hierarchy:

```
curl / fetch
  ↓ (needs JS?)
WebFetch / readability
  ↓ (needs clicks/forms/cookies?)
unbrowser (QuickJS)
  ↓ (needs rendering/canvas/WebML?)
Playwright / Puppeteer
  ↓ (needs human challenge solving?)
Real Chrome + human
```

Most agent builders jump straight to Playwright. But if your page is:
- Server-side rendered
- JavaScript-enhanced but not a full SPA
- A form submission or login flow
- A dashboard that needs cookies

...you're overpaying.

---

## Real Example: Scraping 1,000 Pages

Let's say your agent needs to extract data from 1,000 pages across 50 concurrent sessions.

| Tool | RAM | Cold Start | Tokens | Container |
|------|-----|------------|--------|-----------|
| Playwright | 14 GB | 8s | 12M | 500MB |
| Puppeteer | 15 GB | 10s | 12M | 500MB |
| Browserbase | N/A (cloud) | 2s | 12M | 0 |
| unbrowser | 1.5 GB | 0.1s | 500K | 10MB |
| curl | 0.1 GB | 0.01s | 3M | 2MB |

The "Chrome tax" in this scenario:
- **12.5 GB extra RAM**
- **7.9 seconds per cold start**
- **11.5 million extra tokens**

At $0.01/1K tokens (GPT-4 pricing), that token waste costs **$115 per 1,000 pages**.

---

## When Chrome Is Worth It

Chrome isn't always the wrong choice. Use Chrome when:

- You need screenshots or visual verification
- The page uses Canvas, WebGL, or heavy client-side rendering
- You need to solve CAPTCHAs interactively
- The site fingerprints browser APIs and blocks headless clients
- You need exact compatibility with real user behavior

Chrome is the compatibility ceiling. Everything else is a trade-off.

---

## When Lighter Browsers Win

Use a lighter tool when:

- The page is server-rendered (Wikipedia, news sites, docs)
- You need JS execution but not rendering (SPAs with API calls)
- You need cookies, forms, or multi-step flows
- You're running on Lambda, edge, or resource-constrained environments
- Token efficiency matters (LLM costs scale with input size)

---

## The Decision Framework

Ask yourself:

1. **Does my task need pixels?** → Chrome
2. **Does it need JavaScript?** → unbrowser or Chrome
3. **Does it need clicks/forms/cookies?** → unbrowser or Chrome
4. **Is the page static HTML?** → curl
5. **What's my budget?** → lighter is cheaper

---

## What We Built

We got tired of paying the Chrome tax for agent browsing that didn't need rendering.

So we built [unbrowser](https://unchainedsky.com/unbrowser) — a 10MB binary that gives your agent a headless browser with real JS execution, no Chrome required.

- **10MB** static binary (vs 500MB Chrome)
- **30MB** RAM per session (vs 280MB)
- **0.1s** cold start (vs 8s on Lambda)
- **~500 tokens** output per page (vs 12,000 raw HTML)
- Built-in challenge detection (PerimeterX, Cloudflare, Datadome)
- MCP integration for Claude Desktop/Code

It's not a Chrome replacement. It's the middle tier between curl and Chrome — for agents that need JS but not rendering.

---

## Try It

```bash
# Install
curl -fsSL https://unchainedsky.com/unbrowser/install.sh | sh

# Browse a page
unbrowser navigate https://example.com

# Or use as MCP server
unbrowser --mcp
```

---

## The Calculator

We built a calculator to help you estimate your Chrome tax:

**[Open the Chrome Tax Calculator](chrome-tax-calculator.html)**

Enter your deployment details and see exactly what you're paying.

---

*This post is part of a series on agent browsing architecture. Next: "Bot Wall Field Guide — What Your Agent Sees When It's Blocked."*

---

**About:** We're building tools for AI agents that need to interact with the web. [Unchained Sky](https://unchainedsky.com).
