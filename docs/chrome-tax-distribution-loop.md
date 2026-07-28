# Chrome Tax distribution loop

Goal: route qualified builders to `https://unchainedsky.com/chrome-tax` without turning it into a product pitch. Treat every post as a measurement, then adjust the next post from observed feedback.

## Control-loop cadence

1. **Prepare** — choose one channel, one hypothesis, one link with `?ref=`.
2. **Post** — ask for calibration/critique, not conversion.
3. **Observe for 24–48h** — collect traffic, comments, objections, quotes, and downvote/flag signals.
4. **Classify feedback** — update the objection log below.
5. **Regroup** — change copy/page/data only when feedback repeats or reveals factual ambiguity.
6. **Next post** — never post identical copy twice; carry forward the best objection answer.

## Primary metric

Qualified discussion per source, not raw clicks.

Track:

- `/chrome-tax?ref=<source>` page views
- `/chrome-tax` → `/unbrowser` CTA clicks (`chrome_tax_try_unbrowser`)
- Methodology clicks (`chrome_tax_methodology`)
- GitHub/Smithery/Glama clicks from `/unbrowser`
- comment quality: corrections, benchmark requests, “we have this problem,” “Chrome is required because…”

## Ref tags

- `hn_show` — Hacker News Show HN
- `hn_text` — HN text post/discussion
- `lobsters` — Lobsters
- `reddit_localllama`
- `reddit_ai_agents`
- `reddit_webscraping`
- `reddit_devops`
- `github_issue_<repo>` — targeted issue/discussion comments
- `x_thread`
- `benchmark_post`

## Posting rules

- Lead with the problem: defaulting agents to Chrome has measurable cost.
- Say Chrome is necessary for visual/canvas/CAPTCHA workflows.
- Ask whether the assumptions match production experience.
- Do not claim unbrowser replaces Chrome generally.
- Do not comment on stale/closed issues unless the issue still gets traffic.
- Do not reply defensively; turn objections into test cases.

## Objection log template

| Date | Source | Objection / feedback | Count | Action |
| --- | --- | --- | ---: | --- |
| YYYY-MM-DD | hn_show | “Numbers are too hand-wavy” | 1 | Add method detail or benchmark link if repeated |
| YYYY-MM-DD | reddit_webscraping | “Sites block non-Chrome anyway” | 1 | Point to challenge detector/cookie handoff; add limitation text if repeated |

## Channel sequence

1. Instrumentation + methodology links live.
2. Small GitHub/discussion comments first, where context is specific.
3. HN/Lobsters post once page copy has one round of corrections.
4. Reddit discussion only after benchmark post exists.
5. Benchmark post as second-wave asset.

## GitHub comment template

> We ran into a similar “Chrome tax” problem while building browser tools for agents: Chrome/Playwright is often the right fallback, but it is expensive when the task only needs JS execution, cookies, links, and forms.
>
> I made a calculator for estimating the RAM/cold-start/token overhead and comparing curl/WebFetch/lightweight JS/full Chrome tiers:
> https://unchainedsky.com/chrome-tax?ref=github_issue_REPO
>
> Would be curious if the assumptions match what you’re seeing here. If not, I’d like to update the methodology.

## HN text

Title options:

- Show HN: A calculator for the cost of using Chrome in web agents
- The Chrome Tax: what heavy browsers cost your agent
- When do AI agents actually need Chrome?

Post:

> I’ve been building a lightweight browser runtime for LLM agents and kept seeing the same pattern: people use Playwright/Chrome even when the task only needs JavaScript execution, cookies, clicks, and forms — not pixels.
>
> I made a calculator to estimate the memory, cold-start, and token overhead of that choice. It includes a capability matrix for curl/WebFetch/lightweight JS/full Chrome/managed browser tiers.
>
> Calculator: https://unchainedsky.com/chrome-tax?ref=hn_show
>
> I’m especially interested in whether the assumptions match what others are seeing in production, and where people draw the line before escalating to full Chrome.

## Reddit discussion template

Title:

> When do your agents actually need Chrome?

Body:

> I’ve been trying to quantify the cost of defaulting every web task to Playwright/Chrome. For many agent workloads, the browser never needs screenshots or layout — just JS, cookies, clicks, and forms.
>
> I made a calculator for the “Chrome tax”: https://unchainedsky.com/chrome-tax?ref=reddit_SUBREDDIT
>
> Curious where people draw the line between curl/WebFetch, lightweight JS runtimes, and full Chrome. What task makes you immediately reach for real Chrome?
