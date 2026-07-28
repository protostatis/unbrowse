# WebVoyager usability improvement plan

Distributed implementation plan for improving `unbrowser` usability against
fixed WebVoyager-style task corpora. The goal is to split the work into
independent subagent branches, merge them into one integration PR, then rerun and
score the same corpora.

## Baseline run

Sixteen read-only WebVoyager tasks were run with subagents using only
`target/release/unbrowser` over JSON-RPC.

This first run is a fast regression baseline, not full benchmark-site coverage:
it covers 9 of the 15 unique WebVoyager sites. The all-site coverage corpus below
adds one representative task for every unique site in the upstream 643-task
dataset.

Raw per-task baseline records are checked in at
`docs/webvoyager-baseline-v1-2026-05-16.jsonl`. The file intentionally stores the
subagent result summaries in JSONL rather than prose so future candidate runs can
diff task outcomes, timings, signals, and friction counters mechanically.
The canonical JSONL data contract and artifact changelog are documented in
`docs/webvoyager-data-format.md`.

Baseline artifact naming uses `webvoyager-baseline-v{N}-YYYY-MM-DD.jsonl` so a
single current baseline version can be referenced while older baselines remain
auditable.

The all-site coverage seed is checked in at
`docs/webvoyager-site-coverage-v1.jsonl`.

An initial all-site run is checked in at
`docs/webvoyager-site-coverage-run-v1-2026-05-17.jsonl`.

After the first implementation pass, a v2 all-site run is checked in at
`docs/webvoyager-site-coverage-run-v2-2026-05-17.jsonl`.

After adding the semantic `page_model` layer, a page-model-first v3 run is checked
in at `docs/webvoyager-site-coverage-run-v3-2026-05-17.jsonl`.

After adding `network_extract`, `page_model.network_objects`, `activate`, and
relative URL resolution in JS `fetch()`, a v4 all-site run is checked in at
`docs/webvoyager-site-coverage-run-v4-2026-05-17.jsonl`.

After adding `route_discover` and Google retry-shell browser-route detection, a
v5 all-site run is checked in at
`docs/webvoyager-site-coverage-run-v5-2026-05-17.jsonl`.

| Metric | All-site run |
|---|---:|
| Sites covered | 15 / 15 |
| Answer success | 8 / 15 |
| Handled success | 15 / 15 |
| Answered directly | 8 |
| Challenge-routed | 3 |
| Browser-routed | 4 |

| Metric | v1 all-site | v2 all-site | v3 page-model-first | v4 network/action | v5 route-discover |
|---|---:|---:|---:|---:|---:|
| Answer success | 8 / 15 | 8 / 15 | 8 / 15 | 8 / 15 | 8 / 15 |
| Handled success | 15 / 15 | 15 / 15 | 15 / 15 | 15 / 15 | 15 / 15 |
| `manual_url_guess` friction | 4 / 15 | 2 / 15 | 2 / 15 | 2 / 15 | 1 / 15 |
| `body_used` friction | n/a | 1 / 15 | 1 / 15 | 1 / 15 | 0 / 15 |
| `noisy_text` friction | 7 / 15 | 2 / 15 | 3 / 15 | 1 / 15 | 0 / 15 |
| `form_confusion` friction | 3 / 15 | 1 / 15 | 2 / 15 | 0 / 15 | 0 / 15 |
| Precise browser/challenge routing | 7 / 7 | 7 / 7 | 7 / 7 | 7 / 7 | 7 / 7 |

The v2 pass did not add direct-answer wins; Google Flights, Google Maps, Google
Search, and Wolfram Alpha still require browser routing, while Amazon, Booking,
and ESPN remain challenge-routed. The improvement is reduced workaround friction
on answered sites and more explicit route metadata for non-answers.

The v3 page-model-first pass validated that `page_model` exposes useful semantic
objects and provenance, but forcing it as the first tool did not improve all-site
answer success or residual manual guessing. It slightly increased recorded
friction on Allrecipes/GitHub/Hugging Face/Wolfram because the model currently
adds structure without yet being sufficiently selective or action-diagnostic.

The v4 pass again did not add direct-answer wins; the strict-mode ceiling remains
the same challenge/browser-route split. It did reduce residual friction: Wolfram
now stops on `browser_route` before probing a misleading form, Google Search uses
the page-owned form and only falls back to body evidence for the enable-JS shell,
and `activate` gives action-effect evidence for Coursera's Free Courses route.

The post-v4 friction pass adds `route_discover {goal?, limit?}` and promotes
Google Search retry-shell signatures (`/httpservice/retry/enablejs`, `SG_REL`,
and related markers) into `browser_route.reason=enable_js_interstitial`. The v5
run keeps the same strict-mode answer ceiling but removes Google Search `body`
fallback, makes Hugging Face model-filter navigation provenance-backed via the
visible `/models` route, and leaves BBC article discovery as the only remaining
manual URL guess.

| Metric | Baseline |
|---|---:|
| Answer success | 11 / 16 |
| Non-bot-wall answer success | 11 / 13 |
| Correct hard challenge routing | 3 / 3 |

Failure classes:

| Class | Count | Tasks |
|---|---:|---|
| Expected bot-wall escalation | 3 | `Amazon--0`, `Booking--0`, `ESPN--18` |
| Rate-limit / soft block | 1 | `ArXiv--0` |
| Benchmark or site drift | 1 | `BBC News--28` |

Successful tasks:

| Task | Domain | Notes |
|---|---|---|
| `ArXiv--17` | arxiv.org | Direct paper page extraction worked; search path had friction. |
| `ArXiv--27` | arxiv.org | Clean SSR category page. |
| `BBC News--5` | bbc.com | Clean article extraction after direct article URL resolution. |
| `Apple--12` | apple.com | Clean SSR; noisy text required targeted extraction. |
| `Apple--6` | apple.com | Product page extraction worked. |
| `Apple--19` | apple.com | Needed embedded data / text filtering. |
| `Allrecipes--40` | allrecipes.com | Article navigation and paragraph extraction worked. |
| `Allrecipes--41` | allrecipes.com | Listing/cards worked with title cleanup. |
| `GitHub--29` | github.com | Pricing page worked; text was noisy. |
| `GitHub--37` | github.com | Product page worked; headings needed filtering. |
| `Coursera--37` | coursera.org | Large SSR/listing page was usable. |

## Target fixes

1. Add richer form summaries.
2. Add `find_text` / `text_around` style localized extraction.
3. Add cleaner content modes for main/article text.
4. Promote `aws_waf` as a first-class challenge path.
5. Surface rate-limit metadata and retry hints.
6. Make BlockMap interactive fields expandable, not just counts.
7. Improve repeated card/list extraction.
8. Record WebVoyager task runs as JSONL and score future builds consistently across the fast-regression and all-site corpora.
9. Make network capture/source summaries auditable so headline counts match visible evidence or clearly point to the full `network_stores` call.
10. Add a semantic `page_model` layer so agents can work with task-discoverable JSON objects instead of flat links, counts, and broad text.
11. Add `network_extract` so captured API/GraphQL/NDJSON payloads become scored semantic objects instead of raw `body_preview` blobs.
12. Add `activate` so agents can prove whether an action navigated, changed the DOM, changed network captures, or had no effect.
13. Add `route_discover` so agents can use page-owned links/forms/inferred query URLs before constructing routes manually.
14. Promote common enable-JS retry shells into `browser_route` so agents do not need raw `body` fallback just to classify them.

## Issue-driven implementation plan

The 15-site run found three classes of failure:

| Class | Sites / examples | Product goal |
|---|---|---|
| Recoverable usability friction | Cambridge search, BBC discovery, Apple/GitHub noisy text, Coursera cards | Reduce agent workarounds and improve direct-answer reliability. |
| Correct cheap-path limits | Amazon, Booking, ESPN `aws_waf` | Keep routing clear and actionable; do not waste retries. |
| Browser-route gaps | Google Flights, Google Maps, Google Search, Wolfram Alpha | Detect unusable rendered/result UI early and route explicitly. |

Implementation should optimize for two metrics: more direct answers where the
DOM is sufficient, and more precise `handled_success` classifications where it
is not.

### 1. Expand BlockMap from counts to actionable targets

Observed issues:

| Site | Symptom |
|---|---|
| Apple, Coursera | Agents had to query raw `a` lists because BlockMap only exposed link counts. |
| Google Flights | Inputs were visible and mutable, but submit/search intent was opaque. |
| Cambridge Dictionary | Search controls were visible, but form ownership and submit semantics were ambiguous. |

Design:

- Keep existing count fields for compatibility.
- Add bounded samples for links and buttons, capped at 50 each and ranked by likely usefulness.
- Add labels to every control using `aria-label`, associated `<label for>`, parent label text, placeholder, `name`, `title`, and nearby text.
- Group controls by nearest form and synthesize `query_preview` for GET forms.
- Add `submit_candidates` for forms and standalone buttons that look like search/submit actions.

API additions:

| Field | Shape |
|---|---|
| `blockmap.interactives.link_samples[]` | `{ref, text, href, aria_label?, title?, role?}` — sparse v2 output, score is internal-only |
| `blockmap.interactives.button_samples[]` | `{ref, text, type, aria_label?, title?, role?, matches?}` — sparse v2, repeated labels grouped |
| `forms[].controls[]` | `{ref, tag, type, name, label, placeholder, value, checked, selected, options}` |
| `forms[].submit_candidates[]` | `{ref, tag, text, type, score, reason}` |
| `forms[].query_preview` | Resolved GET action plus current serializable field names, with values redacted only for password-like fields. |

Implementation tasks:

| Step | Files | Notes |
|---|---|---|
| Label extraction helper | `src/js/blockmap.js` | Shared `labelFor(el)` used by inputs/buttons/links. |
| Ranked samples | `src/js/blockmap.js` | Prefer visible text, non-empty href, main/content ancestry, search-ish labels. |
| Form grouping | `src/js/blockmap.js`, `src/js/interact.js` | Mirror `__formData` serialization rules in summaries. |
| Select support | `src/js/dom.js`, `src/js/interact.js` | Ensure select/option `value` and selected state serialize predictably. |

Smoke tests:

- Synthetic GET search form with one text input, one hidden field, one select, and two submit buttons.
- Synthetic label variants: `<label for>`, wrapping `<label>`, `aria-label`, placeholder-only.
- Synthetic standalone search button outside a form.

Acceptance criteria:

- Cambridge Dictionary search form exposes a control labeled `Search` or `Search English` plus a submit candidate.
- Google Flights exposes labeled origin, destination, departure, and return controls, and if no safe submit candidate exists the browser-route classifier can cite that exact missing action.
- Agents should not need an initial raw `query('a')` on Apple/GitHub/Coursera to find product/course links.

### 2. Add localized and clean text extraction

Observed issues:

| Site | Symptom |
|---|---|
| Apple, GitHub, Coursera | Body/main text included large nav, JSON, FAQ, or duplicated content. |
| BBC, Allrecipes | Article content was available but noisy. |
| ArXiv | Section-specific text was truncated in query output, forcing broader text extraction. |

Design:

- Add `find_text` for localized matching with before/after context.
- Add `text_around` for text near an element ref or matched string.
- Add `text_clean` as a stricter version of `text_main` that drops scripts, styles, noscript, SVG, nav/header/footer/aside, JSON-looking text blocks, repeated boilerplate, and low-content hidden widgets.
- Add optional `max_chars` to `text`, `text_main`, and `text_clean` so drivers do not need to pull huge bodies.

RPC additions:

| RPC | Purpose |
|---|---|
| `find_text {text, selector?, exact?, limit?, context_chars?}` | Find localized matches and return `{ref, tag, attrs, before, match, after, text}`. |
| `text_around {ref? | text?, selector?, context_chars?}` | Return surrounding cleaned text around an element or text match. |
| `text_clean {selector?, max_chars?}` | Return chrome/JSON-stripped text from selector or best content root. |

Implementation tasks:

| Step | Files | Notes |
|---|---|---|
| DOM text cleaner | `src/main.rs` or `src/js/extract.js` | Reuse from `text_main`, add JSON/script/noise filters. |
| Context finder | `src/main.rs` | Extend current `query_text` logic but return context windows. |
| MCP schemas | `src/main.rs` | Expose new tools in `mcp_tools()` and `dispatch_tool()`. |
| Docs | `README.md`, this plan | Explain when to prefer each text tool. |

Smoke tests:

- Page with article, nav/footer, embedded JSON, noscript image fallback, repeated FAQ/footer text.
- Page where a target term appears in nav and article; `find_text` should rank article match higher.

Acceptance criteria:

- BBC and Allrecipes answers should be extractable without full-body text.
- GitHub Copilot answer should come from heading/context extraction without reading large embedded JSON.
- ArXiv submission history should be retrievable via context, not body-wide scan.

### 3. Improve repeated card/list extraction

Observed issues:

| Site | Symptom |
|---|---|
| Allrecipes | Card titles included noscript/image artifacts. |
| Coursera | Course cards were extractable but noisy and semantically ambiguous (`Free Trial` vs `free`). |
| Hugging Face | Agent jumped to public API rather than discovering model listings through page structure. |

Design:

- Keep `extract_list` but clean per-field text with the same noise-removal rules as `text_clean`.
- Add `extract_cards {selector?, limit?, kind?}` that auto-detects repeated blocks and returns normalized `{title, url, snippet, meta, image_alt, score}`.
- Add a `card_candidates` summary to BlockMap for pages with repeated article/product/course/listing structures.

Implementation tasks:

| Step | Files | Notes |
|---|---|---|
| Clean text helper | `src/js/extract.js` | Shared by `extract_list`, `extract_cards`, `text_clean`. |
| Card detector | `src/js/extract.js` | Detect repeated anchors/articles/list items by structural similarity and text density. |
| RPC wrapper | `src/main.rs` | Add `extract_cards` method and MCP schema. |
| Tool likelihoods | `src/main.rs` | Raise `extract_cards` recommendation on high repeated-block signals. |

Smoke tests:

- Synthetic article cards with image/noscript fallback, repeated footer links, and duplicate titles.
- Synthetic course cards with badges/meta fields.

Acceptance criteria:

- Allrecipes dinner cards return clean recipe titles.
- Coursera course cards return title plus pricing/free-trial meta separately.
- Hugging Face model listing pages expose model titles/tags without requiring direct API URL guessing.

### 4. Add explicit browser-route classification

Observed issues:

| Site | Symptom |
|---|---|
| Google Maps | Thin JS shell; no interactives. |
| Google Search | 200 status but enable-JS/trouble-accessing page with no result snippets. |
| Wolfram Alpha | 200 status but no usable query input. |
| Google Flights | Inputs were present, but no usable action/result surface was exposed. |

Design:

- Add `browser_route` metadata to `navigate` and tool advice when a page is not challenged but unbrowser cannot reasonably proceed.
- Browser-route reasons should be precise and auditable, not a generic failure.
- Distinguish `thin_shell`, `no_interactives`, `missing_primary_action`, `enable_js_interstitial`, `rendered_result_required`, and `canvas_or_map_ui`.

API additions:

| Field | Shape |
|---|---|
| `navigate.browser_route` | `{needed, reason, confidence, evidence, hint}` |
| `tool_recommendations` | Include `chrome_escalation` when browser-route confidence dominates. |

Implementation tasks:

| Step | Files | Notes |
|---|---|---|
| Detector | `src/main.rs`, `src/js/blockmap.js` | Combine density, interactives, title/body signatures, forms/actions, and result-surface signals. |
| Signatures | `src/challenge.rs` or new helper | Google `enablejs`, map UI, Wolfram app shell signatures are not bot challenges. |
| Tool likelihoods | `src/main.rs` | Recommend `chrome_escalation` with reason. |
| Router/watch output | `scripts/router.py`, `scripts/watch.py` | Print browser-route separately from challenge. |

Smoke tests:

- Synthetic app shell with no landmarks/interactives.
- Synthetic page with inputs but no form/action/submit candidate.
- Synthetic enable-JS interstitial page with 200 status.

Acceptance criteria:

- Google Maps routes as `browser_route.reason=thin_shell` or `canvas_or_map_ui`.
- Google Search routes as `enable_js_interstitial`, not success and not generic failure.
- Wolfram routes as `missing_primary_action` or `rendered_result_required`.
- Google Flights cites `missing_primary_action` when fields exist but no safe search action/result surface exists.

### 5. Preserve and sharpen challenge/rate-limit routing

Observed issues:

| Site | Symptom |
|---|---|
| Amazon, Booking, ESPN | AWS WAF routing worked well. |
| ArXiv search | Prior run hit 429 and was classified as generic `unknown_block`. |

Design:

- Keep `aws_waf` detection as first-class and document clearance-cookie handling.
- Add explicit `rate_limit` metadata for 429, 503 with retry headers, and tiny-body throttles.
- Do not classify normal rate limits as bot-wall challenges unless vendor signatures are present.

API additions:

| Field | Shape |
|---|---|
| `navigate.rate_limit` | `{limited, status, retry_after, retry_after_seconds, reason, hint}` |
| `challenge.provider` | Keep vendor-specific challenge only for actual challenge pages. |

Implementation tasks:

| Step | Files | Notes |
|---|---|---|
| Header parsing | `src/main.rs` | Parse `Retry-After` and expose it in navigate result. |
| Rate-limit detector | `src/challenge.rs` or new helper | Return separate rate-limit metadata before generic `unknown_block`. |
| Watch/router display | `scripts/watch.py`, `scripts/router.py` | Print rate limit as retry-later, not browser escalation. |

Smoke tests:

- Local 429 page with and without `Retry-After`.
- Tiny 503 page.
- AWS WAF fixture still reports `challenge.provider=aws_waf`.

Acceptance criteria:

- ArXiv-style 429 records `handling=rate_limited` with retry hint.
- AWS WAF behavior remains unchanged for Amazon/Booking/ESPN.

### 6. Reduce manual URL/API guessing

Observed issues:

| Site | Symptom |
|---|---|
| BBC | Agent used known article URL when page discovery was weak. |
| Cambridge Dictionary | Agent used `/dictionary/english/sustainability` pattern. |
| Hugging Face | Agent used `/api/models` directly. |

Design:

- Improve page-discovery tools so agents can follow visible structures first.
- Add URL-template hints only when the page itself exposes a search form or canonical route pattern.
- Track `manual_url_guess` in the eval runner so regressions are visible even when answers are correct.

Implementation tasks:

| Step | Files | Notes |
|---|---|---|
| Discovery samples | `src/js/blockmap.js` | Link samples and form query previews reduce raw URL guessing. |
| Result schema enforcement | `train/*.py` | Require friction fields and compute aggregate counts. |
| Optional route hints | `src/js/blockmap.js` | Surface canonical/search form action patterns, not invented routes. |

Acceptance criteria:

- Cambridge should be answerable via homepage form or explicit form action preview.
- BBC should either find the article link/search route or record `manual_url_guess=true` clearly.
- Hugging Face API use remains allowed only if discovered via page links or documented as manual URL fallback.

### 7. Make network capture summaries auditable

Observed issue:

`navigate.network_stores.count` can report a large number such as `52`, while the
inline `top` array intentionally contains only the top 5 metadata entries and no
body previews. If a UI also reports a separate source count, the page can appear
`Reuters.com`.

Design:

- Keep the bounded navigate summary so `navigate` stays small.
- Make truncation explicit with `top_limit`, `has_more`, and a `full_query_hint`.
- Add a host/source breakdown so a `sources` count is backed by visible evidence.
- Keep full bodies behind the `network_stores` RPC, where callers can request `limit: 100`, filter by `host`, or pass an explicit `nav_id`.

API additions:

| Field | Shape |
|---|---|
| `network_stores.top_limit` | Number of entries included inline. |
| `network_stores.has_more` | `true` when `count > top.length`. |
| `network_stores.source_hosts[]` | `[{host, count, bytes, top_score, kinds}]` for captured responses in the navigation scope. |
| `network_stores.full_query_hint` | Example params for `network_stores`, including `nav_id` and `limit`. |

Acceptance criteria:

- A page with 52 captures and 11 source hosts shows all 11 host names in the summary, even though only 5 top capture rows are embedded.
- The summary makes it clear that capture bodies require `network_stores {limit: 100, nav_id: <navigation_id>}`.
- `network_stores` RPC behavior remains backward-compatible.

### 8. Add semantic PageModel objects

Observed issue:

Even after cleaner text/cards/forms, agents still have to reconstruct page
structure mentally from flat evidence. The page may contain useful structure, but
the structure is lost when surfaced as separate link counts, text blobs, card
lists, form fields, and network summaries.

Implementation:

- Add `src/js/page_model.js` and expose `page_model {goal?, types?, limit?}`.
- Return semantic objects with stable IDs, object kind, fields, actions, score,
  confidence, and provenance.
- Initial object kinds: `search_form`, `form`, `nav_link`, `link`,
  `article_card`, `course_card`, `model_card`, `product_card`, `card`, `table`,
  `answer_block`, and `limitation`.
- Score objects against the optional `goal` so task-relevant forms/cards/links
  rise above generic site chrome.
- Preserve strict-mode limits by appending challenge/rate/browser-route metadata
  as `limitation` objects instead of implying the page is answerable.

Acceptance criteria:

- Synthetic search/model/article page produces `search_form`, `model_card`, and
  `article_card` objects with provenance and actions.
- Password-like form fields are redacted in object fields.
- Enable-JS/interstitial pages surface a `limitation` reason in `page_model`.
- Live smoke confirms Wikipedia search forms and NPR article cards are exposed as
  semantic objects.

### 9. Extract captured network data and action effects

Observed issue:

`network_stores` exposes useful API bodies, but agents still need to inspect raw
JSON previews manually. Separately, `click` tells whether a DOM event fired, but
not whether it produced task-useful effects.

Implementation:

- Add `network_extract {query?, types?, limit?, host?, nav_id?}`.
- Parse captured JSON, GraphQL-shaped JSON, and NDJSON bodies from the scoped
  `NetworkStore` entries.
- Return semantic network objects with `kind`, `title`, `url`, `text`, compact
  `fields`, `score`, `confidence`, `matched_terms`, and network provenance
  (`capture_id`, source URL, response kind, JSON path).
- Attach the same objects to `page_model.network_objects` and summarize counts in
  `page_model.summary.network_objects` / `network_captures`.
- Add `activate {ref?, text?}` to click by element ref or visible action text,
  settle the event loop, and return before/after URL, BlockMap/page_model
  summaries, network counts, text/DOM hashes, and a classification of
  `navigated`, `dom_changed`, `network_changed`, `no_effect`, or `unsupported`.
- Resolve relative URLs in JS `fetch()` via the Rust URL resolver before sending
  requests to the fetch worker.

Acceptance criteria:

- Synthetic JSON arrays produce product/network objects with redacted sensitive
  fields and capture/path provenance.
- `page_model` attaches network objects from the current navigation scope.
- A synthetic button that fetches JSON after click is classified as an effective
  activation and the fetched object is available via `network_extract`.

### 10. Build the iteration runner before rerunning

Observed issue:

Subagent runs are useful, but manual aggregation is fragile. The next iteration
needs a local runner/scorer that can call subagents or ingest subagent JSONL and
produce stable metrics.

Implementation tasks:

| Step | Files | Notes |
|---|---|---|
| Corpus loader | `train/webvoyager_eval.py` | Accept any JSONL with `task_id`, `web_name`, `start_url`, `question`, `expected_handling`. |
| Result validator | `train/webvoyager_eval.py` | Enforce shared result schema and friction keys. |
| Scorer | `train/webvoyager_eval.py` | Report answer success, handled success, handling counts, per-site table, friction totals. |
| Docs | `train/README.md` | Add commands for fast-regression and all-site tiers. |

Initial CLI shape:

```bash
python3 train/webvoyager_eval.py score \
  --corpus docs/webvoyager-site-coverage-v1.jsonl \
  --results docs/webvoyager-site-coverage-run-v1-2026-05-17.jsonl
```

Acceptance criteria:

- The scorer reproduces `8/15` answer success and `15/15` handled success for the checked-in all-site run.
- The scorer fails if any corpus site is missing from results.
- The scorer prints friction totals for `manual_url_guess`, `noisy_text`, `form_confusion`, `challenge_routed`, `browser_routed`, and `rate_limited`.

### Next iteration targets

After v5, the expected next improvement is still not to make Chrome-only sites
magically work; it is to remove the last page-owned route/data gap and keep
routing decisions explicit.

Targets for the next run:

| Metric | v5 current | Next target |
|---|---:|---:|
| All-site answer success | 8 / 15 | 9 / 15 or better |
| All-site handled success | 15 / 15 | 15 / 15 |
| `manual_url_guess` friction | 1 / 15 | 0 / 15 |
| `body_used` friction | 0 / 15 | 0 / 15 |
| `noisy_text` friction | 0 / 15 | 0 / 15 |
| `form_confusion` friction | 0 / 15 | 0 / 15 |
| Browser-routed with precise reason | 4 / 4 | 4 / 4 |
| AWS WAF routed correctly | 3 / 3 | 3 / 3 |

Likely answer-success gains:

- Cambridge should become cleaner via form summaries and text tools, even though it already answered.
- Google Flights may become either answerable or more precisely browser-routed if submit/action detection improves.
- Wolfram Alpha may remain browser-routed unless a stable non-rendered query endpoint is discovered from page-owned links/forms.
- Google Search and Maps should remain browser-routed unless the page exposes usable SSR result data.

## Branch and worktree setup

Start from a single integration branch, then create subagent branches from it:

Before rerunning this setup, remove stale worktrees with `git worktree remove` or
choose fresh paths; the example uses sibling directories outside the repo root.

```bash
git switch -c feature/webvoyager-usability
git worktree add ../ub-agent-blockmap -b agent/blockmap-forms
git worktree add ../ub-agent-text -b agent/text-tools
git worktree add ../ub-agent-lists -b agent/list-cards
git worktree add ../ub-agent-challenge -b agent/challenge-rate-limit
git worktree add ../ub-agent-eval -b agent/webvoyager-eval
```

## Workstreams

| Agent | Fixes | Primary files | Deliverables |
|---|---|---|---|
| A: BlockMap/forms | 1, 6 | `src/js/blockmap.js`, `src/js/interact.js`, `src/js/dom.js` | Rich form summaries, expandable link/button samples, correct `select` value serialization. |
| B: Text tools | 2, 3 | `src/main.rs`, `src/js/extract.js`, `README.md` | `find_text`, `text_around`, cleaner content modes, RPC/MCP docs. |
| C: Cards/lists | 7 | `src/js/extract.js`, `src/main.rs` if needed | Cleaner `extract_list`, optional `extract_cards` / auto-card helper, dedupe/noise stripping. |
| D: Challenge/rate-limit | 4, 5 | `src/challenge.rs`, `src/main.rs`, `scripts/router.py`, `scripts/watch.py` | First-class `rate_limited` / `429` metadata, better AWS WAF router/watch behavior. |
| E: Eval runner | 8 | `train/corpus/*`, `train/*.py`, `train/README.md` | Fixed fast-regression and all-site corpora, JSONL result schema, scorer comparing baseline vs candidate. |
| Integrator | All | All touched docs/tests | Merge branches, resolve API naming, run full score, open one implementation PR. |

Suggested timeboxes:

| Agent | Budget | Stop condition |
|---|---:|---|
| A: BlockMap/forms | 90 min | Smoke test demonstrates richer form summaries on a synthetic page. |
| B: Text tools | 90 min | RPC/MCP exposes localized text extraction and docs describe it. |
| C: Cards/lists | 60 min | Synthetic card/list smoke test proves cleaned titles/snippets. |
| D: Challenge/rate-limit | 60 min | Unit tests cover `429` and AWS WAF metadata/watch output. |
| E: Eval runner | 90 min | Runner can emit JSONL using the shared schema below. |
| Integrator | 120 min | All branches merged, validation run, fast-regression and all-site reruns scored. |

If a subagent hits its budget before finishing, it should stop with a short
handoff note covering what landed, what failed, and the smallest remaining next
step. This prevents the distributed run from drifting into open-ended research.

## Shared API contract

Keep existing fields backward-compatible wherever possible.

BlockMap additions:

| Field | Shape |
|---|---|
| `blockmap.interactives.links` | Existing count, unchanged. |
| `blockmap.interactives.buttons` | Existing count, unchanged. |
| `blockmap.interactives.link_samples` | `[{ref, text, href, aria_label, role}]` |
| `blockmap.interactives.button_samples` | `[{ref, text, type, aria_label, role}]` |
| `blockmap.interactives.forms[]` | `{ref, action, method, enctype, fields, controls, submitters, query_preview}` |
| `forms[].controls[]` | `{ref, tag, type, name, label, placeholder, value, checked, options}` |

Text tool additions:

| RPC | Purpose |
|---|---|
| `find_text {text, selector?, exact?, limit?, context_chars?}` | Return localized text hits with context: `{ref, tag, attrs, text, before, match, after}`. |
| `text_around {text? | ref?, selector?, context_chars?}` | Return text near a known string or element ref. |
| `text_clean {selector?, max_chars?}` | Strip `script`, `style`, `noscript`, embedded JSON, nav/footer/aside, and duplicate boilerplate. |
| `text_main` | Return the primary readable page text when the page exposes a main/article body. |

Challenge/rate-limit additions:

| Field | Shape |
|---|---|
| `navigate.challenge` | `null` or `{blocked, provider, confidence, status, matched, clearance_cookie, reason, hint}` |
| `navigate.rate_limit` | `{limited, status, retry_after, retry_after_seconds, hint}` |
| `navigate.browser_route` | `null` or `{needed, reason, confidence, evidence, hint}` |
| `challenge.provider` for AWS WAF | Keep `aws_waf`; ensure docs/router/watch all label it clearly. |
| `429` handling | Prefer `rate_limited` metadata over generic `unknown_block` when no vendor signature is present. |

List extraction additions:

| Area | Requirement |
|---|---|
| `extract_list` text cleanup | Ignore `script`, `style`, `noscript`; collapse whitespace; remove image fallback artifacts. |
| Card helper | Prefer common card/article/product/course selectors and return title/url/snippet/image/meta where available. |

Network/action additions:

| RPC / field | Purpose |
|---|---|
| `network_extract {query?, types?, limit?, host?, nav_id?}` | Convert captured JSON/API bodies into ranked semantic objects with provenance. |
| `page_model.network_objects[]` | Attach the top network-derived objects to the semantic page model. |
| `activate {ref?, text?}` | Click an action target and classify the observable result. |
| `route_discover {goal?, limit?}` | Return page-owned visible routes, forms, and inferred query URLs with provenance. |
| JS `fetch()` relative URL resolution | Ensure page-owned relative API calls are captured by the fetch worker. |

## Shared result schema

All subagents and the eval runner should emit one JSON object per task. Extra
fields are allowed, but stable keys and optional field semantics are defined in
`docs/webvoyager-data-format.md` so scoring can be automated.

```json
{
  "task_id": "ArXiv--17",
  "schema_version": "webvoyager-result-v1",
  "run_id": "webvoyager-v5-2026-05-17",
  "run_timestamp": "2026-05-17T22:18:30Z",
  "web_name": "ArXiv",
  "start_url": "https://arxiv.org/",
  "question": "Find the paper 'GPT-4 Technical Report', when was v3 submitted?",
  "success": true,
  "handled_success": true,
  "handling": "answered",
  "answer": "Mon, 27 Mar 2023 17:46:54 UTC",
  "confidence": 0.95,
  "steps": 15,
  "elapsed_ms": 90000,
  "unbrowser_signals": {
    "statuses": [200],
    "challenge_provider": null,
    "challenge": null,
    "likely_js_filled": false,
    "rate_limit": null,
    "browser_route": null
  },
  "friction": {
    "eval_used": false,
    "body_used": true,
    "manual_url_guess": true,
    "noisy_text": false,
    "form_confusion": true,
    "rate_limited": false,
    "challenge_routed": false,
    "browser_routed": false
  },
  "path_taken": ["navigate https://arxiv.org/", "submit search", "navigate abs page"],
  "failure_or_friction": "Search form selection was ambiguous."
}
```

Allowed `handling` values:

| Value | Meaning |
|---|---|
| `answered` | Task was answered directly. |
| `challenge_routed` | Task stopped on an expected challenge and exposed actionable metadata. |
| `rate_limited` | Task stopped on `429` or equivalent retry-later state. |
| `browser_routed` | Strict `unbrowser` cannot serve the required rendered/canvas/JS UI; use a browser. |
| `site_drift` | Benchmark target no longer exists or has materially changed. |
| `failed` | Any other failure. |

## Merge order

1. Merge Agent A first. It changes DOM/blockmap/interact internals and unblocks better form/search behavior.
2. Merge Agent C second. It changes JS extraction internals with limited API surface.
3. Merge Agent D third. Challenge/rate-limit behavior should land after Agent A's form/search improvements and before Agent B so text/search helpers can consume accurate `rate_limit` and challenge metadata instead of encoding their own retry heuristics.
4. Merge Agent B fourth. It wires final RPC/MCP API names and docs after A/C settle, and after D defines the final retry/escalation signals.
5. Merge Agent E fifth. The scorer/corpus should target the final result schema.
6. Integrator runs full tests, reruns both fixed corpora, commits final docs, and opens one implementation PR from `feature/webvoyager-usability`.

## Validation

Every subagent should run:

```bash
cargo test
cargo build --release
python3 -m unittest train.test_webvoyager_eval -v
```

Subagents should add local smoke scripts when useful, following the existing
`scripts/*_smoke.py` pattern with a local HTTP server instead of live network
dependencies.

## Fixed rerun corpora

Use the same tasks for baseline and candidate scoring. There are two required
tiers:

| Tier | Purpose | Artifact |
|---|---|---|
| Fast regression | Fast, previously observed 16-task set for no-regression checks. | `docs/webvoyager-baseline-v1-2026-05-16.jsonl` |
| All-site coverage | One representative read-only task for every unique WebVoyager site. | `docs/webvoyager-site-coverage-v1.jsonl` |

The fast-regression tier is:

| Task | Start URL | Expected handling |
|---|---|---|
| `ArXiv--17` | `https://arxiv.org/` | Answer. |
| `ArXiv--27` | `https://arxiv.org/` | Answer. |
| `ArXiv--0` | `https://arxiv.org/` | Answer or cleanly classify rate limit. |
| `BBC News--5` | `https://www.bbc.com/news/` | Answer. |
| `BBC News--28` | `https://www.bbc.com/news/` | Answer if possible; otherwise classify site drift. |
| `Apple--12` | `https://www.apple.com/` | Answer. |
| `Apple--6` | `https://www.apple.com/` | Answer. |
| `Apple--19` | `https://www.apple.com/` | Answer. |
| `Allrecipes--40` | `https://www.allrecipes.com/` | Answer. |
| `Allrecipes--41` | `https://www.allrecipes.com/` | Answer. |
| `Amazon--0` | `https://www.amazon.com/` | Correctly route `aws_waf`. |
| `Booking--0` | `https://www.booking.com/` | Correctly route `aws_waf`. |
| `GitHub--29` | `https://github.com/` | Answer. |
| `GitHub--37` | `https://github.com/` | Answer. |
| `Coursera--37` | `https://www.coursera.org/` | Answer. |
| `ESPN--18` | `https://www.espn.com/` | Correctly route `aws_waf`. |

The all-site tier covers every unique site in upstream WebVoyager:

| Site | Task | Start URL | Expected handling |
|---|---|---|---|
| Allrecipes | `Allrecipes--40` | `https://www.allrecipes.com/` | Answer. |
| Amazon | `Amazon--0` | `https://www.amazon.com/` | Challenge route or answer with valid clearance. |
| Apple | `Apple--6` | `https://www.apple.com/` | Answer. |
| ArXiv | `ArXiv--27` | `https://arxiv.org/` | Answer. |
| BBC News | `BBC News--5` | `https://www.bbc.com/news/` | Answer. |
| Booking | `Booking--0` | `https://www.booking.com/` | Challenge route or answer with valid clearance. |
| Cambridge Dictionary | `Cambridge Dictionary--0` | `https://dictionary.cambridge.org/` | Answer. |
| Coursera | `Coursera--37` | `https://www.coursera.org/` | Answer. |
| ESPN | `ESPN--18` | `https://www.espn.com/` | Challenge route or answer with valid clearance. |
| GitHub | `GitHub--37` | `https://github.com/` | Answer. |
| Google Flights | `Google Flights--0` | `https://www.google.com/travel/flights/` | Answer or route to browser if rendered/search UI is unavailable. |
| Google Map | `Google Map--0` | `https://www.google.com/maps/` | Answer or route to browser if rendered/map UI is unavailable. |
| Google Search | `Google Search--0` | `https://www.google.com/` | Answer or route challenge/rate limit. |
| Huggingface | `Huggingface--0` | `https://huggingface.co/` | Answer. |
| Wolfram Alpha | `Wolfram Alpha--0` | `https://www.wolframalpha.com/` | Answer or route to browser if rendered result UI is unavailable. |

The full upstream corpus has 643 tasks across these 15 sites. Running all 643 is
out of scope for the first implementation PR, but the eval runner should not bake
in the 16-task assumption; it should accept arbitrary JSONL corpora so a full
benchmark pass can be added later.

## Scoring

Track two top-level scores:

| Score | Meaning |
|---|---|
| Answer success | The task was answered correctly. |
| Handled success | The task was answered correctly or correctly routed as expected challenge/site drift/rate-limit/browser-route. |

Also track friction counters from subagent reports:

| Counter | Meaning |
|---|---|
| `eval_used` | Agent needed raw JS extraction. |
| `body_used` | Agent needed full raw HTML/body fallback. |
| `manual_url_guess` | Agent had to guess a URL instead of discovering it from page structure. |
| `noisy_text` | Text extraction included large irrelevant nav/JSON/duplicate content. |
| `form_confusion` | Agent selected the wrong form/input/submit path. |
| `rate_limited` | Site returned `429` or equivalent. |
| `challenge_routed` | Challenge was correctly surfaced without retries. |
| `browser_routed` | Strict `unbrowser` correctly identified a rendered/canvas/JS-only route. |

Candidate acceptance targets:

- No regressions on the 11 successful baseline tasks.
- Preserve `3 / 3` correct `aws_waf` routing.
- Improve `ArXiv--0` from `unknown_block` to clean `rate_limited` handling with at most one retry after the first `429`, or complete it if possible without unsafe retrying.
- Execute all 15 sites in `docs/webvoyager-site-coverage-v1.jsonl` and report answer success plus handled success by site against `docs/webvoyager-site-coverage-run-v1-2026-05-17.jsonl`.
- Do not silently skip any benchmark site; a challenge, rate limit, browser-route decision, or site-drift classification is a valid handled outcome, but a missing site is not.
- Reduce friction counters, especially `eval_used`, `body_used`, `manual_url_guess`, `noisy_text`, and `form_confusion`.
