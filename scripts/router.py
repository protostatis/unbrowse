"""router.py — Auto-escalation router for unbrowser.

Wraps the binary as a subprocess. On `navigate`, inspects the response's
`challenge` field (the private-core-aligned shape: provider, confidence,
clearance_cookie, matched, ...). If a challenge fires, calls a pluggable
solver or local cookie service to obtain cookies, replays them via cookies_set,
retries.

The router is transparent: from the agent's perspective it's just a
`navigate(url)` that always returns a 200-shape result on success.

Solvers
-------
A solver is `fn(url: str) -> list[cookie_dict]`. Three reference paths are
provided:

- `cached_cookies_solver(path)` — load cookies from a JSON file (useful
  for demos and for "solve once in real Chrome via DevTools, cache
  forever" workflows).

- `unchained_cli_solver(profile_path)` — shell out to the existing
  unchainedsky-cli (`unchained launch ... cookies get ...`). Requires
  the CLI to be installed.

- `UNBROWSER_COOKIE_SERVICE_URL` / `cookie_service_url` — call a local-only
  service that drives unchained-cli/Chrome and returns cookies. This is the
  preferred transparent path for agents.

For production: write a custom solver that drives real Chrome (Playwright,
puppeteer, raw CDP WebSocket, or an approved internal service). The router
doesn't care how you get the cookies.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from dataclasses import dataclass
from typing import Callable
import urllib.error
import urllib.request

CookieList = list[dict]
Solver = Callable[[str], CookieList]


@dataclass
class RouterConfig:
    binary: str
    cwd: str | None = None
    chrome_solver: Solver | None = None
    cookie_service_url: str | None = None
    cookie_service_timeout: float = 90.0
    max_escalations: int = 1     # avoid infinite loops on permanently-blocked sites
    verbose: bool = True


class RouterError(Exception):
    pass


class Router:
    """Synchronous client for unbrowser with auto-escalation."""

    def __init__(self, config: RouterConfig):
        self.cfg = config
        self._cookie_service_url = (
            config.cookie_service_url
            or os.environ.get("UNBROWSER_COOKIE_SERVICE_URL")
            or ""
        ).rstrip("/")
        self._cookie_service_caps: dict | None = None
        self._proc = subprocess.Popen(
            [config.binary],
            cwd=config.cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self._next_id = 1

    # --- Low-level RPC plumbing --------------------------------------------

    def _send(self, method: str, params: dict | None = None) -> dict:
        req = {"id": self._next_id, "method": method}
        if params is not None:
            req["params"] = params
        self._next_id += 1
        line = json.dumps(req) + "\n"
        assert self._proc.stdin is not None
        self._proc.stdin.write(line)
        self._proc.stdin.flush()
        assert self._proc.stdout is not None
        resp_line = self._proc.stdout.readline()
        if not resp_line:
            raise RouterError(f"binary closed stdout while waiting for {method}")
        resp = json.loads(resp_line)
        if "error" in resp:
            raise RouterError(f"{method}: {resp['error']}")
        return resp.get("result")

    def _log(self, msg: str) -> None:
        if self.cfg.verbose:
            sys.stderr.write(f"[router] {msg}\n")
            sys.stderr.flush()

    # --- Public surface (passes through the binary's RPC methods) ----------

    def navigate(self, url: str) -> dict:
        """Navigate with auto-escalation on bot challenges."""
        result = self._send("navigate", {"url": url})
        attempts = 0
        while self._is_blocked(result) and attempts < self.cfg.max_escalations:
            attempts += 1
            challenge = result["challenge"]
            self._log(
                f"challenge: provider={challenge['provider']} "
                f"confidence={challenge['confidence']} "
                f"clearance_cookie={challenge.get('clearance_cookie')} "
                f"matched={challenge.get('matched')}"
            )
            if self.cfg.chrome_solver is None and not self._cookie_service_url:
                raise RouterError(
                    f"challenge from {challenge['provider']} but no chrome_solver "
                    f"or cookie_service_url configured. Set RouterConfig.chrome_solver "
                    f"or UNBROWSER_COOKIE_SERVICE_URL."
                )
            self._log(f"escalating to cookie solver (attempt {attempts}/{self.cfg.max_escalations})")
            cookies = self._solve_challenge(url, challenge)
            if not cookies:
                raise RouterError(
                    f"cookie solver returned no cookies for {url} - cannot retry"
                )
            self._log(f"solver returned {len(cookies)} cookies; replaying")
            self._send("cookies_set", {"cookies": list(cookies), "url": url})
            result = self._send("navigate", {"url": url})

        if self._is_blocked(result):
            raise RouterError(
                f"still blocked after {attempts} escalation(s): {result['challenge']}"
            )
        route = (result or {}).get("browser_route") or {}
        if route.get("needed"):
            self._log(
                f"browser_route: reason={route.get('reason')} "
                f"confidence={route.get('confidence')} "
                f"evidence={route.get('evidence')}"
            )
        limit = (result or {}).get("rate_limit") or {}
        if limit.get("limited"):
            self._log(
                f"rate_limit: status={limit.get('status')} "
                f"retry_after={limit.get('retry_after')} "
                f"reason={limit.get('reason')}"
            )
        return result

    def query(self, selector: str) -> list[dict]:
        return self._send("query", {"selector": selector})

    def text(self, selector: str = "body") -> str | None:
        return self._send("text", {"selector": selector})

    def click(self, ref: str) -> dict:
        return self._send("click", {"ref": ref})

    def type(self, ref: str, text: str) -> dict:
        return self._send("type", {"ref": ref, "text": text})

    def submit(self, ref: str) -> dict:
        return self._send("submit", {"ref": ref})

    def cookies_set(self, cookies: CookieList, url: str | None = None) -> dict:
        params = {"cookies": list(cookies)}
        if url is not None:
            params["url"] = url
        return self._send("cookies_set", params)

    def cookies_get(self) -> CookieList:
        return self._send("cookies_get")

    def cookies_clear(self) -> dict:
        return self._send("cookies_clear")

    def eval(self, code: str) -> object:
        return self._send("eval", {"code": code})

    def blockmap(self) -> dict:
        return self._send("blockmap")

    def close(self) -> None:
        try:
            self._send("close")
        except RouterError:
            pass
        try:
            self._proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self._proc.kill()

    def __enter__(self) -> "Router":
        return self

    def __exit__(self, *exc) -> None:
        self.close()

    # --- Helpers -----------------------------------------------------------

    @staticmethod
    def _is_blocked(navigate_result: dict) -> bool:
        ch = (navigate_result or {}).get("challenge")
        return bool(ch) and bool(ch.get("blocked"))

    def _solve_challenge(self, url: str, challenge: dict) -> CookieList:
        if self.cfg.chrome_solver is not None:
            return self.cfg.chrome_solver(url)
        return self._solve_via_cookie_service(url, challenge)

    def _solve_via_cookie_service(self, url: str, challenge: dict) -> CookieList:
        if not self._cookie_service_url:
            raise RouterError("cookie service URL is not configured")
        provider = str(challenge.get("provider") or "")
        caps = self._cookie_service_capabilities()
        providers = caps.get("providers") or []
        if providers and provider and provider not in providers:
            raise RouterError(
                f"cookie service does not advertise support for {provider}; "
                f"providers={providers}"
            )
        body = {
            "url": url,
            "provider": provider,
            "clearance_cookie": challenge.get("clearance_cookie"),
        }
        result = _post_json(
            f"{self._cookie_service_url}/solve",
            body,
            timeout=self.cfg.cookie_service_timeout,
        )
        if not result.get("ok"):
            raise RouterError(f"cookie service failed: {result.get('error') or result}")
        return [_normalize_cookie(c) for c in _cookie_list(result)]

    def _cookie_service_capabilities(self) -> dict:
        if self._cookie_service_caps is not None:
            return self._cookie_service_caps
        try:
            self._cookie_service_caps = _get_json(
                f"{self._cookie_service_url}/.well-known/unbrowser-cookie-solver",
                timeout=min(self.cfg.cookie_service_timeout, 10.0),
            )
        except RouterError as exc:
            self._log(f"cookie service capability check failed; trying solve anyway: {exc}")
            self._cookie_service_caps = {}
        return self._cookie_service_caps


# =============================================================================
# Reference solvers
# =============================================================================

def cached_cookies_solver(cookies_path: str) -> Solver:
    """Load cookies from a JSON file. Use for cached "solve-once-in-Chrome" flows.

    Accepts both unbrowser format ({name, value, domain, path,
    secure, http_only}) and CDP format ({httpOnly, ...}); auto-converts the
    latter to the former.
    """
    def solve(url: str) -> CookieList:
        with open(cookies_path) as f:
            raw = json.load(f)
        return [_normalize_cookie(c) for c in raw]
    return solve


def unchained_cli_solver(
    profile: str = "Profile 5",
    port: int = 9333,
    *,
    use_profile: bool = True,
    headless: bool = False,
    stealth: bool = True,
    kill_after: bool = True,
) -> Solver:
    """Shell out to the unchainedsky CLI to launch real Chrome and lift cookies.

    Requires `unchained` to be installed (`pip install unchainedsky-cli`).
    """
    def solve(url: str) -> CookieList:
        launch_cmd = ["unchained", "--port", str(port), "--json", "launch"]
        if use_profile:
            launch_cmd.append("--use-profile")
        if profile:
            launch_cmd.extend(["--profile", profile])
        if headless:
            launch_cmd.append("--headless")
        elif stealth:
            launch_cmd.append("--stealth")
        launch_cmd.append(url)
        owns_chrome = False
        try:
            launch = subprocess.run(
                launch_cmd,
                check=True,
                capture_output=True,
                text=True,
                timeout=60,
            )
            try:
                owns_chrome = json.loads(launch.stdout or "{}").get("already_running") is False
            except json.JSONDecodeError:
                owns_chrome = False
            export = subprocess.run(
                ["unchained", "--port", str(port), "--json", "cookies", "get",
                 "--urls", url],
                check=True,
                capture_output=True,
                text=True,
                timeout=30,
            )
            cookies = _cookie_list(json.loads(export.stdout))
        finally:
            if kill_after and owns_chrome:
                subprocess.run(
                    ["unchained", "--port", str(port), "kill"],
                    capture_output=True,
                    timeout=10,
                )
        return [_normalize_cookie(c) for c in cookies]
    return solve


def cookie_service_solver(service_url: str, timeout: float = 90.0) -> Solver:
    """Return a solver function backed by scripts/cookie_service.py."""
    base = service_url.rstrip("/")

    def solve(url: str) -> CookieList:
        result = _post_json(f"{base}/solve", {"url": url}, timeout=timeout)
        if not result.get("ok"):
            raise RouterError(f"cookie service failed: {result.get('error') or result}")
        return [_normalize_cookie(c) for c in _cookie_list(result)]

    return solve


def _normalize_cookie(c: dict) -> dict:
    """Convert a CDP-shaped cookie to unbrowser's shape (or pass through)."""
    return {
        "name": c["name"],
        "value": c["value"],
        "domain": c.get("domain", ""),
        "path": c.get("path", "/"),
        "secure": c.get("secure", False),
        "http_only": c.get("http_only", c.get("httpOnly", False)),
    }


def _cookie_list(payload: object) -> list[dict]:
    if isinstance(payload, list):
        return [c for c in payload if isinstance(c, dict)]
    if isinstance(payload, dict):
        raw = payload.get("cookies") or payload.get("result") or []
        if isinstance(raw, list):
            return [c for c in raw if isinstance(c, dict)]
    return []


def _get_json(url: str, timeout: float) -> dict:
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
    except (urllib.error.URLError, TimeoutError) as exc:
        raise RouterError(f"GET {url}: {exc}") from exc
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RouterError(f"GET {url}: invalid JSON") from exc


def _post_json(url: str, body: dict, timeout: float) -> dict:
    data = json.dumps(body).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", "replace")
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            raise RouterError(f"POST {url}: HTTP {exc.code}: {raw[:200]}") from exc
    except (urllib.error.URLError, TimeoutError) as exc:
        raise RouterError(f"POST {url}: {exc}") from exc
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RouterError(f"POST {url}: invalid JSON") from exc


# =============================================================================
# Demo CLI
# =============================================================================

def _demo() -> None:
    """Drive the router against an arg URL.

    Usage:
        python scripts/router.py <url> [--cookies <path>]

    Example (no cookies, clean site):
        python scripts/router.py https://news.ycombinator.com

    Example (with cached cookies for a protected site):
        python scripts/router.py https://www.zillow.com/homes/for_rent/ \
            --cookies /tmp/zillow_cookies.json
    """
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("url")
    parser.add_argument("--cookies", default=None,
                        help="Path to a cached cookies JSON file (CDP or ub format)")
    parser.add_argument("--cookie-service", default=None,
                        help="Local cookie service URL (or UNBROWSER_COOKIE_SERVICE_URL)")
    parser.add_argument("--binary", default=None,
                        help="Path to the unbrowser binary (default: cargo run --quiet)")
    args = parser.parse_args()

    binary = args.binary or os.path.expanduser("~/.cargo/bin/cargo")
    if "cargo" in binary:
        # cargo path — need to use it as the launcher and cd into the project.
        env_cmd = [binary, "run", "--quiet"]
        cwd = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
        # Spawn manually since RouterConfig only takes single binary.
        # Easiest: pre-build with cargo, then exec the binary directly.
        target = os.path.join(cwd, "target", "debug", "unbrowser")
        if not os.path.exists(target):
            print(f"[demo] building binary at {target} ...")
            subprocess.run([binary, "build", "--quiet"], cwd=cwd, check=True)
        binary = target
        cwd = None
    else:
        cwd = None

    solver = cached_cookies_solver(args.cookies) if args.cookies else None
    cfg = RouterConfig(
        binary=binary,
        cwd=cwd,
        chrome_solver=solver,
        cookie_service_url=args.cookie_service,
    )

    with Router(cfg) as r:
        result = r.navigate(args.url)
        bm = result.get("blockmap", {}) or {}
        print(f"\n=== navigate ===")
        print(f"  status     : {result['status']}")
        print(f"  url        : {result['url']}")
        print(f"  bytes      : {result['bytes']}")
        print(f"  title      : {bm.get('title')}")
        print(f"  challenge  : {result.get('challenge')}")
        print(f"  structure  : {len(bm.get('structure', []))} blocks, "
              f"{len(bm.get('headings', []))} headings, "
              f"{bm.get('interactives', {}).get('links', 0)} links")


if __name__ == "__main__":
    _demo()
