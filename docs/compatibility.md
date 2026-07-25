# Compatibility, detection, and escalation

`unbrowser` is a cheap-first browser tier, not a Chrome replacement. It can read and interact with SSR pages cheaply, can run bounded QuickJS scripts when requested, and tells an agent when the page needs a real browser.

## Default mode and script mode

`navigate` defaults to fetch + parse: the result is the server-rendered HTML, a stateful cookie jar, a virtual DOM, and a BlockMap. This is intentionally fast and predictable.

Set `exec_scripts: true` for inline and external page scripts:

```json
{"id":1,"method":"navigate","params":{"url":"https://example.com","exec_scripts":true}}
```

QuickJS can handle light hydration, script-visible JSON, simple fetch/XHR flows, and some module loaders. It is not V8 and it does not render pixels. Script execution is watchdog-bounded so a hostile or heavy page returns instead of wedging the process.

## Practical compatibility tiers

| Page tier | Expectation | Recommended action |
|---|---|---|
| Static / SSR docs, news, search, repositories | Excellent | Default `navigate`, then query/extract. |
| SSR plus light hydration | Usually usable | Read SSR DOM first; opt into scripts only if necessary. |
| Bot wall with reusable clearance cookie | Usable with handoff | Solve in real Chrome once, call `cookies_set`, retry. |
| Module-loader app / light script app | Partial | Try `exec_scripts: true`; inspect resulting DOM and routes. |
| Heavy React/Vue/Ember dashboard | Bounded but incomplete | Use signals below, then escalate. |
| Workers, Canvas, WebGL, IndexedDB-dependent app | Out of scope | Escalate to real Chrome. |
| Behavioral challenges / Kasada / advanced Akamai | Out of scope | Real Chrome, human interaction, or a trusted solver. |

## Tested examples

The following are useful regression examples, not a compatibility guarantee: Wikipedia, MDN, docs.rs, Hacker News, BBC, arXiv listings, Google/Bing/Brave/DuckDuckGo HTML search, GitHub repository pages, npm, Stack Overflow, CoinGecko, Yahoo Finance, and archive.org. Sites can change their rendering or anti-bot behavior without notice; use the signals below instead of assuming a domain will always remain in one tier.

## Signals that tell an agent to change tier

`blockmap.density` avoids spending repeated calls on an empty app shell:

- `thin_shell: true` — almost no text, headings, or interactives; common React/Ember root.
- `likely_js_filled: true` — empty table/list/cell shells or many scripts with little visible UI.
- `json_scripts: N` — application/json script tags may contain the data without rendering it.
- `script_heavy_shell: true` — lots of scripts, little useful static UI.

For bot walls, `navigate.challenge` identifies the likely provider (`cloudflare_turnstile`, `datadome`, `perimeterx_block`, `arkose_labs`, `recaptcha`, and others), status, confidence, and clearance-cookie hint. Use the signal to choose a recovery path; it is not a claim that the binary can solve every challenge.

## Cookie handoff

Cookies are held in process memory and automatically sent to matching domains. Replay a cookie obtained from a real browser:

```json
{"id":1,"method":"cookies_set","params":{"cookies":[{"name":"cf_clearance","value":"…","domain":"example.com","secure":true}]}}
```

Then navigate again in the same session. Clearance cookies expire, so replay works only for their lifetime.

### Local cookie solver service

Install the optional local Chrome/CDP helper when a workflow needs transparent cookie handoff:

```bash
pip install 'pyunbrowser[solver]'
unbrowser cookie-service --headless --profile unbrowser-cookie-service
unbrowser router https://example.com/protected
```

The service is loopback-only by default and returns cookies from the user's own Chrome/Unchained profile; it does not fabricate challenge tokens. Keep it on `127.0.0.1`. Non-loopback binds and remote cookie-service URLs require explicit opt-in because they can expose browser cookies. Use `--allow-host` to restrict target domains.

The equivalent explicit path is:

```text
challenge detected → real Chrome obtains cookie → cookies_set → retry once
```

## Escalation choices

| Need | Use |
|---|---|
| Cheap SSR extraction, stateful forms, cookie replay, route discovery | `unbrowser` |
| Real V8, pixels, extensions, authenticated browser profile, active challenge solve | [`unchainedsky-cli`](https://github.com/protostatis/unchainedsky-cli) / local Chrome |
| Human-in-the-loop browser agent | [Unchained](https://unchainedsky.com) |

The vocabulary transfers across tiers: `navigate`, `query`, `click`, `type`, and cookies are deliberately similar.

## Honest limits

- No screenshots or visual layout inspection.
- No full V8 compatibility or JIT performance.
- Multipart form uploads are not supported; construct the request manually or escalate.
- Script execution is opt-in and bounded by `UNBROWSER_TIMEOUT_MS`.
- Browser-shaped requests can improve compatibility over a raw HTTP client, but they are not a guarantee against dedicated fingerprinting or challenge systems.
