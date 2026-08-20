"""Smart wrapper: infers search vs navigate.

Two-entry inference:
  1. search:<query>  -> Brave HTML search via unbrowser (fallback to Brave API if BRAVE_API_KEY set)
  2. navigate:<url>  -> navigate + auto discover (discover + cards + page_model)

Also exposes `run(task)` which infers from raw string: URL-looking -> navigate_auto,
otherwise -> brave search.

Example:
    from unbrowser.smart import SmartClient

    with SmartClient() as ub:
        # entry 1: search
        hits = ub.search("Pixel 11 review", engine="brave")  # -> [{title,url,snippet,display_url}]
        # entry 2: navigate + auto discover
        bundle = ub.navigate_auto(hits[0]["url"], goal="Pixel 11 review")
        # bundle = {navigate, blockmap, discover, cards, page_model, extract}

        # or infer
        bundle = ub.run("https://www.engadget.com/2240042/google-pixel-11-review/")
        hits   = ub.run("Pixel 11 review")

Requires UNBROWSER_BIN to point to a recent build that exposes discover/extract_cards/page_model
for the auto bundle to be rich; otherwise it gracefully degrades.
"""

from __future__ import annotations

import json
import os
import re
from typing import Any
from urllib.parse import quote_plus, urlparse
from urllib.request import Request, urlopen

from . import Client, UnbrowserError

BRAVE_SEARCH_HTML = "https://search.brave.com/search?q={q}&source=web"
BRAVE_API_ENDPOINT = "https://api.search.brave.com/res/v1/web/search"

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

_URL_RE = re.compile(r"^https?://", re.I)


def is_url(s: str) -> bool:
    s = s.strip()
    if _URL_RE.match(s):
        return True
    # bare domain like engadget.com/foo -> treat as url
    try:
        p = urlparse(s if "://" in s else "https://" + s)
        return bool(p.netloc and "." in p.netloc and " " not in s)
    except Exception:
        return False


def _norm_url(u: str, base: str | None = None) -> str:
    u = u.strip()
    if not u:
        return u
    # absolute URL
    if _URL_RE.match(u):
        return u
    # protocol-relative //cdn.example.com/foo
    if u.startswith("//"):
        return "https:" + u
    # root-relative or relative: join with base (last_url) if available
    if base and (u.startswith("/") or not _URL_RE.match(u)):
        try:
            from urllib.parse import urljoin

            return urljoin(base, u)
        except Exception:
            pass
    return "https://" + u.lstrip("/")


# ---------------------------------------------------------------------------
# Brave HTML extraction
# ---------------------------------------------------------------------------

_BRAVE_SNIPPET_JS = r"""
(function(q){
  const snippets = document.querySelectorAll('.snippet[data-type="web"]');
  const out = [];
  const limit = q.limit || 10;
  for (let i=0;i<snippets.length && out.length<limit;i++){
    const el = snippets[i];
    const a = el.querySelector('a[href^="http"]');
    if (!a) continue;
    const href = a.getAttribute('href') || a.href;
    // Brave sometimes wraps with tracking redirect; keep as-is — caller can follow
    const titleEl = el.querySelector('.title, .snippet-title, [class*="title"]');
    const title = (titleEl ? titleEl.textContent.trim() :
                   a.textContent.trim()).replace(/\s+/g,' ').slice(0,300);
    // description/snippet: Brave uses multiple possible selectors
    let snippet = "";
    const cands = el.querySelectorAll('.snippet-content, .snippet-description, p, [class*="snippet"] p, [class*="desc"]');
    for (const c of cands){
      const t = (c.textContent||"").trim().replace(/\s+/g,' ');
      if (t.length > 40 && !t.startsWith("http")){
        snippet = t.slice(0,500);
        break;
      }
    }
    if (!snippet){
      // fallback: whole snippet text minus title
      const all = el.textContent.replace(/\s+/g,' ').trim();
      snippet = all.slice(title.length, title.length+400).trim();
    }
    const cite = el.querySelector('cite');
    const display_url = cite ? cite.textContent.trim().replace(/\s+/g,' ').slice(0,120) : href;
    // filter out non-web snippets that slipped through
    if (!href || href.includes("search.brave.com")) continue;
    out.push({title, url: href, snippet, display_url});
  }
  return JSON.stringify(out);
})
"""


def _ddg_html_extract(client: Client, count: int = 10) -> list[dict]:
    """Fallback DDG HTML extraction (used when Brave 429s)."""
    seen: set[str] = set()
    out: list[dict] = []
    for a in client.query("a.result__a"):
        href = (a.get("attrs", {}).get("href") or "").strip()
        # DDG wraps as //duckduckgo.com/l/?uddg=...
        if "uddg=" in href:
            try:
                from urllib.parse import parse_qs, urlparse as _up
                href = parse_qs(_up(href).query).get("uddg", [href])[0]
            except Exception:
                pass
        if href.startswith("//"):
            href = "https:" + href
        if not href.startswith("http") or "duckduckgo.com" in href:
            continue
        if href in seen:
            continue
        seen.add(href)
        txt = (a.get("text") or "").strip()[:200]
        if len(txt) < 8:
            continue
        out.append({"title": txt, "url": href, "snippet": "", "display_url": href})
        if len(out) >= count:
            break
    # generic fallback if selector missed (DDG varies)
    if not out:
        for a in client.query("a"):
            href = (a.get("attrs", {}).get("href") or "").strip()
            if not href.startswith("http") or "duckduckgo.com" in href:
                continue
            if href in seen:
                continue
            if len((a.get("text") or "").strip()) < 10:
                continue
            seen.add(href)
            out.append({"title": (a.get("text") or "").strip()[:200], "url": href, "snippet": "", "display_url": href})
            if len(out) >= count:
                break
    return out


def _brave_html_extract(client: Client, query: str, count: int = 10) -> list[dict]:
    url = BRAVE_SEARCH_HTML.format(q=quote_plus(query))
    nav = client.navigate(url)
    if nav.get("status", 200) >= 400:
        # 429/503 rate-limit on Brave HTML — fallback to DDG HTML so the
        # two-entry flow still completes (same parsed shape).
        if nav.get("status") in (429, 503):
            try:
                ddg_url = "https://duckduckgo.com/html/?q=" + quote_plus(query)
                dnav = client.navigate(ddg_url)
                if dnav.get("status", 200) < 400:
                    return _ddg_html_extract(client, count=count)
            except Exception:
                pass
        raise UnbrowserError(f"brave search navigate failed: {nav.get('status')} {url}")
    # Prefer structured JS extraction; fallback to generic link scan
    try:
        raw = client.eval(f"({_BRAVE_SNIPPET_JS})({{limit:{int(count)}}})")
        # eval returns Python value already JSON-decoded if valid JSON string? But our JS returns JSON string.
        # So raw may be a JSON string.
        if isinstance(raw, str):
            try:
                items = json.loads(raw)
            except Exception:
                items = []
        elif isinstance(raw, list):
            items = raw
        else:
            items = []
        if items:
            return items[:count]
    except Exception:
        pass
    # fallback: take brave links that are https and not search.brave.com, dedup
    seen = set()
    out: list[dict] = []
    for a in client.query("a"):
        href = (a.get("attrs", {}).get("href") or "").strip()
        if not href.startswith("http") or "search.brave.com" in href:
            continue
        if href in seen:
            continue
        seen.add(href)
        txt = (a.get("text") or "").strip().replace("\n", " ")[:200]
        if len(txt) < 10:
            continue
        out.append({"title": txt, "url": href, "snippet": "", "display_url": href})
        if len(out) >= count:
            break
    return out


def _brave_api_extract(query: str, count: int = 10) -> list[dict] | None:
    key = os.environ.get("BRAVE_API_KEY") or os.environ.get("BRAVE_SEARCH_API_KEY")
    if not key:
        return None
    try:
        req = Request(
            f"{BRAVE_API_ENDPOINT}?q={quote_plus(query)}&count={count}",
            headers={"Accept": "application/json", "X-Subscription-Token": key},
        )
        with urlopen(req, timeout=10) as resp:
            data = json.loads(resp.read().decode("utf-8"))
        results = data.get("web", {}).get("results", []) or data.get("results", [])
        out = []
        for r in results[:count]:
            out.append({
                "title": r.get("title", "")[:300],
                "url": r.get("url", ""),
                "snippet": (r.get("description") or r.get("snippet") or "")[:500],
                "display_url": r.get("url", "")[:120],
            })
        return out
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Help catalog (grouped 32) for progressive discovery
# ---------------------------------------------------------------------------

HELP_CATALOG: dict[str, Any] = {
    "core": {
        "search": {"when": "find URLs for a query", "example": "ub.search('Pixel 11 review', count=5)"},
        "open": {"when": "fetch URL + auto discover routes/cards", "example": "ub.navigate_auto('https://example.com', goal='Pixel 11')"},
        "extract": {"when": "auto-strategy structured data (JSON-LD, Next, Nuxt, OG)", "example": "ub.call('extract')"},
        "help": {"when": "discover full 32", "example": "ub.help() or ub.help('query')"},
    },
    "query_text": {
        "query": {"when": "CSS → [{ref, tag, attrs, text}] (stable refs for click/type)"},
        "query_debug": {"when": "diagnose selector miss (hints: selector_miss, thin_shell, embedded_json)"},
        "query_text": {"when": "find by visible text (hashed React classes, anchor-promotion)"},
        "find_text": {"when": "ranked text matches with before/after context"},
        "text_around": {"when": "400-char window around ref or text match"},
    },
    "reading": {
        "text": {"when": "textContent of first match (default body) — hatnote trap"},
        "text_main": {"when": "main content (excludes header/nav/footer)"},
        "text_clean": {"when": "chrome+JSON stripped, whitespace-collapsed"},
        "blockmap": {"when": "recompute BlockMap after DOM mutation"},
        "body": {"when": "raw HTML fallback (100KB+)"},
    },
    "discovery": {
        "page_model": {"when": "semantic objects (search_form, article_card, product_card...)"},
        "route_discover": {"when": "ranked links/forms + inferred query URLs"},
        "discover": {"when": "merged DOM+inferred+network graph with provenance (use this before guessing URLs)"},
        "network_extract": {"when": "parse captured JSON/API into semantic objects"},
        "network_stores": {"when": "ranked fetch/XHR captures (256KB preview)"},
    },
    "extraction": {
        "extract": {"when": "auto-strategy JSON-LD → Next → Nuxt → OG → microdata"},
        "extract_cards": {"when": "repeated cards → [{title, price, url, snippet}]"},
        "extract_list": {"when": "explicit {item_selector, fields: {name: 'sel @attr'}}"},
        "extract_table": {"when": "table → {headers, rows}"},
        "table_to_json": {"when": "alias for extract_table (default table)"},
    },
    "interaction": {
        "click": {"when": "dispatch click on e:NN (auto-follows <a href>)"},
        "activate": {"when": "probe click → {navigated, dom_changed, network_changed, no_effect}"},
        "type": {"when": "set input value + input/change events"},
        "submit": {"when": "form → GET or x-www-form-urlencoded POST"},
        "settle": {"when": "drain microtasks + timers"},
        "eval": {"when": "QuickJS JS (bounded, use for price selectors)"},
    },
    "session": {
        "cookies_set": {"when": "replay clearance cookie (_px3 etc) from real Chrome"},
        "cookies_get": {"when": "export jar"},
        "cookies_clear": {"when": "clear jar"},
        "report_outcome": {"when": "bind success/failure to navigation_id for policy learning"},
        "network_stores_clear": {"when": "drop captures"},
    },
}


def _help_catalog(topic: str | None = None) -> dict:
    if topic:
        t = topic.lower()
        for group, tools in HELP_CATALOG.items():
            if t == group or t in tools:
                return {group: tools} if t == group else {t: tools.get(t, {})}
        # fuzzy: search in descriptions
        out: dict[str, Any] = {}
        for group, tools in HELP_CATALOG.items():
            for name, info in tools.items():
                if t in name or t in str(info.get("when", "")).lower():
                    out.setdefault(group, {})[name] = info
        return out or HELP_CATALOG
    return HELP_CATALOG


def _next_tools_from_bundle(bundle: dict) -> list[dict]:
    """Build next_tools from navigate signals + tool_likelihoods."""
    nxt: list[dict] = []
    raw = bundle.get("raw") or {}
    recs = raw.get("tool_recommendations") or []
    likes = raw.get("tool_likelihoods") or {}
    for name in recs[:6]:
        nxt.append({"tool": name, "when": HELP_CATALOG.get("core", {}).get(name, {}).get("when") or "recommended", "confidence": float(likes.get(name, 0.7))})
    if not nxt:
        bm = bundle.get("blockmap") or {}
        density = bm.get("density") or {}
        if density.get("likely_js_filled"):
            nxt.append({"tool": "eval", "when": "JS-gated content, inspect script JSON", "confidence": 0.85})
        if (bundle.get("cards") is not None and len(bundle.get("cards") or []) == 0):
            nxt.append({"tool": "extract_list", "when": "cards missed, try explicit fields", "confidence": 0.6})
    return nxt[:6]


def _escalation_for_bundle(bundle: dict) -> dict | None:
    """Portable escalation: stable reason + evidence + severity + retryable.

    Rust emits facts (challenge, status, density, scripts, extract); Python maps
    to host actions. This keeps reason codes stable and avoids phantom tools.
    """
    status = bundle.get("status")
    challenge = bundle.get("challenge")
    bm = bundle.get("blockmap") or {}
    density = bm.get("density") or {}
    extract = bundle.get("extract") or {}
    scripts = bundle.get("scripts") or {}

    # 1. challenge / bot wall — portable fact from Rust
    if challenge:
        provider = challenge.get("provider") or challenge.get("vendor") or "unknown"
        return {
            "reason": "challenge",
            "category": "external_capability",
            "confidence": float(challenge.get("confidence", 0.9)) if isinstance(challenge.get("confidence"), (int, float)) else 0.9,
            "severity": "high",
            "retryable": False,
            "evidence": {"provider": provider, "status": status, "clearance_cookie": challenge.get("clearance_cookie")},
            "hint": "Continue using session state from a user-authorized browser, where permitted. Acquire a clearance cookie in real Chrome for this origin and replay via cookies_set.",
            "options": [
                {"action": "replay_clearance_cookie", "tool": "cookies_set", "params": {"cookies": [{"name": challenge.get("clearance_cookie") or "_px3", "value": "<from user-authorized Chrome>", "domain": urlparse(bundle.get("url") or "").hostname or "example.com"}]}, "requires_user_confirmation": True},
                {"action": "external_action", "external_action": "chrome_escalation", "reason": provider, "params": {"reason": provider}},
                {"action": "try_help", "tool": "help", "params": {"topic": "session"}},
            ],
            "next_tools": [{"tool": "cookies_set", "when": "replay user-authorized cookie", "confidence": 0.9}],
        }
    # 2. http errors — split per advisor
    if isinstance(status, int) and status >= 400:
        if status in (401, 403):
            return {"reason": "auth_required", "category": "external_capability", "confidence": 0.95, "severity": "high", "retryable": False, "evidence": {"status": status}, "hint": f"HTTP {status} auth required or blocked. Acquire session state from a user-authorized browser if permitted.", "options": [{"action": "external_action", "external_action": "chrome_escalation", "reason": "auth"}, {"action": "try_help", "tool": "help", "params": {"topic": "session"}}], "next_tools": [{"tool": "help", "when": "session", "confidence": 0.7}]}
        if status == 404:
            return {"reason": "not_found", "category": "terminal", "confidence": 0.95, "severity": "low", "retryable": False, "evidence": {"status": status}, "hint": f"HTTP {status} not found — terminal, do not retry.", "options": [{"action": "try_help", "tool": "help", "params": {"topic": "discovery"}}], "next_tools": []}
        if status == 429:
            return {"reason": "rate_limited", "category": "retry", "confidence": 0.9, "severity": "medium", "retryable": True, "evidence": {"status": status}, "hint": f"HTTP {status} rate-limited. Back off and retry, or switch search provider (Brave→DDG fallback already handles this for search).", "options": [{"action": "retry_backoff", "tool": "open", "params": {"url": bundle.get("url")}}, {"action": "try_help", "tool": "help", "params": {"topic": "session"}}], "next_tools": [{"tool": "open", "when": "retry with backoff", "confidence": 0.6}]}
        if status >= 500:
            return {"reason": "server_error", "category": "retry", "confidence": 0.8, "severity": "medium", "retryable": True, "evidence": {"status": status}, "hint": f"HTTP {status} server error — retryable.", "options": [{"action": "retry", "tool": "open", "params": {"url": bundle.get("url")}}], "next_tools": [{"tool": "open", "when": "retry", "confidence": 0.6}]}
        return {"reason": "http_error", "category": "retry", "confidence": 0.8, "severity": "medium", "retryable": True, "evidence": {"status": status}, "hint": f"HTTP {status} — check challenge field and retry.", "options": [{"action": "try_help", "tool": "help", "params": {"topic": "session"}}], "next_tools": [{"tool": "help", "when": "session", "confidence": 0.7}]}
    # 3. timeout — enrichment bounded
    if bundle.get("discover_timeout") or bundle.get("cards_timeout") or bundle.get("page_model_timeout"):
        return {
            "reason": "timeout",
            "category": "retry",
            "confidence": 0.7,
            "severity": "medium",
            "retryable": True,
            "evidence": {"discover_timeout": bool(bundle.get("discover_timeout")), "cards_timeout": bool(bundle.get("cards_timeout"))},
            "hint": "Enrichment timed out (heavy DOM). Retry with smaller limits or skip discover.",
            "options": [
                {"action": "retry_smaller", "tool": "open", "params": {"url": bundle.get("url"), "discover_limit": 3, "cards_limit": 3}},
                {"action": "skip_discover", "tool": "extract_cards", "params": {"limit": 5}},
                {"action": "try_help", "tool": "help", "params": {"topic": "extraction"}},
            ],
            "next_tools": [{"tool": "extract_cards", "when": "cards only", "confidence": 0.7}],
        }
    # 4. unsupported JS feature / thin shell — stable codes
    if density.get("thin_shell"):
        return {"reason": "thin_shell", "category": "continuation", "confidence": 0.7, "severity": "medium", "retryable": True, "evidence": {"thin_shell": True}, "hint": "SSR shell with little content — try exec_scripts or check embedded JSON.", "options": [{"action": "retry_exec_scripts", "tool": "open", "params": {"url": bundle.get("url"), "exec_scripts": True}}, {"action": "try_extract", "tool": "extract", "params": {}}], "next_tools": [{"tool": "extract", "when": "auto-strategy", "confidence": 0.7}]}
    if density.get("likely_js_filled"):
        return {"reason": "unsupported_js_feature", "category": "external_capability", "confidence": 0.85, "severity": "high", "retryable": False, "evidence": {"likely_js_filled": True, "script_errors": (scripts.get("errors") or [])[:2]}, "hint": "Client-rendered content requires JS features not supported by QuickJS (e.g. ES modules, import maps, WASM). Use extract for embedded JSON or escalate to real Chrome, where permitted.", "options": [{"action": "extract_alternate", "tool": "extract", "params": {"strategy": "json_in_script"}}, {"action": "external_action", "external_action": "chrome_escalation", "reason": "unsupported_js_feature"}], "next_tools": [{"tool": "extract", "when": "try json_in_script / nuxt_data", "confidence": 0.8}]}
    # check ES-module specifically (REI case) even when likely_js_filled is false
    errs = " ".join(str(e) for e in (scripts.get("errors") or []))
    if "export" in errs or "import" in errs:
        return {"reason": "unsupported_js_feature", "category": "external_capability", "confidence": 0.85, "severity": "high", "retryable": False, "evidence": {"script_errors": (scripts.get("errors") or [])[:2]}, "hint": "QuickJS cannot run ES-module bundles (export/import). Extract embedded data or use a real browser, where permitted.", "options": [{"action": "extract_alternate", "tool": "extract", "params": {"strategy": "nuxt_data"}}, {"action": "external_action", "external_action": "chrome_escalation", "reason": "ES-module"}], "next_tools": [{"tool": "extract", "when": "nuxt_data", "confidence": 0.8}]}
    # 5. partial_result — supersedes old extract_truncated (informational, not escalation)
    if extract.get("primary_truncated"):
        pt = extract["primary_truncated"]
        return {
            "reason": "partial_result",
            "category": "continuation",
            "confidence": 0.8,
            "severity": "low",
            "retryable": True,
            "evidence": {"strategy": pt.get("strategy"), "size_bytes": pt.get("size_bytes")},
            "hint": f"Primary {pt.get('strategy')} {pt.get('size_bytes')} bytes exceeds inline cap; call extract(strategy=\"{pt.get('strategy')}\") for full.",
            "options": [{"action": "fetch_full_extract", "tool": "extract", "params": {"strategy": pt.get("strategy")}}, {"action": "try_help", "tool": "help", "params": {"topic": "extraction"}}],
            "next_tools": [{"tool": "extract", "when": f"strategy={pt.get('strategy')}", "confidence": 0.9}],
        }
    # 6. cards miss
    cards = bundle.get("cards")
    if isinstance(cards, list) and len(cards) == 0 and (density.get("li", {}) or {}).get("total", 0) > 20:
        return {"reason": "cards_miss", "category": "continuation", "confidence": 0.6, "severity": "low", "retryable": True, "evidence": {"li_total": density.get("li", {}).get("total")}, "hint": "extract_cards returned 0 but DOM has many list items — try explicit selectors.", "options": [{"action": "try_extract_list", "tool": "extract_list", "params": {"item_selector": "article", "fields": {"title": "h3"}}}, {"action": "try_page_model", "tool": "page_model", "params": {}}], "next_tools": [{"tool": "extract_list", "when": "explicit fields", "confidence": 0.6}]}
    return None


# ---------------------------------------------------------------------------
# SmartClient
# ---------------------------------------------------------------------------

class SmartClient(Client):
    """Client with inference: search (Brave) vs navigate+auto discover."""

    # ---- search -----------------------------------------------------------

    def search(self, query: str, engine: str = "brave", count: int = 10, **kw) -> Any:  # type: ignore[override]
        """Search via engine; Brave is the default for the smart wrapper.

        engine:
          brave  — Brave HTML via unbrowser (or Brave API if BRAVE_API_KEY env is set)
          ddg    — DuckDuckGo HTML (via base Client)
          bing   — Bing search (via base Client)

        Returns:
          brave: list[{title,url,snippet,display_url}]
          ddg/bing: base Client.navigate result dict (for backwards compat)
        """
        eng = (engine or "brave").lower()
        if eng == "brave":
            # Prefer API if available (no browser round-trip for search page)
            api = _brave_api_extract(query, count=count)
            if api is not None and api:
                return api
            return _brave_html_extract(self, query, count=count)
        # delegate to base for ddg/bing
        return super().search(query, engine=eng)  # type: ignore[arg-type]

    def search_brave(self, query: str, count: int = 10) -> list[dict]:
        return self.search(query, engine="brave", count=count)  # type: ignore[return-value]

    # ---- navigate + auto discover -----------------------------------------

    def help(self, topic: str | None = None) -> dict:
        """Discover full tooling from the minimal 3.

        topic: None -> full grouped catalog, or filter e.g. 'query', 'extraction', 'interaction'
        Returns the HELP_CATALOG grouped dict so agents can auto-discover the 32 beyond search/open/extract.
        """
        return _help_catalog(topic)

    def navigate_auto(
        self,
        url: str,
        goal: str | None = None,
        exec_scripts: bool = False,
        discover_limit: int = 12,
        cards_limit: int = 10,
        include_page_model: bool = False,
        timeout: float = 8.0,
    ) -> dict:
        """Navigate then auto-run discovery (bounded) + escalation.

        Returns bundle:
          {
            url, status, blockmap, headers,
            discover: {...} | None,
            cards: [...] | None,        # extract_cards
            page_model: {...} | None,
            extract: {...} | None,      # auto-strategy
            escalation: {...} | None,   # reason/hint/options when retry/escalation is useful
            next_tools: [...],          # recommended next tools
          }
        On failure, escalation.options tells the agent what to try next
        (retry, try alternate extractor, escalate_to_chrome, help).
        """
        import concurrent.futures as _cf

        def _timed_call(method: str, kw: dict, tm: float = timeout):
            # run Client.call in a thread so we can bound it without blocking
            # the main thread. On timeout we abandon the worker (Python threads
            # cannot be killed) and return a timeout marker — the worker will
            # eventually complete and be reaped when the process exits.
            ex = _cf.ThreadPoolExecutor(max_workers=1)
            fut = ex.submit(self.call, method, **kw)
            try:
                res = fut.result(timeout=tm)
                ex.shutdown(wait=True)
                return res
            except _cf.TimeoutError:
                try:
                    fut.cancel()
                except Exception:
                    pass
                ex.shutdown(wait=False, cancel_futures=True)
                return {"_timeout": True, "error": f"{method} timed out after {tm}s"}
            except Exception:
                ex.shutdown(wait=False, cancel_futures=True)
                raise

        # handle relative hrefs like "/" from query("a") results
        url = _norm_url(url, self._last_url)
        nav = super().navigate(url, exec_scripts=exec_scripts)
        bundle: dict[str, Any] = {
            "url": nav.get("url", url),
            "status": nav.get("status"),
            "blockmap": nav.get("blockmap"),
            "headers": nav.get("headers"),
            "challenge": nav.get("challenge"),
            "scripts": nav.get("scripts"),
            "extract": nav.get("extract"),
            "raw": nav,
        }
        if goal is None:
            try:
                goal = (nav.get("blockmap", {}) or {}).get("title", "")[:120] or None
            except Exception:
                goal = None

        # enrichment is best-effort and time-bounded
        enrichments = []
        enrichments.append(("discover", {"goal": goal, "limit": discover_limit} if goal else {"limit": discover_limit}))
        enrichments.append(("cards", {"limit": cards_limit}))
        if include_page_model:
            enrichments.append(("page_model", {"goal": goal} if goal else {}))

        for name, call_kw in enrichments:
            try:
                if name == "discover":
                    res = _timed_call("discover", call_kw)
                    if isinstance(res, dict) and res.get("_timeout"):
                        bundle["discover"] = None
                        bundle["discover_timeout"] = True
                    else:
                        bundle["discover"] = res
                elif name == "cards":
                    try:
                        res = _timed_call("extract_cards", call_kw)
                        if isinstance(res, dict) and res.get("_timeout"):
                            bundle["cards"] = None
                            bundle["cards_timeout"] = True
                        else:
                            bundle["cards"] = res
                    except UnbrowserError as e:
                        if "unknown method" in str(e):
                            bundle["cards"] = None
                        else:
                            raise
                elif name == "page_model":
                    res = _timed_call("page_model", call_kw)
                    if isinstance(res, dict) and res.get("_timeout"):
                        bundle["page_model"] = None
                        bundle["page_model_timeout"] = True
                    else:
                        bundle["page_model"] = res
            except UnbrowserError as e:
                if "unknown method" in str(e):
                    bundle[name] = None
                else:
                    bundle[name + "_error"] = str(e)
            except Exception as e:
                bundle[name + "_error"] = str(e)
        # escalation on failures (Option A: expose related escalation)
        esc = _escalation_for_bundle(bundle)
        bundle["escalation"] = esc
        bundle["next_tools"] = esc.get("next_tools", []) if esc else _next_tools_from_bundle(bundle)
        return bundle

    # ---- infer ------------------------------------------------------------

    def run(self, task: str, **kw) -> Any:
        """Infer search vs navigate from a single string.

        - URL-looking (http/https or bare domain) -> navigate_auto
        - otherwise -> brave search

        kwargs pass through:
          navigate: goal, exec_scripts, discover_limit, cards_limit
          search: count
        """
        t = (task or "").strip()
        if not t:
            raise ValueError("empty task")
        if is_url(t):
            return self.navigate_auto(t, **{k: v for k, v in kw.items() if k in {"goal", "exec_scripts", "discover_limit", "cards_limit", "include_page_model"}})
        # search path
        count = int(kw.get("count", kw.get("limit", 10)))
        return self.search(t, engine=kw.get("engine", "brave"), count=count)

    # optional shorthand aliases for the two-entry framing
    def entry_search(self, query: str, **kw) -> list[dict]:
        return self.search(query, engine="brave", **kw)  # type: ignore[return-value]

    def entry_navigate(self, url: str, **kw) -> dict:
        return self.navigate_auto(url, **kw)


# ergonomic module-level helper
def run(task: str, **kw) -> Any:
    """One-shot infer: URL -> navigate+auto discover, else -> brave search."""
    with SmartClient() as ub:
        return ub.run(task, **kw)
