# Draft: We tested 21 websites with lightweight browsing and Chrome. Here’s where each failed.

Status: outline draft. Use this as the second-wave distribution asset after `/chrome-tax` gets one feedback cycle.

## Thesis

Chrome is the compatibility target, but it should be an escalation tier, not the default for every agent page fetch. A useful browser stack needs an honest failure matrix: static fetch, JS execution without rendering, full Chrome, and real-browser/cookie handoff.

## Candidate headline

We tested 21 websites with two headless browsers. Here’s where each failed.

## Intro

Most web agents start with Playwright because it works. That decision is safe, but it is not free: hundreds of MB per session, slower cold starts, larger containers, and noisy DOM/token output.

The question is not “Can lightweight browsing replace Chrome?” It cannot. The better question is: “Which sites actually need Chrome, and which only need stateful JavaScript, cookies, links, and forms?”

This benchmark documents that line.

Estimate the cost side first: https://unchainedsky.com/chrome-tax?ref=benchmark_post

## Test tiers

- `curl/fetch` — HTTP only, no JS, no interactions.
- `unbrowser` — native QuickJS runtime, stateful cookies, DOM, JS, forms, clicks, challenge detection, no rendering.
- `Chrome/Playwright or Obscura` — V8 and rendering tier, heavier but more compatible.
- `real Chrome + cookie handoff` — human/managed-browser escalation for bot walls.

## Known results to include

| Site | Category | unbrowser result | Chrome/Obscura result | Takeaway |
| --- | --- | --- | --- | --- |
| npmjs.com | Cloudflare-backed app | Passes, useful page extracted | Passes | Full Chrome not required |
| StackOverflow | Cloudflare | Passes where Obscura default saw challenge | Challenge in tested default | TLS/header behavior matters, not just V8 |
| crates.io | Ember SPA | `thin_shell:true` | Renders title/content | Rendering/V8 tier wins |
| Reddit | network/TLS block | `reddit_network_block`, 0.9 confidence | Fails in tested setup | Wire fingerprint can dominate browser engine |
| Amazon | AWS WAF | Detects `aws_waf`; cookie handoff succeeds | Real Chrome/session needed first | Escalation + replay is right model |

## Sections

1. Methodology: same network, same task, what counts as success.
2. Results matrix: 21 rows grouped by failure mode.
3. Failure taxonomy:
   - static SSR, no JS needed
   - JS execution needed, no pixels
   - SPA/rendering needed
   - challenge page/cookie clearance needed
   - TLS/network-level block
4. Cost model: link back to `/chrome-tax`.
5. Decision rule:
   - start with fetch/readability for static pages
   - use lightweight JS when you need session, JS, links/forms/cookies
   - escalate to Chrome when you need pixels/layout/canvas/heavy SPA rendering
   - escalate to real Chrome/human when you hit interactive bot walls
6. Honest limitations.

## Data still needed

- Fill remaining 16 sites with repeatable commands and saved outputs.
- Record status code, title, chars, headings, links, challenge detector output, and elapsed time.
- Re-run in one clean environment before publishing.
- Add methodology repo link once public.
