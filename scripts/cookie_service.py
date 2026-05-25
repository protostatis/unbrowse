#!/usr/bin/env python3
"""Local cookie solver service backed by unchained-cli.

The service is intentionally small and local-only by default. It does not try to
fabricate challenge tokens. It opens the target URL in Chrome through
`unchained`, waits for the browser to receive challenge/session cookies, exports
those cookies, and returns them to the caller for `unbrowser.cookies_set`.

Protocol:
  GET  /.well-known/unbrowser-cookie-solver  -> capabilities
  GET  /healthz                              -> liveness
  POST /solve {url, provider?, clearance_cookie?, wait_seconds?}
"""
from __future__ import annotations

import argparse
import atexit
import ipaddress
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import urlparse


CAPABILITY_PATH = "/.well-known/unbrowser-cookie-solver"
DEFAULT_PROVIDERS = [
    "perimeterx_block",
    "cloudflare_turnstile",
    "datadome",
    "aws_waf",
    "akamai_bmp",
    "imperva",
    "arkose_labs",
    "recaptcha",
    "press_hold",
    "yahoo_sad_panda",
    "interstitial",
    "generic_human_verification",
    "unknown_block",
]


@dataclass
class Config:
    host: str = "127.0.0.1"
    port: int = 8765
    unchained: str = "unchained"
    cdp_port: int = 9333
    profile: str = "unbrowser-cookie-service"
    use_profile: bool = False
    headless: bool = True
    stealth: bool = True
    keep_chrome: bool = True
    launch_timeout: float = 30.0
    wait_timeout: float = 5.0
    max_wait_seconds: float = 45.0
    request_deadline: float = 90.0
    poll_interval: float = 0.5
    max_queue: int = 4
    request_timeout: float = 15.0
    providers: list[str] = field(default_factory=lambda: list(DEFAULT_PROVIDERS))
    allow_hosts: list[str] = field(default_factory=list)
    block_private_network: bool = True
    verbose: bool = True


class SolveError(Exception):
    pass


class CookieSolver:
    def __init__(self, cfg: Config):
        self.cfg = cfg
        self._owns_chrome = False
        self._solve_lock = threading.Lock()
        self._solve_slots = threading.BoundedSemaphore(max(1, cfg.max_queue))

    def capabilities(self) -> dict[str, Any]:
        return {
            "name": "local-unchained-cookie-solver",
            "version": 1,
            "protocol_version": 1,
            "providers": self.cfg.providers,
            "cookie_export": True,
            "requires_user_browser": True,
            "local_only": self.cfg.host in ("127.0.0.1", "localhost", "::1"),
            "headless": self.cfg.headless,
            "stealth": self.cfg.stealth or self.cfg.headless,
            "cdp_port": self.cfg.cdp_port,
        }

    def readiness(self) -> dict[str, Any]:
        found = shutil.which(self.cfg.unchained) is not None
        callable_ok = False
        if found:
            try:
                out = subprocess.run(
                    [self.cfg.unchained, "--help"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                )
                callable_ok = out.returncode == 0
            except (OSError, subprocess.TimeoutExpired):
                callable_ok = False
        ok = found and callable_ok
        return {
            "ok": ok,
            "unchained": found,
            "unchained_callable": callable_ok,
            "providers": self.cfg.providers,
            "headless": self.cfg.headless,
            "stealth": self.cfg.stealth or self.cfg.headless,
        }

    def solve(
        self,
        url: str,
        provider: str | None = None,
        clearance_cookie: str | None = None,
        wait_seconds: float | None = None,
    ) -> dict[str, Any]:
        if not self._solve_slots.acquire(blocking=False):
            raise SolveError("solve queue is full; retry later")
        try:
            with self._solve_lock:
                return self._solve_locked(url, provider, clearance_cookie, wait_seconds)
        finally:
            self._solve_slots.release()

    def _solve_locked(
        self,
        url: str,
        provider: str | None,
        clearance_cookie: str | None,
        wait_seconds: float | None,
    ) -> dict[str, Any]:
        parsed = self._validate_url(url)
        if provider and provider not in self.cfg.providers:
            raise SolveError(f"unsupported provider: {provider}")

        started = time.time()
        requested_wait = float(wait_seconds or self.cfg.max_wait_seconds)
        deadline = started + min(requested_wait, self.cfg.request_deadline)
        cookies: list[dict[str, Any]] = []
        success = False
        try:
            self._launch(url, deadline)
            while time.time() < deadline:
                self._wait(deadline)
                cookies = self._get_cookies(url, deadline)
                if self._has_cookie(cookies, clearance_cookie):
                    break
                time.sleep(min(self.cfg.poll_interval, max(0.0, deadline - time.time())))

            normalized = [
                c for c in (_normalize_cookie(c, parsed.hostname or "") for c in cookies) if c
            ]
            cookie_names = sorted({c["name"] for c in normalized})
            if clearance_cookie and clearance_cookie not in cookie_names:
                raise SolveError(
                    f"clearance cookie {clearance_cookie!r} was not observed; "
                    f"observed={cookie_names}"
                )
            if not normalized:
                raise SolveError("no cookies exported from Chrome")

            success = True
            self._log(
                f"solved provider={provider or 'unknown'} host={parsed.netloc} "
                f"cookies={cookie_names} elapsed_ms={int((time.time() - started) * 1000)}"
            )
            return {
                "ok": True,
                "provider": provider,
                "url": url,
                "cookies": normalized,
                "cookie_names": cookie_names,
                "elapsed_ms": int((time.time() - started) * 1000),
                "headless": self.cfg.headless,
                "stealth": self.cfg.stealth or self.cfg.headless,
            }
        finally:
            if self._owns_chrome and (not self.cfg.keep_chrome or not success):
                self._kill_chrome()

    def shutdown(self) -> None:
        if self._owns_chrome:
            self._kill_chrome()

    def _validate_url(self, url: str):
        parsed = urlparse(url)
        if parsed.scheme not in ("http", "https") or not parsed.netloc:
            raise SolveError("url must be an absolute http(s) URL")
        host = parsed.hostname or ""
        if self.cfg.allow_hosts:
            if not _host_allowed(host, self.cfg.allow_hosts):
                raise SolveError(f"host is not allowlisted: {host}")
        elif self.cfg.block_private_network and _is_private_or_reserved_host(host):
            raise SolveError(
                f"host {host!r} is not allowed by default; pass --allow-host to opt in"
            )
        return parsed

    def _launch(self, url: str, deadline: float) -> None:
        timeout = _bounded_timeout(deadline, self.cfg.launch_timeout)
        cmd = [self.cfg.unchained, "--port", str(self.cfg.cdp_port), "--json", "launch"]
        if self.cfg.use_profile:
            cmd.append("--use-profile")
        if self.cfg.profile:
            cmd.extend(["--profile", self.cfg.profile])
        if self.cfg.headless:
            cmd.append("--headless")
        elif self.cfg.stealth:
            cmd.append("--stealth")
        cmd.extend(["--timeout", str(max(1, int(timeout))), url])
        out = self._run(cmd, timeout=timeout)
        try:
            payload = json.loads(out.stdout or "{}")
        except json.JSONDecodeError:
            payload = {}
        if payload.get("already_running") is False:
            self._owns_chrome = True
        self._log(
            f"launch headless={self.cfg.headless} profile={self.cfg.profile} "
            f"already_running={payload.get('already_running')}"
        )

    def _wait(self, deadline: float) -> None:
        timeout = _bounded_timeout(deadline, self.cfg.wait_timeout)
        cmd = [
            self.cfg.unchained,
            "--port",
            str(self.cfg.cdp_port),
            "wait",
            "--strategy",
            "both",
            "--timeout",
            str(max(1, int(timeout))),
        ]
        try:
            subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as exc:
            raise SolveError("unchained wait timed out") from exc

    def _get_cookies(self, url: str, deadline: float) -> list[dict[str, Any]]:
        timeout = _bounded_timeout(deadline, 15.0)
        cmd = [
            self.cfg.unchained,
            "--port",
            str(self.cfg.cdp_port),
            "--json",
            "cookies",
            "get",
            "--urls",
            url,
        ]
        out = self._run(cmd, timeout=timeout)
        try:
            payload = json.loads(out.stdout or "[]")
        except json.JSONDecodeError as exc:
            raise SolveError("unchained cookies output was not JSON") from exc
        if isinstance(payload, list):
            return payload
        if isinstance(payload, dict):
            return payload.get("cookies") or payload.get("result") or []
        return []

    def _kill_chrome(self) -> None:
        subprocess.run(
            [self.cfg.unchained, "--port", str(self.cfg.cdp_port), "kill"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        self._owns_chrome = False

    def _run(self, cmd: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
        label = _command_label(cmd)
        try:
            out = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as exc:
            raise SolveError(f"{label} timed out") from exc
        if out.returncode != 0:
            self._log(f"{label} failed with exit code {out.returncode}")
            raise SolveError(f"{label} failed with exit code {out.returncode}")
        return out

    @staticmethod
    def _has_cookie(cookies: list[dict[str, Any]], name: str | None) -> bool:
        if not name:
            return bool(cookies)
        return any(c.get("name") == name for c in cookies)

    def _log(self, msg: str) -> None:
        if self.cfg.verbose:
            sys.stderr.write(f"[cookie-service] {msg}\n")
            sys.stderr.flush()


def make_handler(solver: CookieSolver):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            super().setup()
            self.request.settimeout(solver.cfg.request_timeout)

        def do_GET(self):
            if self.path == CAPABILITY_PATH:
                self._json(200, solver.capabilities())
                return
            if self.path == "/healthz":
                self._json(200, {"ok": True})
                return
            if self.path == "/readyz":
                self._json(200, solver.readiness())
                return
            self._json(404, {"ok": False, "error": "not found"})

        def do_POST(self):
            if self.path != "/solve":
                self._json(404, {"ok": False, "error": "not found"})
                return
            try:
                body = self._read_json()
                result = solver.solve(
                    url=str(body.get("url") or ""),
                    provider=body.get("provider"),
                    clearance_cookie=body.get("clearance_cookie"),
                    wait_seconds=body.get("wait_seconds"),
                )
                self._json(200, result)
            except SolveError as exc:
                self._json(502, {"ok": False, "error": str(exc)})
            except Exception as exc:
                self._json(500, {"ok": False, "error": f"internal error: {exc}"})

        def _read_json(self) -> dict[str, Any]:
            length = self.headers.get("content-length")
            if length is None:
                raise SolveError("content-length header is required")
            try:
                n = int(length or "0")
            except ValueError as exc:
                raise SolveError("content-length must be an integer") from exc
            if n > 64 * 1024:
                raise SolveError("request body too large")
            raw = self.rfile.read(n).decode("utf-8")
            if not raw:
                return {}
            try:
                payload = json.loads(raw)
            except json.JSONDecodeError as exc:
                raise SolveError("request body must be JSON") from exc
            if not isinstance(payload, dict):
                raise SolveError("request body must be a JSON object")
            return payload

        def _json(self, status: int, payload: dict[str, Any]) -> None:
            raw = json.dumps(payload, separators=(",", ":")).encode("utf-8")
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def log_message(self, fmt, *args):
            if solver.cfg.verbose:
                sys.stderr.write("[cookie-service] http " + fmt % args + "\n")

    return Handler


def _normalize_cookie(c: dict[str, Any], fallback_host: str) -> dict[str, Any] | None:
    name = c.get("name")
    if not isinstance(name, str) or not name:
        return None
    return {
        "name": name,
        "value": c.get("value", ""),
        "domain": c.get("domain") or fallback_host,
        "path": c.get("path") or "/",
        "secure": bool(c.get("secure", False)),
        "http_only": bool(c.get("http_only", c.get("httpOnly", False))),
    }


def _host_allowed(host: str, allow_hosts: list[str]) -> bool:
    host = host.lower().strip(".")
    for allowed in allow_hosts:
        a = allowed.lower().strip(".")
        if host == a or host.endswith("." + a):
            return True
    return False


def _validate_allow_hosts(allow_hosts: list[str]) -> list[str]:
    out: list[str] = []
    for raw in allow_hosts:
        host = raw.lower().strip().strip(".")
        if not host:
            raise SolveError("--allow-host must not be empty")
        if host == "localhost":
            out.append(host)
            continue
        try:
            ipaddress.ip_address(host)
            out.append(host)
            continue
        except ValueError:
            pass
        if "." not in host:
            raise SolveError(
                f"--allow-host {raw!r} is too broad; use a registrable domain like example.com"
            )
        out.append(host)
    return out


def _is_private_or_reserved_host(host: str) -> bool:
    h = host.lower().strip(".")
    if not h:
        return True
    if h == "localhost" or h.endswith(".localhost"):
        return True
    if "." not in h:
        return True
    if h.endswith((".local", ".localdomain", ".internal", ".svc", ".cluster.local")):
        return True
    try:
        ip = ipaddress.ip_address(h)
        return not ip.is_global
    except ValueError:
        pass
    try:
        infos = socket.getaddrinfo(h, None, type=socket.SOCK_STREAM)
    except OSError:
        return False
    for info in infos:
        try:
            ip = ipaddress.ip_address(info[4][0])
        except (IndexError, ValueError):
            continue
        if not ip.is_global:
            return True
    return False


def _bounded_timeout(deadline: float, preferred: float) -> float:
    remaining = deadline - time.time()
    if remaining <= 0:
        raise SolveError("solve request deadline exceeded")
    return min(preferred, remaining)


def _command_label(cmd: list[str]) -> str:
    if not cmd:
        return "command"
    base = os.path.basename(cmd[0]) or "command"
    for subcommand in ("launch", "wait", "cookies", "kill"):
        if subcommand in cmd:
            return f"{base} {subcommand}"
    return base


def parse_args() -> Config:
    p = argparse.ArgumentParser(description="Local unchained-backed cookie solver service")
    p.add_argument("--host", default=os.environ.get("UNBROWSER_COOKIE_SERVICE_HOST", "127.0.0.1"))
    p.add_argument("--port", type=int, default=int(os.environ.get("UNBROWSER_COOKIE_SERVICE_PORT", "8765")))
    p.add_argument("--unchained", default=os.environ.get("UNBROWSER_UNCHAINED", "unchained"))
    p.add_argument("--cdp-port", type=int, default=int(os.environ.get("UNBROWSER_COOKIE_SERVICE_CDP_PORT", "9333")))
    p.add_argument("--profile", default=os.environ.get("UNBROWSER_COOKIE_SERVICE_PROFILE", "unbrowser-cookie-service"))
    p.add_argument("--use-profile", action="store_true", help="Use an existing Chrome profile by name")
    p.add_argument("--headless", action=argparse.BooleanOptionalAction, default=_env_bool("UNBROWSER_COOKIE_SERVICE_HEADLESS", True))
    p.add_argument("--stealth", action=argparse.BooleanOptionalAction, default=_env_bool("UNBROWSER_COOKIE_SERVICE_STEALTH", True))
    p.add_argument("--keep-chrome", action=argparse.BooleanOptionalAction, default=True)
    p.add_argument("--launch-timeout", type=float, default=30.0)
    p.add_argument("--wait-timeout", type=float, default=5.0)
    p.add_argument("--max-wait-seconds", type=float, default=45.0)
    p.add_argument("--request-deadline", type=float, default=90.0, help="Total solve request deadline in seconds")
    p.add_argument("--poll-interval", type=float, default=0.5)
    p.add_argument("--max-queue", type=int, default=4, help="Maximum queued/in-flight solve requests")
    p.add_argument("--request-timeout", type=float, default=15.0, help="Per-connection socket timeout in seconds")
    p.add_argument("--providers", default=",".join(DEFAULT_PROVIDERS), help="Comma-separated supported challenge providers")
    p.add_argument("--allow-host", action="append", default=[], help="Restrict solves to this host or suffix; repeatable")
    p.add_argument("--allow-private-network", action="store_true", help="Allow private/reserved hosts when no --allow-host is configured")
    p.add_argument("--quiet", action="store_true")
    a = p.parse_args()
    return Config(
        host=a.host,
        port=a.port,
        unchained=a.unchained,
        cdp_port=a.cdp_port,
        profile=a.profile,
        use_profile=a.use_profile,
        headless=a.headless,
        stealth=a.stealth,
        keep_chrome=a.keep_chrome,
        launch_timeout=a.launch_timeout,
        wait_timeout=a.wait_timeout,
        max_wait_seconds=a.max_wait_seconds,
        request_deadline=a.request_deadline,
        poll_interval=a.poll_interval,
        providers=[x.strip() for x in a.providers.split(",") if x.strip()],
        allow_hosts=_validate_allow_hosts(a.allow_host),
        block_private_network=not a.allow_private_network,
        verbose=not a.quiet,
        max_queue=a.max_queue,
        request_timeout=a.request_timeout,
    )


def _env_bool(name: str, default: bool) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    return value.strip().lower() not in ("0", "false", "no", "off")


def main() -> int:
    cfg = parse_args()
    if shutil.which(cfg.unchained) is None:
        sys.stderr.write(f"[cookie-service] unchained binary not found: {cfg.unchained}\n")
        return 2
    solver = CookieSolver(cfg)
    atexit.register(solver.shutdown)
    httpd = ThreadingHTTPServer((cfg.host, cfg.port), make_handler(solver))
    httpd.timeout = cfg.request_timeout

    def _shutdown(*_):
        solver.shutdown()
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, _shutdown)
    signal.signal(signal.SIGINT, _shutdown)
    if cfg.verbose:
        sys.stderr.write(
            f"[cookie-service] listening on http://{cfg.host}:{cfg.port} "
            f"headless={cfg.headless} profile={cfg.profile}\n"
        )
        sys.stderr.flush()
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        solver.shutdown()
        httpd.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
