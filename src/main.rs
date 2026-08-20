use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader as StdBufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use rquickjs::FromJs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

mod bytecode_cache;
mod challenge;
mod network_store;
mod policy;
mod prefit;
mod profile;
use profile::Profile;

const DOM_JS: &str = include_str!("js/dom.js");
const SHIMS_JS: &str = include_str!("js/shims.js");
const BLOCKMAP_JS: &str = include_str!("js/blockmap.js");
const INTERACT_JS: &str = include_str!("js/interact.js");
const EXTRACT_JS: &str = include_str!("js/extract.js");
const PAGE_MODEL_JS: &str = include_str!("js/page_model.js");

static EVENTS_ENABLED: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShimMode {
    Stable,
    Enhanced,
}

impl ShimMode {
    fn as_str(self) -> &'static str {
        match self {
            ShimMode::Stable => "stable",
            ShimMode::Enhanced => "enhanced",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "stable" | "default" => Ok(ShimMode::Stable),
            "enhanced" | "experimental" => Ok(ShimMode::Enhanced),
            other => Err(anyhow!(
                "invalid shim mode '{other}'. Expected stable or enhanced"
            )),
        }
    }
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct Response {
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

fn eval_code_param(params: &Value) -> std::result::Result<&str, String> {
    let mut found: Vec<(&'static str, &str)> = Vec::new();
    for key in ["code", "script", "expression"] {
        if let Some(v) = params.get(key) {
            let s = v
                .as_str()
                .ok_or_else(|| format!("eval.{key} must be a string"))?;
            found.push((key, s));
        }
    }
    match found.len() {
        0 => Err("missing 'code' param (aliases accepted: 'script', 'expression')".to_string()),
        1 => Ok(found[0].1),
        _ => Err(format!(
            "ambiguous eval params: provide only one of {}",
            found
                .iter()
                .map(|(k, _)| format!("'{k}'"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[derive(Default)]
struct CookieJar {
    inner: RwLock<cookie_store::CookieStore>,
}

impl wreq::cookie::CookieStore for CookieJar {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &http::HeaderValue>, uri: &http::Uri) {
        let Some(url) = url_from_uri(uri) else {
            return;
        };
        let parsed: Vec<cookie::Cookie<'static>> = headers
            .filter_map(|h| h.to_str().ok())
            .filter_map(|s| cookie::Cookie::parse(s.to_string()).ok())
            .collect();
        if let Ok(mut store) = self.inner.write() {
            store.store_response_cookies(parsed.into_iter(), &url);
        }
    }

    fn cookies(&self, uri: &http::Uri) -> wreq::cookie::Cookies {
        let Some(url) = url_from_uri(uri) else {
            return wreq::cookie::Cookies::Empty;
        };
        let Ok(store) = self.inner.read() else {
            return wreq::cookie::Cookies::Empty;
        };
        let s: String = store
            .get_request_values(&url)
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        if s.is_empty() {
            wreq::cookie::Cookies::Empty
        } else {
            http::HeaderValue::from_str(&s)
                .map(wreq::cookie::Cookies::Compressed)
                .unwrap_or(wreq::cookie::Cookies::Empty)
        }
    }
}

fn url_from_uri(uri: &http::Uri) -> Option<url::Url> {
    url::Url::parse(&uri.to_string()).ok()
}

impl CookieJar {
    fn export(&self) -> Vec<Value> {
        match self.inner.read() {
            Ok(s) => s
                .iter_unexpired()
                .map(|c| {
                    json!({
                        "name": c.name(),
                        "value": c.value(),
                        "domain": c.domain(),
                        "path": c.path(),
                        "secure": c.secure().unwrap_or(false),
                        "http_only": c.http_only().unwrap_or(false),
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn import(&self, items: &[Value], default_url: Option<&str>) -> Result<usize> {
        let mut store = self
            .inner
            .write()
            .map_err(|_| anyhow!("cookie jar lock poisoned"))?;
        let mut added = 0;
        for item in items {
            let (cookie_str, url_str) = build_cookie(item, default_url)?;
            let url = url::Url::parse(&url_str).map_err(|e| anyhow!("parse url: {e}"))?;
            if let Ok(c) = cookie::Cookie::parse(cookie_str) {
                store.store_response_cookies(std::iter::once(c.into_owned()), &url);
                added += 1;
            }
        }
        Ok(added)
    }

    fn clear(&self) {
        if let Ok(mut s) = self.inner.write() {
            s.clear();
        }
    }
}

// Accept either a Set-Cookie string or a {name, value, domain, path?, secure?, http_only?, url?} object.
fn build_cookie(item: &Value, default_url: Option<&str>) -> Result<(String, String)> {
    if let Some(s) = item.as_str() {
        // Bare Set-Cookie string — derive url from default_url
        let url = default_url
            .map(String::from)
            .ok_or_else(|| anyhow!("string-form cookie requires 'url' param"))?;
        return Ok((s.to_string(), url));
    }
    let obj = item
        .as_object()
        .ok_or_else(|| anyhow!("cookie must be string or object"))?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("cookie missing 'name'"))?;
    let value = obj.get("value").and_then(|v| v.as_str()).unwrap_or("");
    let domain = obj.get("domain").and_then(|v| v.as_str());
    let path = obj.get("path").and_then(|v| v.as_str()).unwrap_or("/");
    let secure = obj.get("secure").and_then(|v| v.as_bool()).unwrap_or(false);
    let http_only = obj
        .get("http_only")
        .or_else(|| obj.get("httpOnly"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut s = format!("{name}={value}; Path={path}");
    if let Some(d) = domain {
        s.push_str(&format!("; Domain={d}"));
    }
    if secure {
        s.push_str("; Secure");
    }
    if http_only {
        s.push_str("; HttpOnly");
    }

    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            domain.map(|d| {
                let host = d.trim_start_matches('.');
                let scheme = if secure { "https" } else { "http" };
                format!("{scheme}://{host}/")
            })
        })
        .or_else(|| default_url.map(String::from))
        .ok_or_else(|| anyhow!("cookie {name} has no 'url' or 'domain'"))?;

    Ok((s, url))
}

// =============================================================================
// Fetch worker — lets page-script `fetch()` calls go through the same
// wreq::Client we use for navigate (so cookies + Chrome profile TLS fingerprint
// stay coherent). One dedicated thread, one current_thread tokio runtime,
// requests serialized through an mpsc channel. Responses queue into a shared
// Mutex<Vec<...>> that JS drains via __host_drain_fetches() during settle().
// =============================================================================

struct FetchRequest {
    id: u64,
    method: String,
    url: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Serialize)]
struct FetchResponse {
    id: u64,
    status: u16,
    headers: HashMap<String, String>,
    body: String,
    error: Option<String>,
}

struct FetchQueue {
    sender: mpsc::Sender<FetchRequest>,
    results: Arc<Mutex<Vec<FetchResponse>>>,
    network_store: Arc<Mutex<network_store::NetworkStore>>,
    /// nav_id of the currently-running navigate call, set by navigate_with
    /// at start (after seed_dom) and updated each navigation. The worker
    /// thread reads this when capturing fetches so each capture is bound
    /// to whichever navigation was in flight when it resolved. Prevents
    /// page A's captures from leaking into page B's summary. (PR #7
    /// review medium.)
    current_nav_id: Arc<Mutex<Option<String>>>,
}

fn spawn_fetch_worker(http: wreq::Client) -> FetchQueue {
    let (tx, rx) = mpsc::channel::<FetchRequest>();
    let results: Arc<Mutex<Vec<FetchResponse>>> = Arc::new(Mutex::new(Vec::new()));
    let results_for_thread = results.clone();
    let network_store: Arc<Mutex<network_store::NetworkStore>> =
        Arc::new(Mutex::new(network_store::NetworkStore::default()));
    let store_for_thread = network_store.clone();
    let current_nav_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let nav_id_for_thread = current_nav_id.clone();

    std::thread::Builder::new()
        .name("unbrowser-fetch".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            runtime.block_on(async move {
                while let Ok(req) = rx.recv() {
                    // Snapshot before move — FetchResponse doesn't carry url/method.
                    let url = req.url.clone();
                    let method = req.method.clone();
                    let resp = run_fetch(http.clone(), req).await;
                    // Network capture: opportunistic content-bearing
                    // response capture for the network_stores RPC. See
                    // src/network_store.rs. Skipped for blocked URLs
                    // because the policy hook in __host_fetch_send
                    // short-circuits BEFORE this worker ever sees them
                    // (synthetic 204 enqueued directly to results), so
                    // tracker bodies are never even fetched.
                    //
                    // nav_id binding: read whichever navigate is currently
                    // in flight (or the most recent one that ran) — this
                    // gives each capture a stable navigation_id for
                    // per-page filtering.
                    if resp.error.is_none() && !resp.body.is_empty() {
                        let nav_id = nav_id_for_thread.lock().ok().and_then(|g| g.clone());
                        if let Ok(mut s) = store_for_thread.lock() {
                            s.maybe_capture(
                                &url,
                                &method,
                                resp.status,
                                &resp.headers,
                                &resp.body,
                                nav_id.as_deref(),
                            );
                        }
                    }
                    if let Ok(mut g) = results_for_thread.lock() {
                        g.push(resp);
                    }
                }
            });
        })
        .ok();

    FetchQueue {
        sender: tx,
        results,
        network_store,
        current_nav_id,
    }
}

async fn run_fetch(http: wreq::Client, req: FetchRequest) -> FetchResponse {
    let method = match req.method.to_uppercase().as_str() {
        "GET" => http::Method::GET,
        "POST" => http::Method::POST,
        "PUT" => http::Method::PUT,
        "DELETE" => http::Method::DELETE,
        "HEAD" => http::Method::HEAD,
        "PATCH" => http::Method::PATCH,
        "OPTIONS" => http::Method::OPTIONS,
        _ => http::Method::GET,
    };
    let mut builder = http.request(method, &req.url);
    for (k, v) in &req.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    if !req.body.is_empty() {
        builder = builder.body(req.body.clone());
    }
    match builder.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let mut hmap = HashMap::new();
            for (n, v) in resp.headers() {
                hmap.insert(
                    n.as_str().to_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                );
            }
            let body = resp.text().await.unwrap_or_default();
            FetchResponse {
                id: req.id,
                status,
                headers: hmap,
                body,
                error: None,
            }
        }
        Err(e) => FetchResponse {
            id: req.id,
            status: 0,
            headers: HashMap::new(),
            body: String::new(),
            error: Some(e.to_string()),
        },
    }
}

struct Session {
    // Holds the QuickJS runtime alive for the Context's lifetime AND
    // exposes execute_pending_job() / is_job_pending() so settle() can
    // drain the microtask queue between timer firings.
    js_rt: rquickjs::Runtime,
    js_ctx: rquickjs::Context,
    http: wreq::Client,
    jar: Arc<CookieJar>,
    // Fetch worker queue — held to keep the worker thread alive and to
    // expose results to settle() via __pollFetches() driven by the JS layer.
    _fetch: Arc<FetchQueue>,
    // Global eval-time deadline (unix-ms). 0 = no deadline. Read by the
    // QuickJS interrupt handler installed once at Session::new and bumped
    // by the per-RPC dispatcher and the navigate script phase. Without
    // this every exec_scripts=true call on a hostile SPA could leave a
    // CPU-pegged process behind.
    eval_deadline_ms: Arc<AtomicU64>,
    last_url: Option<String>,
    last_challenge: Option<Value>,
    last_rate_limit: Option<Value>,
    last_browser_route: Option<Value>,
    // Raw HTML body of the most recent navigate, shared with the JS layer
    // via the __host_raw_body() host function. Arc'd so the function closure
    // can read it after the DOM has been mutated by hydration scripts —
    // e.g. Next.js App Router removes inline `__next_f.push` script
    // elements after rehydrating, but the raw body still has them, which
    // is what `strategyRscPayload` in extract.js needs.
    last_body: Arc<Mutex<Option<String>>>,
    // True when --policy=blocklist (or UNBROWSER_POLICY=blocklist) is set.
    // Read by the external-script fetch loop in navigate_with and by the
    // __host_fetch_send hook — see src/policy.rs.
    policy_block: bool,
    // Monotonic counter for navigation_id. Each navigate() call increments
    // and emits a navigation_started event with the new id. Subsequent
    // events from that navigation (script_decision, policy_trace) carry
    // the same id so a driver can join outcomes against decisions.
    // See docs/probabilistic-policy.md §4.5 (outcome protocol).
    //
    // Ordering: Relaxed is correct today (single Session, single QuickJS
    // runtime, current-thread tokio runtime — `navigate_with` cannot run
    // concurrently with itself). If concurrency is ever introduced, the
    // counter would still produce unique ids, but the *visibility* of
    // associated emissions would need at least AcqRel.
    nav_counter: AtomicU64,
    // Set of nav_ids that this Session has issued via next_nav_id() and
    // that have at least reached the navigation_started emit point. Read
    // by report_outcome to reject outcomes for unknown ids. Bounded by
    // number of navigates per process — small in practice; if it ever
    // matters we can switch to a ring buffer.
    nav_ids_issued: Mutex<HashSet<String>>,
    /// Bytecode cache config for eval_with_cache. shim_hash incorporates
    /// the JS environment (shims.js + dom.js) so cached bytecode whose
    /// captured globals diverge from the current build is rejected
    /// automatically. Disabled when UNBROWSER_NO_BYTECODE_CACHE=1.
    bytecode_cache_root: std::path::PathBuf,
    shim_hash: String,
    bytecode_cache_disabled: bool,
    /// Loaded at Session::new from the embedded JSON bundle. Each navigate
    /// looks up the target domain to apply per-(domain, framework)
    /// decision parameters trained centrally — extends the global
    /// Tier-1 blocklist with per-site additions, surfaces settle hints
    /// via the prefit_applied event. None on parse failure (rare; would
    /// be a build-time bug). See src/prefit.rs (R1 from white paper §6).
    prefit: Option<prefit::PrefitBundle>,
    /// JS runtime fidelity profile. `stable` preserves the release behavior;
    /// `enhanced` enables opt-in layout/media/storage shims for A/B tests.
    shim_mode: ShimMode,
}

struct DiscoverOptions<'a> {
    url: Option<&'a str>,
    goal: Option<&'a str>,
    exec_scripts: bool,
    limit: u32,
    same_origin: bool,
    include_network: bool,
    debug: bool,
}

const DISCOVERY_DOM_CONFIDENCE: f64 = 0.72;
const DISCOVERY_INFERRED_CONFIDENCE: f64 = 0.68;
const DISCOVERY_NETWORK_OBJECT_CONFIDENCE: f64 = 0.58;
const DISCOVERY_CAPTURE_API_CONFIDENCE: f64 = 0.62;
const DISCOVERY_NETWORK_ROUTE_CONFIDENCE: f64 = 0.54;
const DISCOVERY_RAW_NETWORK_API_SCORE: f64 = 0.48;
const DISCOVERY_RAW_NETWORK_ROUTE_SCORE: f64 = 0.52;
const DISCOVERY_RAW_NETWORK_API_CONFIDENCE: f64 = 0.56;
const DISCOVERY_RAW_NETWORK_ROUTE_CONFIDENCE: f64 = 0.58;
const RAW_NETWORK_ROUTE_MAX_DEPTH: usize = 32;

fn parse_discover_options(params: &Value) -> Result<DiscoverOptions<'_>> {
    let url = match params.get("url") {
        Some(Value::String(s)) => Some(s.as_str()),
        Some(Value::Null) | None => None,
        Some(_) => return Err(anyhow!("discover.url must be a string")),
    };
    validate_discover_url(url)?;

    let limit = match params.get("limit") {
        Some(v) => v
            .as_u64()
            .ok_or_else(|| anyhow!("discover.limit must be an integer"))?,
        None => 50,
    };
    if !(1..=200).contains(&limit) {
        return Err(anyhow!("discover.limit must be between 1 and 200"));
    }

    Ok(DiscoverOptions {
        url,
        goal: params.get("goal").and_then(|v| v.as_str()),
        exec_scripts: params
            .get("exec_scripts")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        limit: limit as u32,
        same_origin: params
            .get("same_origin")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        include_network: params
            .get("include_network")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        debug: params
            .get("debug")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

fn validate_discover_url(raw: Option<&str>) -> Result<()> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let parsed = url::Url::parse(raw).map_err(|e| anyhow!("invalid discover.url: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        scheme => Err(anyhow!(
            "discover.url must use http or https, got '{scheme}'"
        )),
    }
}

impl Session {
    fn new(profile: &Profile, policy_block: bool, shim_mode: ShimMode) -> Result<Self> {
        let js_rt = rquickjs::Runtime::new().context("rquickjs Runtime::new")?;
        let js_ctx = rquickjs::Context::full(&js_rt).context("rquickjs Context::full")?;
        // Allocated up here so the __host_raw_body() host function below can
        // clone the Arc into its closure before Session is constructed.
        let last_body_arc: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // Install the always-on watchdog. Every nested QuickJS eval (including
        // ones inside settle's __pumpTimers callbacks and __pollFetches
        // resolvers) consults this atomic. Default 0 = no bound; the dispatcher
        // bumps it before each RPC call.
        let eval_deadline_ms: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
        let dl_for_handler = eval_deadline_ms.clone();
        js_rt.set_interrupt_handler(Some(Box::new(move || {
            let deadline = dl_for_handler.load(Ordering::Relaxed);
            if deadline == 0 {
                return false;
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            now >= deadline
        })));
        let jar = Arc::new(CookieJar::default());
        let http = wreq::Client::builder()
            .emulation(profile.emulation)
            .cookie_provider(jar.clone())
            // .emulation(...) appears to clobber the default redirect policy.
            // Explicit follow-up-to-10 matches Chrome's behavior on http://github.com,
            // httpbin.org/redirect/N, and the Yahoo "sad panda" 301 chain.
            .redirect(wreq::redirect::Policy::limited(10))
            .build()
            .context("wreq client build")?;
        // Spawn the fetch worker thread (uses the same wreq::Client so cookies
        // + TLS fingerprint stay coherent with navigate).
        let fetch = Arc::new(spawn_fetch_worker(http.clone()));

        // Install JS layers in order:
        //   1. dom.js     — document, Element, querySelector, __seedDOM, etc.
        //   2. shims.js   — passive browser globals (window, navigator, location,
        //                   storage, etc.) — coherent with our Chrome profile TLS FP
        //   3. blockmap.js — __blockmap() page-summary walker
        //   4. interact.js — __click, __type, __byRef, __formData
        //   5. extract.js  — text/list/card helpers
        //   6. page_model.js — semantic page object model
        // Then register host bindings the JS layer references at call time
        // (__host_fetch_send, __host_drain_fetches).
        js_ctx.with(|ctx| -> Result<()> {
            ctx.eval::<(), _>(DOM_JS)
                .map_err(|e| anyhow!("eval dom.js: {e}"))?;
            ctx.globals()
                .set("__UNBROWSER_SHIM_MODE", shim_mode.as_str())
                .map_err(|e| anyhow!("set __UNBROWSER_SHIM_MODE: {e}"))?;
            ctx.eval::<(), _>(SHIMS_JS)
                .map_err(|e| anyhow!("eval shims.js: {e}"))?;
            ctx.eval::<(), _>(BLOCKMAP_JS)
                .map_err(|e| anyhow!("eval blockmap.js: {e}"))?;
            ctx.eval::<(), _>(INTERACT_JS)
                .map_err(|e| anyhow!("eval interact.js: {e}"))?;
            ctx.eval::<(), _>(EXTRACT_JS)
                .map_err(|e| anyhow!("eval extract.js: {e}"))?;
            ctx.eval::<(), _>(PAGE_MODEL_JS)
                .map_err(|e| anyhow!("eval page_model.js: {e}"))?;
            // Apply profile-driven navigator.* patches AFTER shims.js
            // installs the base navigator object. Page scripts that read
            // navigator.userAgent / .platform / .languages now see the
            // profile values, coherent with the TLS+H2 emulation above.
            ctx.eval::<(), _>(profile.js_init())
                .map_err(|e| anyhow!("eval profile.js_init: {e}"))?;

            // __host_fetch_send(id, method, url, headers_json, body) — fire-and-forget.
            // headers_json is a JSON-encoded string from JS to avoid converting
            // an rquickjs Object inside the host closure.
            //
            // Policy hook: when policy_block is on, decide(url) gates the send.
            // Blocked URLs short-circuit with a synthetic 204 pushed straight
            // into the results queue — JS sees the same Promise resolution
            // shape it would for a network-completed request, just with empty
            // body and no actual HTTP made. See src/policy.rs.
            let sender = fetch.sender.clone();
            let results_for_block = fetch.results.clone();
            let host_send = rquickjs::Function::new(
                ctx.clone(),
                move |id: f64, method: String, url: String, headers_json: String, body: String| {
                    if policy_block {
                        let d = policy::decide(&url);
                        if d.blocked {
                            emit_event(
                                "policy_blocked",
                                json!({
                                    "url": url,
                                    "category": d.category.map(|c| c.as_str()),
                                    "matched": d.matched_pattern,
                                    "method": method,
                                }),
                            );
                            if let Ok(mut g) = results_for_block.lock() {
                                g.push(FetchResponse {
                                    id: id as u64,
                                    status: 204,
                                    headers: HashMap::new(),
                                    body: String::new(),
                                    error: None,
                                });
                            }
                            return;
                        }
                    }
                    let mut hmap: HashMap<String, String> = HashMap::new();
                    if let Ok(serde_json::Value::Object(map)) =
                        serde_json::from_str::<serde_json::Value>(&headers_json)
                    {
                        for (k, v) in map {
                            if let Some(s) = v.as_str() {
                                hmap.insert(k, s.to_string());
                            }
                        }
                    }
                    let req = FetchRequest {
                        id: id as u64,
                        method,
                        url,
                        headers: hmap,
                        body: body.into_bytes(),
                    };
                    let _ = sender.send(req);
                },
            )
            .map_err(|e| anyhow!("install __host_fetch_send: {e}"))?;
            ctx.globals()
                .set("__host_fetch_send", host_send)
                .map_err(|e| anyhow!("set __host_fetch_send: {e}"))?;

            // __host_drain_fetches() -> JSON-encoded array of pending FetchResponse.
            // JS-side parses and resolves the corresponding Promises.
            let results = fetch.results.clone();
            let host_drain = rquickjs::Function::new(ctx.clone(), move || -> String {
                let mut guard = match results.lock() {
                    Ok(g) => g,
                    Err(_) => return "[]".to_string(),
                };
                let drained: Vec<FetchResponse> = guard.drain(..).collect();
                drop(guard);
                serde_json::to_string(&drained).unwrap_or_else(|_| "[]".to_string())
            })
            .map_err(|e| anyhow!("install __host_drain_fetches: {e}"))?;
            ctx.globals()
                .set("__host_drain_fetches", host_drain)
                .map_err(|e| anyhow!("set __host_drain_fetches: {e}"))?;

            // __host_resolve_url(src, base) — delegates to Rust's url::Url::join,
            // which is fully spec-compliant (handles ../, ./, query-only,
            // fragment-only, scheme-relative). Used by dom.js's dynamic-script
            // loader so dynamic chunks resolve correctly. (PR #6 review medium.)
            // Returns the input src on parse failure — caller decides whether
            // to fall back to the JS-side regex resolver.
            let host_resolve_url =
                rquickjs::Function::new(ctx.clone(), |src: String, base: String| -> String {
                    if src.is_empty() {
                        return src;
                    }
                    match url::Url::parse(&base) {
                        Ok(b) => b.join(&src).map(|u| u.to_string()).unwrap_or(src),
                        Err(_) => src,
                    }
                })
                .map_err(|e| anyhow!("install __host_resolve_url: {e}"))?;
            ctx.globals()
                .set("__host_resolve_url", host_resolve_url)
                .map_err(|e| anyhow!("set __host_resolve_url: {e}"))?;

            // __host_raw_body() — returns the raw HTML body of the most
            // recent navigate. Used by extract.js's strategyRscPayload after
            // hydration scripts have removed inline `__next_f.push` script
            // elements (Next.js App Router does this after hydrating; the
            // raw body still contains the RSC chunks).
            let body_for_host = last_body_arc.clone();
            let host_raw_body = rquickjs::Function::new(ctx.clone(), move || -> String {
                body_for_host
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                    .unwrap_or_default()
            })
            .map_err(|e| anyhow!("install __host_raw_body: {e}"))?;
            ctx.globals()
                .set("__host_raw_body", host_raw_body)
                .map_err(|e| anyhow!("set __host_raw_body: {e}"))?;

            // __host_parse_html_fragment(html) — parses an HTML fragment
            // string into the same JSON tree shape main.rs's full document
            // parser produces. Used by dom.js's Element.innerHTML setter
            // and insertAdjacentHTML(); without it those silently no-op.
            // Context element is <body> (matches what real browsers do
            // for innerHTML on most elements). Returns a fragment-rooted
            // tree as JSON; caller JSON.parses and feeds to buildChildren.
            // (Implements piece #2 from the SPA-content-extraction proposal.)
            let host_parse_fragment =
                rquickjs::Function::new(ctx.clone(), |html: String| -> String {
                    parse_html_fragment_to_json(&html)
                })
                .map_err(|e| anyhow!("install __host_parse_html_fragment: {e}"))?;
            ctx.globals()
                .set("__host_parse_html_fragment", host_parse_fragment)
                .map_err(|e| anyhow!("set __host_parse_html_fragment: {e}"))?;

            Ok(())
        })?;
        // Bytecode cache setup: hash the JS env so cache files invalidate
        // automatically on shims.js / dom.js changes. Prune once at startup
        // so the cap is honored across many process lifetimes — we don't
        // pay the directory walk on every cache hit.
        let bytecode_cache_disabled = bytecode_cache::is_disabled();
        let bytecode_cache_root = bytecode_cache::cache_dir();
        let shim_hash = bytecode_cache::sha256(&format!(
            "{}\0{DOM_JS}\0{SHIMS_JS}\0{BLOCKMAP_JS}\0{INTERACT_JS}\0{EXTRACT_JS}\0{PAGE_MODEL_JS}",
            shim_mode.as_str()
        ));
        if !bytecode_cache_disabled {
            bytecode_cache::prune(&bytecode_cache_root, bytecode_cache::max_total_bytes());
        }
        let prefit = prefit::PrefitBundle::load_embedded();
        Ok(Self {
            js_rt,
            js_ctx,
            http,
            jar,
            _fetch: fetch,
            eval_deadline_ms,
            last_url: None,
            last_challenge: None,
            last_rate_limit: None,
            last_browser_route: None,
            last_body: last_body_arc,
            policy_block,
            nav_counter: AtomicU64::new(0),
            nav_ids_issued: Mutex::new(HashSet::new()),
            bytecode_cache_root,
            shim_hash,
            bytecode_cache_disabled,
            prefit,
            shim_mode,
        })
    }

    // Generate the next navigation_id for events emitted by navigate_with.
    // Format `nav_<n>` keeps it grep-friendly and short. Within a single
    // session (process lifetime) ids are unique and monotonic; not globally
    // unique — drivers that need cross-session correlation should pair this
    // with their own session id.
    fn next_nav_id(&self) -> String {
        let n = self.nav_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("nav_{n}");
        if let Ok(mut set) = self.nav_ids_issued.lock() {
            set.insert(id.clone());
        }
        id
    }

    fn nav_id_is_known(&self, id: &str) -> bool {
        self.nav_ids_issued
            .lock()
            .map(|set| set.contains(id))
            .unwrap_or(false)
    }

    // Set a wall-clock deadline (ms from now) that bounds every JS eval until
    // restored. Returns the previous deadline so the caller can restore it
    // (supports nested deadlines — the script phase tightens the navigate
    // budget, then restores the outer dispatcher budget). A budget of 0 means
    // "leave it unbounded"; the dispatcher should never call that.
    fn set_eval_deadline_from_now(&self, ms: u64) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let new_dl = now.saturating_add(ms);
        self.eval_deadline_ms.swap(new_dl, Ordering::Relaxed)
    }

    fn restore_eval_deadline(&self, prev: u64) {
        self.eval_deadline_ms.store(prev, Ordering::Relaxed);
    }

    // Eval that doesn't try to JSON.stringify the result. Right tool for
    // executing page <script> tags whose last expression often returns a
    // DOM Element (circular refs → stringify throws). Surfaces real JS
    // errors via ctx.catch() like eval() does.
    fn eval_void(&self, code: &str) -> Result<()> {
        self.js_ctx.with(|ctx| -> Result<()> {
            match ctx.eval::<rquickjs::Value, _>(code) {
                Ok(_) => Ok(()),
                Err(rquickjs::Error::Exception) => {
                    Err(anyhow!("{}", format_js_exception(ctx.catch())))
                }
                Err(e) => Err(anyhow!("js eval: {e}")),
            }
        })
    }

    // Eval with bytecode cache. On hit, skips the QuickJS parse phase
    // (the dominant cost on heavy React/Next bundles). On miss, compiles,
    // writes bytecode to disk, then executes. `name` is a debug-friendly
    // identifier (URL or "inline-{hash}") used for events and source-map-
    // style stack frames. Falls back to plain eval on any cache failure
    // so caching is opportunistic — never blocks correctness.
    //
    // See src/bytecode_cache.rs for the unsafe QuickJS glue.
    fn eval_with_cache(&self, source: &str, name: &str) -> Result<()> {
        if self.bytecode_cache_disabled {
            return self.eval_void(source);
        }
        let key = bytecode_cache::cache_key(source, &self.shim_hash);
        let root = &self.bytecode_cache_root;
        // The eval-deadline watchdog must be SUSPENDED only across
        // compile_to_bytecode (JS_Eval COMPILE_ONLY) — the watchdog
        // spuriously aborts on >~16KB scripts during the parse-only path.
        // It MUST stay armed across load_and_eval, which actually runs
        // user code and can loop forever. Earlier code suspended it for
        // the whole closure, which let cached bundles run unbounded.
        let dl = self.eval_deadline_ms.clone();
        self.js_ctx.with(|ctx| -> Result<()> {
            // Try cache. Watchdog stays armed — load_and_eval runs user code.
            if let Some(bytes) = bytecode_cache::read(root, &key) {
                let bytes_len = bytes.len();
                match bytecode_cache::load_and_eval(&ctx, &bytes) {
                    Ok(()) => {
                        emit_event(
                            "bytecode_cache",
                            json!({
                                "schema_version": 1,
                                "hit": true,
                                "name": name,
                                "bytes": bytes_len,
                            }),
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        // Could be a true JS exception OR a watchdog interrupt
                        // (cached bundle ran past the deadline). Surface either
                        // back to the caller so script_executed gets the right
                        // error and `interrupted` flag.
                        emit_event(
                            "bytecode_cache",
                            json!({
                                "schema_version": 1,
                                "hit": true,
                                "name": name,
                                "load_error": e,
                            }),
                        );
                        let caught = ctx.catch();
                        return if caught.is_null() || caught.is_undefined() {
                            Err(anyhow!("bytecode eval threw (no exception captured)"))
                        } else {
                            Err(anyhow!("{}", format_js_exception(caught)))
                        };
                    }
                }
            }
            // Cache miss: compile (watchdog suspended just for this call),
            // persist, then execute via the freshly-produced bytecode.
            let prev_dl = dl.swap(0, Ordering::Relaxed);
            let compile_res = bytecode_cache::compile_to_bytecode(&ctx, source, name);
            dl.store(prev_dl, Ordering::Relaxed);
            match compile_res {
                Ok(bytes) => {
                    let _ = bytecode_cache::write(root, &key, &bytes);
                    let bytes_len = bytes.len();
                    let result = bytecode_cache::load_and_eval(&ctx, &bytes);
                    emit_event(
                        "bytecode_cache",
                        json!({
                            "schema_version": 1,
                            "hit": false,
                            "name": name,
                            "compiled_bytes": bytes_len,
                        }),
                    );
                    match result {
                        Ok(()) => Ok(()),
                        // Eval-time exception OR watchdog interrupt.
                        Err(_) => {
                            let caught = ctx.catch();
                            if caught.is_null() || caught.is_undefined() {
                                Err(anyhow!("bytecode eval threw (no exception captured)"))
                            } else {
                                Err(anyhow!("{}", format_js_exception(caught)))
                            }
                        }
                    }
                }
                Err(e) => {
                    // Compile failure (SyntaxError, OOM, or QuickJS refusal).
                    // Surface via NDJSON so drivers see why caching skipped.
                    // Fall back to plain eval — matches eval_void's
                    // exception path so JS-level errors still surface.
                    emit_event(
                        "bytecode_cache",
                        json!({
                            "schema_version": 1,
                            "hit": false,
                            "name": name,
                            "compile_error": e,
                        }),
                    );
                    match ctx.eval::<rquickjs::Value, _>(source) {
                        Ok(_) => Ok(()),
                        Err(rquickjs::Error::Exception) => {
                            Err(anyhow!("{}", format_js_exception(ctx.catch())))
                        }
                        Err(e) => Err(anyhow!("js eval: {e}")),
                    }
                }
            }
        })
    }

    fn eval(&self, code: &str) -> Result<Value> {
        self.js_ctx.with(|ctx| -> Result<Value> {
            let val = match ctx.eval::<rquickjs::Value, _>(code) {
                Ok(v) => v,
                Err(rquickjs::Error::Exception) => {
                    return Err(anyhow!("{}", format_js_exception(ctx.catch())));
                }
                Err(e) => return Err(anyhow!("js eval: {e}")),
            };
            if val.is_undefined() {
                return Ok(Value::Null);
            }
            let json_obj: rquickjs::Object = ctx
                .globals()
                .get("JSON")
                .map_err(|e| anyhow!("get JSON: {e}"))?;
            let stringify: rquickjs::Function = json_obj
                .get("stringify")
                .map_err(|e| anyhow!("get stringify: {e}"))?;
            let result: rquickjs::Value = stringify
                .call((val,))
                .map_err(|e| anyhow!("call stringify: {e}"))?;
            if result.is_undefined() || result.is_null() {
                return Ok(Value::Null);
            }
            let s = String::from_js(&ctx, result).map_err(|e| anyhow!("to string: {e}"))?;
            Ok(serde_json::from_str(&s).unwrap_or(Value::String(s)))
        })
    }

    fn fragment_target_present(&self, requested_url: &str) -> Option<bool> {
        let candidates = challenge::fragment_target_candidates(requested_url)?;
        let candidates_json = serde_json::to_string(&candidates).ok()?;
        let code = format!(
            "(function(){{ \
                var targets = {candidates_json}; \
                var all = document.getElementsByTagName('*'); \
                for (var i = 0; i < all.length; i++) {{ \
                    var id = all[i].getAttribute('id'); \
                    var name = all[i].getAttribute('name'); \
                    if (targets.indexOf(id) !== -1 || targets.indexOf(name) !== -1) return true; \
                }} \
                return false; \
            }})()"
        );
        Some(
            self.eval(&code)
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
        )
    }

    async fn navigate(&mut self, url: &str, exec_scripts: bool) -> Result<Value> {
        self.navigate_with(self.http.get(url), url, exec_scripts, false)
            .await
    }

    /// Navigate with optional ascii blockmap for human debugging.
    async fn navigate_opts(
        &mut self,
        url: &str,
        exec_scripts: bool,
        include_ascii: bool,
    ) -> Result<Value> {
        self.navigate_with(self.http.get(url), url, exec_scripts, include_ascii)
            .await
    }

    // Shared pipeline: take an already-built wreq::RequestBuilder (GET from
    // navigate, POST from submit), send it, run it through DOM seeding,
    // BlockMap, challenge detection, and optional script execution. Keeps
    // GET and POST coherent on cookies/TLS-FP/redirect handling without a
    // second copy of the post-fetch logic.
    async fn navigate_with(
        &mut self,
        req: wreq::RequestBuilder,
        requested_url: &str,
        exec_scripts: bool,
        include_ascii: bool,
    ) -> Result<Value> {
        let nav_start = std::time::Instant::now();
        let nav_id = self.next_nav_id();
        let resp = req.send().await.context("http send")?;
        let status = resp.status().as_u16();
        let final_url = resp.uri().to_string();
        // Defer navigation_started until DOM is seeded — pairing invariant:
        // if navigation_started fires, policy_trace WILL fire before this
        // function returns. Errors above this point (http send, body read,
        // DOM seed) propagate without firing either event, so a driver
        // never sees an orphan navigation_id. See review of PR #4 H2.

        // Snapshot useful response headers before consuming the response body.
        // Multi-value headers (Set-Cookie) are joined with ' || ' since they're
        // mostly diagnostic — the actual cookie storage already happened in
        // wreq's CookieStore impl.
        // Full header map for internal use (network_store, challenge detection).
        let mut headers: serde_json::Map<String, Value> = serde_json::Map::new();
        let mut headers_flat: HashMap<String, String> = HashMap::new();
        for (name, value) in resp.headers() {
            let key = name.as_str().to_lowercase();
            let v = value.to_str().unwrap_or("").to_string();
            match headers.get_mut(&key) {
                Some(Value::String(existing)) => {
                    *existing = format!("{existing} || {v}");
                }
                _ => {
                    headers.insert(key.clone(), Value::String(v.clone()));
                }
            }
            headers_flat.insert(key, v);
        }

        // Filtered headers for the agent-facing response: keep only the fields
        // an LLM agent can actually act on. Full headers are ~1-8K tokens of
        // CSP policies, preload hints, CDN metadata, ad-tech domains — noise
        // that inflates every navigate response with zero agent utility.
        let mut response_headers: serde_json::Map<String, Value> = serde_json::Map::new();
        for key in ["content-type", "location"] {
            if let Some(v) = headers.get(key) {
                response_headers.insert((*key).to_string(), v.clone());
            }
        }

        let body = resp.text().await.context("read body")?;
        let bytes = body.len();

        // Capture the navigate response itself into the network store.
        // JSON-shaped landing pages (raw GraphQL endpoints, JSON feeds,
        // route-data preloads) get surfaced via the network_stores RPC
        // alongside fetch/XHR responses from page scripts. The navigate
        // body is often the single most important content-bearing fetch
        // on a JSON-API-shaped site. HTML pages are skipped by the
        // classifier (text/html → score 0).
        if (200..400).contains(&status)
            && !body.is_empty()
            && let Ok(mut s) = self._fetch.network_store.lock()
        {
            s.maybe_capture(
                &final_url,
                "GET",
                status,
                &headers_flat,
                &body,
                Some(&nav_id),
            );
        }

        let rate_limit_detection = challenge::detect_rate_limit(status, &body, &headers_flat);
        let rate_limit = rate_limit_detection
            .as_ref()
            .map(|d| serde_json::to_value(d).unwrap_or_default());
        let challenge_detection = challenge::detect(status, &body);
        if let Some(d) = &challenge_detection {
            emit_event("challenge", serde_json::to_value(d).unwrap_or_default());
            if let Some(solution_url) = challenge::solve_url(d, &body, &final_url) {
                emit_event(
                    "challenge_auto_solved",
                    json!({
                        "schema_version": 1,
                        "navigation_id": nav_id,
                        "provider": d.provider,
                        "solution_url": solution_url,
                    }),
                );
                return Box::pin(self.navigate(&solution_url, exec_scripts)).await;
            }
        }
        let challenge = challenge_detection.map(|d| serde_json::to_value(d).unwrap_or_default());

        let tree = parse_html_to_tree(&body);
        self.seed_dom(&tree)?;

        // Publish current_nav_id to the worker so any fetch that resolves
        // during this navigate's settle loop is bound to this navigation.
        // Set after seed_dom to match the navigation_started invariant —
        // this nav_id is now "live" until the next navigate overwrites it.
        if let Ok(mut g) = self._fetch.current_nav_id.lock() {
            *g = Some(nav_id.clone());
        }

        // DOM is now committed for this nav_id. From here on, the function
        // path always reaches the policy_trace emission (script branches
        // both emit it; non-exec branch emits a minimal trace). Safe to
        // announce navigation_started.
        emit_event(
            "navigation_started",
            json!({
                "schema_version": 1,
                "navigation_id": nav_id,
                "url": final_url,
                "status": status,
                "bytes": bytes,
                "exec_scripts": exec_scripts,
                "policy_block": self.policy_block,
            }),
        );

        // Prefit lookup (R2 from white paper §6 Track 1). Look up the
        // target host in the embedded prefit bundle. If we have an entry,
        // emit `prefit_applied` so drivers see what per-domain knowledge
        // is in play, and capture the additions for the script-fetch loop
        // below to extend Tier-1 blocking with.
        let prefit_for_domain: Option<&prefit::DomainPrefit> = self.prefit.as_ref().and_then(|b| {
            url::Url::parse(&final_url)
                .ok()
                .and_then(|u| u.host_str().map(|s| s.to_string()))
                .and_then(|host| b.lookup_domain(&host))
        });
        if let Some(p) = prefit_for_domain {
            emit_event(
                "prefit_applied",
                json!({
                    "schema_version": 1,
                    "navigation_id": nav_id,
                    "domain": p.domain,
                    "framework": p.framework,
                    "blocklist_additions": p.blocklist_additions.len(),
                    "shape_hint": p.shape_hint,
                    "settle_distribution": p.settle_distribution,
                }),
            );
        }

        // Update window.location for any page scripts that read it.
        let url_lit = serde_json::to_string(&final_url)?;
        let _ = self.eval(&format!("__setLocation({url_lit})"));

        // Phase 5: optionally execute page scripts (inline + external src).
        // Per-decision accumulator for the synthetic outcome path (see
        // derive_outcome below). Built up through the script-fetch and
        // assembly passes; emitted as `outcome_for_decision` events after
        // the navigate's success/fail verdict is computed. fetch_failed
        // decisions are intentionally not recorded — that's a network
        // failure, not a policy choice we want T2 to learn from.
        let mut decisions: Vec<DecisionRecord> = Vec::new();
        let scripts = if exec_scripts && (200..400).contains(&status) {
            let items = collect_scripts(&tree, &final_url);
            let mut inline_count = 0usize;
            let mut external_count = 0usize;
            let mut async_count = 0usize;
            let mut policy_blocked_count = 0usize;
            let mut fetch_errors: Vec<String> = Vec::new();

            // Spawn external fetches in parallel — current_thread runtime
            // interleaves them at network-I/O await points, so a page with
            // N external bundles takes ~max(round-trip times) instead of
            // sum(round-trip times). Each task has a per-fetch timeout so
            // a single huge bundle can't hang the navigate indefinitely.
            // Document ordering preserved by indexing results.
            //
            // Policy hook: when self.policy_block is on, decide(url) gates
            // each external fetch BEFORE we spawn the task. Static <script
            // src> tracker URLs (Adobe DTM, Ketch, GoogleTagServices, etc.)
            // are caught here — they bypass __host_fetch_send because page
            // scripts haven't run yet to issue them, so this is the
            // structurally correct place for the gate. See src/policy.rs.
            const SCRIPT_FETCH_TIMEOUT_MS: u64 = 8000;
            let mut fetch_tasks: Vec<(usize, tokio::task::JoinHandle<Result<String, String>>)> =
                Vec::new();
            // Authoritative record of which script ids were skipped at first
            // pass. Replaces the previous "re-call policy::decide in assembly
            // pass" approach (review M4) — fragile if policy_block toggles
            // mid-navigate or if any non-deterministic structural prior
            // enters policy::decide later. HashSet keeps the assembly pass
            // O(1) per item.
            let mut skipped_ids: HashSet<usize> = HashSet::new();
            for (idx, item) in items.iter().enumerate() {
                if let ScriptItem::External { url: u, kind } = item {
                    let host = host_of(u);
                    let kind_str = script_kind_str(*kind);
                    if self.policy_block {
                        let d = policy::decide(u);
                        if d.blocked {
                            // Spec §6 schema: action enum is small (skip|run|
                            // fetch_failed), reasons compose orthogonally.
                            // Was previously action: "skip_blocklist".
                            emit_event(
                                "script_decision",
                                json!({
                                    "schema_version": 1,
                                    "navigation_id": nav_id,
                                    "script_id": idx,
                                    "url": u,
                                    "host": host,
                                    "kind": kind_str,
                                    "action": "skip",
                                    "reason": "blocklist",
                                    "category": d.category.map(|c| c.as_str()),
                                    "matched": d.matched_pattern,
                                }),
                            );
                            // Legacy event — kept for one cycle for back-compat
                            // with policy_baseline.py / policy_e2e.py. Drop in
                            // a follow-up PR once consumers have switched.
                            emit_event(
                                "policy_blocked",
                                json!({
                                    "url": u,
                                    "kind": "static_script",
                                    "category": d.category.map(|c| c.as_str()),
                                    "matched": d.matched_pattern,
                                }),
                            );
                            decisions.push(DecisionRecord {
                                action: "skip",
                                host: host.clone(),
                            });
                            policy_blocked_count += 1;
                            skipped_ids.insert(idx);
                            continue;
                        }
                        // Tier-1.5: per-domain blocklist additions from the
                        // prefit bundle. URLs that aren't in the global
                        // Tier-1 list but ARE in this domain's known-tracker
                        // set are also skipped. Reason recorded as
                        // "prefit_blocklist" so drivers can distinguish.
                        //
                        // Bayesian gate (R2 / decide()): when we have a
                        // posterior for `block:<host>` on this domain, draw
                        // a Thompson sample and gate the block on
                        // `sample >= threshold`. With a high-confidence
                        // posterior (lots of "blocked & succeeded" evidence)
                        // we block aggressively; with Beta(1, 1)
                        // placeholders we coin-flip; with strong evidence
                        // *against* blocking we let the script through.
                        // No posterior → fall through to the deterministic
                        // block (preserves prior behavior on un-trained
                        // entries). Threshold defaults to 0.5 — the natural
                        // Bayesian decision boundary, see prefit::decide.
                        if let (Some(bundle), Some(p)) = (self.prefit.as_ref(), prefit_for_domain)
                            && bundle.matches_blocklist_addition(p, u)
                        {
                            let decision_key = format!("block:{host}");
                            const THRESHOLD: f64 = 0.5;
                            // Minimum observation count before we'll
                            // trust a posterior to override the
                            // deterministic block. Tier-1.5 prefit
                            // entries ship with `Beta(1, 1) / n=0`
                            // placeholders ("we know about this
                            // domain, no data yet") — sampling those
                            // gives a uniform draw and would let
                            // ~50% of hand-curated tracker hits
                            // through. Bias is "deterministic until
                            // evidence flips it": only consult once
                            // we've seen ≥5 trials, which is the
                            // smallest n where a Beta posterior's
                            // 95% credible interval is meaningfully
                            // narrower than the prior.
                            const MIN_POSTERIOR_OBSERVATIONS: u64 = 5;
                            // Default to the deterministic block when no
                            // posterior exists OR when the posterior
                            // is too thin to be informative
                            // (preserves prior behavior on un-trained
                            // entries). When a posterior IS present
                            // and well-observed, use Thompson sampling
                            // via `decide_traced` so the gate is informed.
                            let post_opt = bundle.lookup_posterior(&p.domain, &decision_key);
                            let has_useful_posterior = post_opt
                                .as_ref()
                                .map(|p| p.n >= MIN_POSTERIOR_OBSERVATIONS)
                                .unwrap_or(false);
                            let (block_now, outcome) = if has_useful_posterior {
                                let mut rng = rand::thread_rng();
                                let out = bundle.decide_traced(
                                    &mut rng,
                                    &p.domain,
                                    &decision_key,
                                    THRESHOLD,
                                );
                                (out.blocked, Some(out))
                            } else {
                                (true, None)
                            };
                            if let Some(out) = outcome
                                && let (Some(post), Some(s)) = (out.posterior, out.sampled)
                            {
                                emit_event(
                                    "posterior_consulted",
                                    json!({
                                        "schema_version": 1,
                                        "navigation_id": nav_id,
                                        "decision_key": decision_key,
                                        "domain": p.domain,
                                        "alpha": post.alpha,
                                        "beta": post.beta,
                                        "n": post.n,
                                        "sampled": s,
                                        "threshold": THRESHOLD,
                                        "blocked": block_now,
                                    }),
                                );
                            }
                            if block_now {
                                let mut decision = json!({
                                    "schema_version": 1,
                                    "navigation_id": nav_id,
                                    "script_id": idx,
                                    "url": u,
                                    "host": host,
                                    "kind": kind_str,
                                    "action": "skip",
                                    "reason": "prefit_blocklist",
                                    "domain": p.domain,
                                });
                                // When we consulted a posterior, attach it
                                // so script_decision rows are self-contained
                                // for downstream credit assignment.
                                if let Some(out) = outcome
                                    && let (Some(post), Some(s)) = (out.posterior, out.sampled)
                                {
                                    decision["posterior_alpha"] = json!(post.alpha);
                                    decision["posterior_beta"] = json!(post.beta);
                                    decision["posterior_n"] = json!(post.n);
                                    decision["posterior_sampled"] = json!(s);
                                }
                                emit_event("script_decision", decision);
                                // Record skip for synthetic outcome derivation.
                                // We push only on the actual block path — when
                                // the Bayesian gate lets the script through,
                                // the queued/allow record is emitted by the
                                // fetch path below, so T2 sees consistent
                                // (decision_key, outcome) pairs.
                                decisions.push(DecisionRecord {
                                    action: "skip",
                                    host: host.clone(),
                                });
                                policy_blocked_count += 1;
                                skipped_ids.insert(idx);
                                continue;
                            }
                            // Posterior gated us out of blocking — let the
                            // script through. We DON'T emit a script_decision
                            // here (the action will be "queued" — emitted
                            // implicitly by the fetch path below).
                        }
                    }
                    let url = u.clone();
                    let http = self.http.clone();
                    fetch_tasks.push((
                        idx,
                        tokio::spawn(async move {
                            let fut = async {
                                match http.get(&url).send().await {
                                    Ok(resp) if resp.status().is_success() => {
                                        match resp.text().await {
                                            Ok(body) => Ok(body),
                                            Err(e) => Err(format!("read {url}: {e}")),
                                        }
                                    }
                                    Ok(resp) => {
                                        Err(format!("status {} fetching {}", resp.status(), url))
                                    }
                                    Err(e) => Err(format!("fetch {url}: {e}")),
                                }
                            };
                            match tokio::time::timeout(
                                std::time::Duration::from_millis(SCRIPT_FETCH_TIMEOUT_MS),
                                fut,
                            )
                            .await
                            {
                                Ok(r) => r,
                                Err(_) => Err(format!(
                                    "timeout {SCRIPT_FETCH_TIMEOUT_MS}ms fetching {url}"
                                )),
                            }
                        }),
                    ));
                }
            }
            let mut external_results: HashMap<usize, String> = HashMap::new();
            for (idx, task) in fetch_tasks {
                match task.await {
                    Ok(Ok(body)) => {
                        external_results.insert(idx, body);
                        external_count += 1;
                    }
                    Ok(Err(e)) => fetch_errors.push(e),
                    Err(join_e) => fetch_errors.push(format!("task panicked: {join_e}")),
                }
            }

            // Two-pass assembly to honor `async` script semantics:
            //   sync_sources  — Inline + External(Sync) in document order
            //   async_sources — External(Async) in document order, executed
            //                   AFTER the sync queue. The HTML spec lets async
            //                   scripts execute as soon as their fetch
            //                   completes (no order guarantee w.r.t. other
            //                   scripts); we approximate by executing them
            //                   last in document order, which is spec-legal
            //                   (well-behaved async scripts can't depend on
            //                   ordering anyway) and trivially deterministic
            //                   for replay/measurement. Defer is folded into
            //                   Sync — we have no incremental parsing, so
            //                   "execute after parse in document order"
            //                   collapses to "execute now in document order."
            // Each entry pairs (script_id, kind_str, optional url, body) so
            // the eval loop below can emit a script_executed event per
            // source with the correct script_id and url for credit
            // assignment by future Bayesian phases.
            let mut sync_sources: Vec<(usize, &'static str, Option<String>, String)> = Vec::new();
            let mut async_sources: Vec<(usize, &'static str, Option<String>, String)> = Vec::new();
            let mut fetch_failed_count = 0usize;
            for (idx, item) in items.into_iter().enumerate() {
                match item {
                    ScriptItem::Inline(s) => {
                        inline_count += 1;
                        // No script_decision for inline (v0 emits decisions
                        // for external only — inline scripts always run).
                        sync_sources.push((idx, "inline", None, s));
                    }
                    ScriptItem::External { url, kind } => {
                        if skipped_ids.contains(&idx) {
                            // Already emitted script_decision(skip) at first pass.
                            continue;
                        }
                        let host = host_of(&url);
                        let kind_str = script_kind_str(kind);
                        if let Some(body) = external_results.remove(&idx) {
                            // Spec §6: action enum is run|skip|fetch_failed.
                            // We use "queued" here because eval has not yet
                            // happened — the actual execution outcome is
                            // reported separately via script_executed below.
                            // Drivers wanting "ran successfully" should join
                            // script_decision{action: queued} with
                            // script_executed{error: null}.
                            emit_event(
                                "script_decision",
                                json!({
                                    "schema_version": 1,
                                    "navigation_id": nav_id,
                                    "script_id": idx,
                                    "url": url,
                                    "host": host,
                                    "kind": kind_str,
                                    "action": "queued",
                                }),
                            );
                            decisions.push(DecisionRecord {
                                action: "queued",
                                host: host.clone(),
                            });
                            match kind {
                                ScriptKind::Sync => {
                                    sync_sources.push((idx, kind_str, Some(url), body));
                                }
                                ScriptKind::Async => {
                                    async_count += 1;
                                    async_sources.push((idx, kind_str, Some(url), body));
                                }
                            }
                        } else {
                            fetch_failed_count += 1;
                            emit_event(
                                "script_decision",
                                json!({
                                    "schema_version": 1,
                                    "navigation_id": nav_id,
                                    "script_id": idx,
                                    "url": url,
                                    "host": host,
                                    "kind": kind_str,
                                    "action": "fetch_failed",
                                }),
                            );
                        }
                    }
                }
            }
            let sources: Vec<(usize, &'static str, Option<String>, String)> =
                sync_sources.into_iter().chain(async_sources).collect();
            // Eval all in document order. Page scripts often end with an
            // Element-returning expression (circular refs → JSON.stringify
            // throws), so use eval_void.
            //
            // Bound total eval time. Heavy React/Vue bundles can run pathological
            // top-level code in QuickJS for tens of seconds; we don't want a
            // single navigate hanging the binary. The watchdog interrupt
            // handler installed in Session::new fires periodically inside
            // QuickJS and aborts any running script (or settle pump callback,
            // or microtask) once the deadline passes. Tighten the outer
            // dispatcher budget for the script-eval phase, then restore.
            let script_eval_budget_ms = read_script_eval_budget_ms();
            let prev_deadline = self.set_eval_deadline_from_now(script_eval_budget_ms);

            let script_phase_start = std::time::Instant::now();
            let mut eval_errors: Vec<String> = Vec::new();
            let mut executed: usize = 0;
            let mut interrupted: usize = 0;
            let mut budget_exhausted = false;
            let mut budget_skipped: usize = 0;
            for (idx, (script_id, kind_str, url, source)) in sources.iter().enumerate() {
                if script_phase_start.elapsed().as_millis() as u64 >= script_eval_budget_ms {
                    budget_exhausted = true;
                    budget_skipped = sources.len().saturating_sub(idx);
                    break;
                }
                let eval_start = std::time::Instant::now();
                // Set document.currentScript so webpack's automatic-publicPath
                // detection works. Bluesky's main.js (and many webpack bundles
                // with chunked output) calls
                //   __webpack_require__.p = <derive from currentScript.src>
                // and throws "Automatic publicPath is not supported in this
                // browser" if currentScript is missing — that bails hydration
                // before any DOM mounting happens.
                if let Some(u) = url.as_deref() {
                    let url_lit = serde_json::to_string(u).unwrap_or_default();
                    let _ = self.eval_void(&format!("__setCurrentScript({url_lit})"));
                }
                // Three-way routing:
                //   1. Module-shaped sources (PR #11) → __loadModule, which
                //      recursively loads deps then evals the cleaned body.
                //      Returns a Promise; settle drives it to completion.
                //      Bytecode caching skipped for modules — the loader
                //      strips imports before eval, so the cached bytecode
                //      would not match the public source's hash.
                //   2. Classic sources → eval_with_cache. Hit skips parse;
                //      miss compiles + caches + evals.
                let result = if looks_like_module(source)
                    || (self.shim_mode == ShimMode::Enhanced && looks_like_enhanced_module(source))
                {
                    let src_lit = serde_json::to_string(source).unwrap_or_default();
                    let url_lit =
                        serde_json::to_string(url.as_deref().unwrap_or("")).unwrap_or_default();
                    self.eval_void(&format!("__loadModule({src_lit}, {url_lit})"))
                } else {
                    let cache_name = url.as_deref().unwrap_or("inline").to_string();
                    self.eval_with_cache(source, &cache_name)
                };
                if url.is_some() {
                    let _ = self.eval_void("__setCurrentScript(null)");
                }
                let duration_us = eval_start.elapsed().as_micros() as u64;
                match result {
                    Err(e) => {
                        let msg = e.to_string();
                        let is_interrupt = msg.contains("interrupted");
                        if is_interrupt {
                            interrupted += 1;
                            budget_exhausted = true;
                            budget_skipped = sources.len().saturating_sub(idx + 1);
                        }
                        let truncated = if msg.len() > 200 {
                            format!("{}…", &msg[..200])
                        } else {
                            msg.clone()
                        };
                        eval_errors.push(truncated.clone());
                        // Spec §6: script_executed reports actual runtime
                        // outcome, distinct from script_decision (queued).
                        emit_event(
                            "script_executed",
                            json!({
                                "schema_version": 1,
                                "navigation_id": nav_id,
                                "script_id": script_id,
                                "url": url,
                                "kind": kind_str,
                                "duration_us": duration_us,
                                "error": truncated,
                                "interrupted": is_interrupt,
                            }),
                        );
                        if is_interrupt {
                            break;
                        }
                    }
                    Ok(()) => {
                        executed += 1;
                        emit_event(
                            "script_executed",
                            json!({
                                "schema_version": 1,
                                "navigation_id": nav_id,
                                "script_id": script_id,
                                "url": url,
                                "kind": kind_str,
                                "duration_us": duration_us,
                                "error": Value::Null,
                                "interrupted": false,
                            }),
                        );
                    }
                }
            }

            // Restore the dispatcher's outer deadline so settle's pumps run
            // under the broader navigate budget rather than the tight 5s
            // script-phase one. (Settle pump callbacks are bounded too — they
            // run inside QuickJS evals which still consult the same atomic.)
            self.restore_eval_deadline(prev_deadline);
            // If the last script hit the watchdog, clearing currentScript under
            // the expired deadline may also have been interrupted. Clear it once
            // more after restoring the normal RPC budget.
            let _ = self.eval_void("__setCurrentScript(null)");
            // Fire DOMContentLoaded → settle → load → settle. Each settle
            // emits a `settle_exit` event with reason + counts so traces show
            // exactly why we bailed (idle / budget_exhausted / max_iters).
            // Without this, a hung Next.js hydration looks indistinguishable
            // from a clean exit in the NDJSON stream — only `policy_trace`
            // carries the settle blob and it's often truncated.
            let _ = self
                .eval("typeof __fireDOMContentLoaded === 'function' && __fireDOMContentLoaded()");
            let after_dcl = self.settle(2000, 100).await.ok();
            if let Some(r) = &after_dcl {
                emit_event(
                    "settle_exit",
                    json!({
                        "schema_version": 1,
                        "navigation_id": nav_id,
                        "phase": "after_dcl",
                        "result": r,
                    }),
                );
            }
            let _ = self.eval("typeof __fireLoad === 'function' && __fireLoad()");
            let after_load = self.settle(1500, 50).await.ok();
            if let Some(r) = &after_load {
                emit_event(
                    "settle_exit",
                    json!({
                        "schema_version": 1,
                        "navigation_id": nav_id,
                        "phase": "after_load",
                        "result": r,
                    }),
                );
            }
            // Phase A: per-navigation policy trace. One event summarizing
            // every decision made during this navigate, joined to outcomes
            // via navigation_id when the driver later calls report_outcome.
            // See docs/probabilistic-policy.md §4.5.
            emit_event(
                "policy_trace",
                json!({
                    "schema_version": 1,
                    "navigation_id": nav_id,
                    "url": final_url,
                    "policy_block": self.policy_block,
                    "scripts": {
                        "inline": inline_count,
                        "external": external_count,
                        "async": async_count,
                        "skipped_blocklist": policy_blocked_count,
                        "fetch_failed": fetch_failed_count,
                        "budget_ms": script_eval_budget_ms,
                        "executed": executed,
                        "interrupted": interrupted,
                        "budget_exhausted": budget_exhausted,
                        "budget_skipped": budget_skipped,
                    },
                    "settle": {
                        "after_dcl": after_dcl,
                        "after_load": after_load,
                    },
                    "elapsed_ms": nav_start.elapsed().as_millis() as u64,
                }),
            );
            Some(json!({
                "inline_count": inline_count,
                "external_count": external_count,
                "async_count": async_count,
                "policy_blocked": policy_blocked_count,
                "fetch_failed": fetch_failed_count,
                "budget_ms": script_eval_budget_ms,
                "executed": executed,
                "interrupted": interrupted,
                "budget_exhausted": budget_exhausted,
                "budget_skipped": budget_skipped,
                "errors_count": eval_errors.len(),
                "errors": eval_errors.into_iter().take(10).collect::<Vec<_>>(),
                "fetch_errors_count": fetch_errors.len(),
                "fetch_errors": fetch_errors.into_iter().take(10).collect::<Vec<_>>(),
                "settle_after_dcl": after_dcl,
                "settle_after_load": after_load,
            }))
        } else {
            // exec_scripts=false: still emit a minimal policy_trace so the
            // driver always has a paired event for navigation_started.
            emit_event(
                "policy_trace",
                json!({
                    "schema_version": 1,
                    "navigation_id": nav_id,
                    "url": final_url,
                    "policy_block": self.policy_block,
                    "scripts": null,
                    "settle": null,
                    "elapsed_ms": nav_start.elapsed().as_millis() as u64,
                }),
            );
            None
        };

        self.last_url = Some(final_url.clone());
        if let Ok(mut g) = self.last_body.lock() {
            *g = Some(body.clone());
        }

        let mut blockmap = self.blockmap().unwrap_or(Value::Null);
        apply_status_to_blockmap(&mut blockmap, status);

        // Strip the human-readable ASCII grid by default — it's redundant
        // with the JSON structure/headings/density fields and costs
        // ~500-8000 chars per page with zero agent utility.
        if !include_ascii && let Some(obj) = blockmap.as_object_mut() {
            obj.remove("ascii");
        }

        let fragment_target_present = self.fragment_target_present(requested_url);
        let browser_route =
            challenge::detect_browser_route(status, &body, &blockmap).or_else(|| {
                fragment_target_present.and_then(|target_present| {
                    challenge::detect_unresolved_fragment_route(
                        status,
                        requested_url,
                        target_present,
                    )
                })
            });
        self.last_challenge = challenge.clone();
        self.last_rate_limit = rate_limit.clone();
        self.last_browser_route = if challenge.is_none() && rate_limit.is_none() {
            browser_route.clone()
        } else {
            None
        };

        // Auto-extract whenever the page embeds JSON-bearing <script> tags
        // (density.json_scripts > 0). Across a 32-site sweep this delivers
        // substantial structured data (JSON-LD article schemas, __NEXT_DATA__
        // page state, json_in_script product blobs, GitHub RSC payloads) on
        // ~15/20 sites where the agent would otherwise have to issue a second
        // extract() call. The earlier conjunctive gate on `likely_js_filled`
        // was empirically inert: shell-shaped pages and JSON-bearing pages are
        // anti-correlated in the wild — if a site is a thin shell it usually
        // fetches data later via XHR; if it embeds JSON it usually rendered
        // enough HTML to not look like a shell.
        //
        // Cost: __extract() is a sync QuickJS eval over the already-parsed
        // DOM (no network, no re-parse). On pages with only meta tags this is
        // sub-ms. On JSON-heavy pages a JSON.parse pass + the FFI roundtrip
        // back through serde_json runs ~20–150ms — bounded by the inline-size
        // cap below so a runaway result can't bloat the navigate response.
        //
        // Inline cap rationale: navigate's response is a single JSON-RPC
        // message the agent must consume as context. 256 KB of Next.js page
        // props (~64K tokens) dominates the response on SPA sites, most of
        // which is infrastructure (analytics, navigation, feature flags)
        // rather than actual page content. 16 KB (~4K tokens) provides a
        // compact preview — the agent gets enough to decide whether to call
        // extract() for the full payload. On overflow we return a stub
        // carrying strategy / confidence / size / all_hits_summary so the
        // agent knows what's available.
        const MAX_INLINE_EXTRACT_BYTES: usize = 16 * 1024;

        let json_scripts = blockmap
            .get("density")
            .and_then(|d| d.get("json_scripts"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let (auto_extract, auto_extract_error) = if json_scripts > 0 {
            match self.extract(None) {
                Ok(v) => {
                    let size = serde_json::to_string(&v).map(|s| s.len()).unwrap_or(0);
                    if size > MAX_INLINE_EXTRACT_BYTES {
                        // Primary doesn't fit. Walk all_hits (already sorted
                        // by confidence desc) and pick the first one that
                        // does fit — gives the agent SOME usable data inline
                        // instead of a stub. e.g. Polymarket's next_data is
                        // ~750KB; json_ld (20KB, conf 0.95) fits and carries
                        // the markets list. Agent can call extract() for the
                        // full primary if they want.
                        let primary_strategy = v
                            .get("strategy")
                            .and_then(|s| s.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let primary_confidence =
                            v.get("confidence").cloned().unwrap_or(Value::Null);
                        let mut chosen: Option<Value> = None;
                        if let Some(hits) = v.get("all_hits").and_then(|h| h.as_array()) {
                            for hit in hits {
                                let hit_size =
                                    serde_json::to_string(hit).map(|s| s.len()).unwrap_or(0);
                                if hit_size <= MAX_INLINE_EXTRACT_BYTES {
                                    let mut sub = json!({
                                            "strategy": hit.get("strategy").cloned().unwrap_or(Value::Null),
                                            "confidence": hit.get("confidence").cloned().unwrap_or(Value::Null),
                                            "data": hit.get("data").cloned().unwrap_or(Value::Null),
                                            "primary_truncated": {
                                                "strategy": primary_strategy.clone(),
                                                "confidence": primary_confidence.clone(),
                                                "size_bytes": size,
                                    "hint": format!(
                                        "primary \"{strat}\" ({size} bytes) exceeds {MAX_INLINE_EXTRACT_BYTES} byte inline cap; showing best fitting fallback. Call extract(strategy=\"{strat}\") for the full primary.",
                                        strat = primary_strategy
                                    ),
                                            },
                                        });
                                    // Carry truncated all_hits summary for visibility.
                                    if let Some(map) = sub.as_object_mut() {
                                        let summary: Vec<Value> = hits.iter().map(|h| {
                                            let s = serde_json::to_string(h).map(|s| s.len()).unwrap_or(0);
                                            json!({
                                                "strategy": h.get("strategy").cloned().unwrap_or(Value::Null),
                                                "confidence": h.get("confidence").cloned().unwrap_or(Value::Null),
                                                "size_bytes": s,
                                            })
                                        }).collect();
                                        map.insert(
                                            "all_hits_summary".into(),
                                            Value::Array(summary),
                                        );
                                    }
                                    chosen = Some(sub);
                                    break;
                                }
                            }
                        }
                        if let Some(c) = chosen {
                            (Some(c), None)
                        } else {
                            (
                                Some(json!({
                                    "strategy": primary_strategy,
                                    "confidence": primary_confidence,
                                    "data": null,
                                    "truncated": true,
                                    "size_bytes": size,
                                    "hint": format!(
                                        "extract result {size} bytes exceeds {MAX_INLINE_EXTRACT_BYTES} byte inline cap and no smaller hit fit either; call extract() to retrieve full data"
                                    ),
                                })),
                                None,
                            )
                        }
                    } else {
                        (Some(v), None)
                    }
                }
                Err(e) => (None, Some(e.to_string())),
            }
        } else {
            (None, None)
        };

        emit_event(
            "navigate",
            json!({
                "url": final_url,
                "status": status,
                "bytes": bytes,
                "elapsed_ms": nav_start.elapsed().as_millis() as u64,
                "exec_scripts": exec_scripts,
                "shim_mode": self.shim_mode.as_str(),
                "scripts_executed": scripts.as_ref().and_then(|s| s.get("executed")),
                "scripts_interrupted": scripts.as_ref().and_then(|s| s.get("interrupted")),
                "auto_extract_strategy": auto_extract.as_ref().and_then(|e| e.get("strategy")),
                "auto_extract_confidence": auto_extract.as_ref().and_then(|e| e.get("confidence")),
                "auto_extract_truncated": auto_extract.as_ref().and_then(|e| e.get("truncated")),
                "auto_extract_error": auto_extract_error,
                "rate_limit": rate_limit.clone(),
                "browser_route": browser_route.clone(),
            }),
        );

        // network_stores: opportunistic capture of content-bearing
        // fetch/XHR responses (JSON / GraphQL / NDJSON / route data).
        // Navigate result includes a SUMMARY (count + top-K metadata)
        // — full bodies are accessed via the network_stores RPC method
        // to keep the navigate result reasonable in size. Scoped to THIS
        // navigation_id so page A's captures don't leak into page B's
        // summary. (PR #7 review medium.)
        let network_stores = self._fetch.network_store.lock().ok().map(|s| {
            serde_json::to_value(s.summary(5, network_store::NavScope::Only(&nav_id)))
                .unwrap_or(Value::Null)
        });

        // Synthetic outcome derivation. T2 (offline aggregator) needs an
        // outcome stream — "did this navigate produce useful output?" — to
        // fit Bayesian posteriors against. report_outcome RPC exists for
        // drivers that care, but few drivers call it; deriving an outcome
        // from the navigate result itself gives T2 a signal regardless.
        //
        // Fires AFTER policy_trace (and auto_extract / network_stores
        // collection) so all the inputs are populated. Two events:
        //   - outcome_derived (one per navigate): the verdict + reasons +
        //     signals so the heuristic is debuggable.
        //   - outcome_for_decision (one per script_decision in this nav):
        //     pairs the per-decision key (block:<host> / allow:<host>)
        //     with the navigate-level success bit, which is exactly what
        //     T2 reads to update the corresponding posteriors.
        //
        // Pure derivation lives in derive_outcome() (testable, no I/O).
        let scripts_for_outcome = scripts.clone().unwrap_or(Value::Null);
        let extract_for_outcome = auto_extract.clone().unwrap_or(Value::Null);
        let network_for_outcome = network_stores.clone().unwrap_or(Value::Null);
        let challenge_for_outcome = challenge.clone().unwrap_or(Value::Null);
        let mut tool_advice = derive_tool_likelihoods(
            status,
            exec_scripts,
            &blockmap,
            &extract_for_outcome,
            &network_for_outcome,
            &challenge_for_outcome,
            &scripts_for_outcome,
        );
        apply_browser_route_tool_advice(&mut tool_advice, &browser_route);
        let (success, reasons, signals) = derive_outcome(
            status,
            exec_scripts,
            &challenge_for_outcome,
            &blockmap,
            &extract_for_outcome,
            &network_for_outcome,
            &scripts_for_outcome,
        );
        emit_event(
            "outcome_derived",
            json!({
                "schema_version": 1,
                "navigation_id": nav_id,
                "url": final_url,
                "success": success,
                "reasons": reasons,
                "signals": signals,
            }),
        );
        // Per-decision attribution: for every script_decision recorded
        // during this navigate (skip / queued only — fetch_failed isn't a
        // policy choice), emit a paired outcome_for_decision so T2 can
        // bucket success/fail by `block:<host>` or `allow:<host>` keys.
        // Hosts that didn't parse out of the URL drop silently — they
        // can't be bucketed usefully.
        for d in &decisions {
            if let Some(key) = d.decision_key() {
                emit_event(
                    "outcome_for_decision",
                    json!({
                        "schema_version": 1,
                        "navigation_id": nav_id,
                        "decision_key": key,
                        "action": d.action,
                        "success": success,
                    }),
                );
            }
        }

        Ok(json!({
            "navigation_id": nav_id,
            "status": status,
            "url": final_url,
            "bytes": bytes,
            "headers": Value::Object(response_headers),
            "shim_mode": self.shim_mode.as_str(),
            "blockmap": blockmap,
            "challenge": challenge,
            "rate_limit": rate_limit,
            "browser_route": browser_route,
            "scripts": scripts,
            "extract": auto_extract,
            "network_stores": network_stores,
            "tool_confidence": tool_advice.get("confidence").cloned().unwrap_or(Value::Null),
            "tool_margin": tool_advice.get("margin").cloned().unwrap_or(Value::Null),
            "tool_likelihoods": tool_advice.get("tool_likelihoods").cloned().unwrap_or(Value::Null),
            "tool_recommendations": tool_advice.get("tool_recommendations").cloned().unwrap_or(Value::Null),
        }))
    }

    fn blockmap(&self) -> Result<Value> {
        self.eval("__blockmap()")
    }

    fn network_scope_id(&self, nav_id: Option<&str>) -> Option<String> {
        match nav_id {
            Some("all") => None,
            Some(explicit) => Some(explicit.to_string()),
            None => self
                ._fetch
                .current_nav_id
                .lock()
                .ok()
                .and_then(|g| g.clone()),
        }
    }

    fn network_captures(
        &self,
        limit: usize,
        host: Option<&str>,
        nav_id: Option<&str>,
    ) -> (Option<String>, Vec<network_store::NetworkCapture>) {
        let scope_id = self.network_scope_id(nav_id);
        let captures = self
            ._fetch
            .network_store
            .lock()
            .map(|s| {
                let scope = match scope_id.as_deref() {
                    Some(id) => network_store::NavScope::Only(id),
                    None => network_store::NavScope::All,
                };
                s.ranked(limit, host, scope)
            })
            .unwrap_or_default();
        (scope_id, captures)
    }

    fn network_counts(&self) -> Value {
        let current_nav_id = self.network_scope_id(None);
        self._fetch
            .network_store
            .lock()
            .map(|s| {
                let current = current_nav_id
                    .as_deref()
                    .map(|id| s.summary(0, network_store::NavScope::Only(id)).count)
                    .unwrap_or(0);
                let all = s.summary(0, network_store::NavScope::All).count;
                json!({
                    "current_nav_id": current_nav_id,
                    "current_nav_count": current,
                    "all_count": all,
                })
            })
            .unwrap_or_else(|_| {
                json!({
                    "current_nav_id": current_nav_id,
                    "current_nav_count": 0,
                    "all_count": 0,
                })
            })
    }

    fn network_extract(
        &self,
        query: Option<&str>,
        types: Option<&Value>,
        limit: usize,
        host: Option<&str>,
        nav_id: Option<&str>,
    ) -> Result<Value> {
        let limit = limit.clamp(1, 100);
        let capture_limit = limit.saturating_mul(4).clamp(20, 100);
        let (scope_id, captures) = self.network_captures(capture_limit, host, nav_id);
        let terms = network_terms(query.unwrap_or(""));
        let allowed = parse_object_type_filter(types);
        let mut errors = Vec::new();
        let mut objects = Vec::new();

        for capture in &captures {
            match extract_network_objects_from_capture(capture, &terms, limit.saturating_mul(12)) {
                Ok(mut found) => objects.append(&mut found),
                Err(e) => errors.push(json!({
                    "capture_id": capture.capture_id,
                    "url": capture.url,
                    "error": e,
                    "body_truncated": capture.body_truncated,
                })),
            }
        }

        if let Some(allowed) = &allowed {
            objects.retain(|o| network_kind_allowed(&o.kind, allowed));
        }
        objects.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        dedupe_network_objects(&mut objects);
        objects.truncate(limit);
        let values: Vec<Value> = objects
            .iter()
            .enumerate()
            .map(|(idx, obj)| obj.to_value(idx + 1))
            .collect();

        Ok(json!({
            "query": query,
            "nav_id": scope_id,
            "host": host,
            "capture_count": captures.len(),
            "object_count": values.len(),
            "objects": values,
            "errors": errors,
        }))
    }

    fn page_model(&self, goal: Option<&str>, types: Option<&Value>, limit: u32) -> Result<Value> {
        let types = match types {
            Some(v) if v.is_array() => v.clone(),
            _ => Value::Null,
        };
        let opts = json!({
            "goal": goal,
            "types": types,
            "limit": limit,
        });
        let mut model = self.eval(&format!("__pageModel({})", serde_json::to_string(&opts)?))?;
        self.attach_page_model_network_objects(&mut model, goal, Some(&types), limit as usize);
        self.attach_page_model_limitations(&mut model);
        Ok(model)
    }

    fn route_discover(&self, goal: Option<&str>, limit: u32) -> Result<Value> {
        let opts = json!({
            "goal": goal,
            "limit": limit,
        });
        self.eval(&format!(
            "__routeDiscover({})",
            serde_json::to_string(&opts)?
        ))
    }

    async fn discover(&mut self, opts: DiscoverOptions<'_>) -> Result<Value> {
        let DiscoverOptions {
            url,
            goal,
            exec_scripts,
            limit,
            same_origin,
            include_network,
            debug,
        } = opts;
        let nav = if let Some(u) = url {
            Some(self.navigate(u, exec_scripts).await?)
        } else {
            None
        };
        let current_url = nav
            .as_ref()
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| self.last_url.clone())
            .unwrap_or_default();
        let title = nav
            .as_ref()
            .and_then(|v| v.get("blockmap"))
            .and_then(|v| v.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = nav
            .as_ref()
            .and_then(|v| v.get("status"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let navigation_id = nav
            .as_ref()
            .and_then(|v| v.get("navigation_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| self.network_scope_id(None));
        let static_route_keys = if exec_scripts {
            self.last_body
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .map(|body| static_dom_route_keys_from_body(&body, &current_url))
                .unwrap_or_default()
        } else {
            HashSet::new()
        };

        let mut discovery_errors = Vec::new();
        let route_model = match self.route_discover(goal, limit) {
            Ok(v) => v,
            Err(e) => {
                discovery_errors.push(json!({
                    "tool": "route_discover",
                    "error": e.to_string(),
                    "partial": true,
                }));
                json!({
                    "url": current_url,
                    "title": title,
                    "goal": goal,
                    "routes": [],
                    "forms": [],
                    "inferred_urls": [],
                    "summary": {"routes": 0, "forms": 0, "inferred_urls": 0},
                })
            }
        };
        let mut routes = Vec::new();
        let mut route_index: HashMap<String, usize> = HashMap::new();
        let fallback_dom_source = if exec_scripts { "js_dom" } else { "static_dom" };

        if let Some(items) = route_model.get("routes").and_then(|v| v.as_array()) {
            for item in items {
                let source = item
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(discovery_route_key)
                    .filter(|key| static_route_keys.contains(key))
                    .map(|_| "static_dom")
                    .unwrap_or(fallback_dom_source);
                upsert_discovery_route(
                    &mut routes,
                    &mut route_index,
                    item,
                    source,
                    "document_route",
                    DISCOVERY_DOM_CONFIDENCE,
                    &current_url,
                );
            }
        }
        if let Some(items) = route_model.get("inferred_urls").and_then(|v| v.as_array()) {
            for item in items {
                let source = if item.get("kind").and_then(|v| v.as_str()) == Some("form_query_url")
                {
                    "form_inference"
                } else {
                    "route_inference"
                };
                upsert_discovery_route(
                    &mut routes,
                    &mut route_index,
                    item,
                    source,
                    "inferred_route",
                    DISCOVERY_INFERRED_CONFIDENCE,
                    &current_url,
                );
            }
        }

        let forms = route_model
            .get("forms")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));

        let mut network_sources = Vec::new();
        let mut api_endpoints = Vec::new();
        let mut network_extract_result = Value::Null;
        if include_network {
            match self.network_extract(goal, None, limit as usize, None, None) {
                Ok(network) => {
                    if let Some(items) = network.get("objects").and_then(|v| v.as_array()) {
                        for item in items {
                            upsert_discovery_route(
                                &mut routes,
                                &mut route_index,
                                item,
                                "network_json",
                                "network_route",
                                DISCOVERY_NETWORK_OBJECT_CONFIDENCE,
                                &current_url,
                            );
                        }
                    }
                    network_extract_result = network;
                }
                Err(e) => {
                    discovery_errors.push(json!({
                        "tool": "network_extract",
                        "error": e.to_string(),
                        "partial": true,
                    }));
                }
            }
            let (_, captures) = self.network_captures(10, None, None);
            for capture in captures {
                let kind = serde_json::to_value(capture.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| format!("{:?}", capture.kind));
                if is_api_like_url(&capture.url) {
                    api_endpoints.push(json!({
                        "url": capture.url,
                        "method": capture.method,
                        "source": "network_capture",
                        "confidence": DISCOVERY_CAPTURE_API_CONFIDENCE,
                        "provenance": [{
                            "source": "network",
                            "capture_id": capture.capture_id,
                            "kind": kind,
                            "reason": "captured content-bearing API response"
                        }]
                    }));
                }
                for item in raw_network_route_candidates(&capture, limit as usize) {
                    let source = item
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .filter(|k| *k == "api_endpoint")
                        .map(|_| "network_api")
                        .unwrap_or("network_json");
                    if source == "network_api" {
                        api_endpoints.push(json!({
                            "url": item.get("url").cloned().unwrap_or(Value::Null),
                            "method": capture.method,
                            "source": "network_json",
                            "confidence": item.get("confidence").cloned().unwrap_or(json!(DISCOVERY_RAW_NETWORK_API_CONFIDENCE)),
                            "provenance": item.get("provenance").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
                        }));
                    }
                    upsert_discovery_route(
                        &mut routes,
                        &mut route_index,
                        &item,
                        source,
                        "network_route",
                        DISCOVERY_NETWORK_ROUTE_CONFIDENCE,
                        &current_url,
                    );
                }
                network_sources.push(json!({
                    "capture_id": capture.capture_id,
                    "url": capture.url,
                    "method": capture.method,
                    "status": capture.status,
                    "kind": kind,
                    "score": capture.score,
                    "body_bytes": capture.body_bytes,
                    "body_truncated": capture.body_truncated,
                    "navigation_id": capture.navigation_id,
                }));
            }
        }

        if same_origin {
            routes.retain(|route| {
                route
                    .get("page_owned")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            });
        }
        routes.sort_by(|a, b| {
            let bs = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let as_ = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            bs.partial_cmp(&as_).unwrap_or(std::cmp::Ordering::Equal)
        });
        routes.truncate(limit as usize);

        let form_count = forms.as_array().map(|a| a.len()).unwrap_or(0);
        let mut escalations = discovery_escalations(
            &self.last_challenge,
            &self.last_rate_limit,
            &self.last_browser_route,
            routes.len(),
            form_count,
        );
        if same_origin && routes.is_empty() && form_count == 0 && escalations.is_empty() {
            escalations.push(json!({
                "reason": "no_routes_found",
                "recommended_tool": if exec_scripts { "chrome" } else { "discover_exec_scripts" },
                "confidence": if exec_scripts { 0.66 } else { 0.58 },
                "evidence": ["discover.same_origin_filter"],
                "hint": if exec_scripts {
                    "No same-origin routes or forms were discovered after script execution; use Chrome if the task requires hidden UI state."
                } else {
                    "No same-origin routes or forms were discovered on the static pass; retry discover with exec_scripts:true before escalating."
                },
            }));
        }

        let navigate_summary = summarize_discover_navigate(nav.as_ref());
        let route_discover_summary = route_model
            .get("summary")
            .cloned()
            .unwrap_or_else(|| json!({"routes": 0, "forms": 0, "inferred_urls": 0}));
        let network_extract_summary = if network_extract_result.is_null() {
            Value::Null
        } else {
            json!({
                "nav_id": network_extract_result.get("nav_id").cloned().unwrap_or(Value::Null),
                "capture_count": network_extract_result.get("capture_count").and_then(|v| v.as_u64()).unwrap_or(0),
                "object_count": network_extract_result.get("object_count").and_then(|v| v.as_u64()).unwrap_or(0),
                "errors": network_extract_result.get("errors").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
            })
        };

        let mut result = json!({
            "url": current_url,
            "title": title,
            "status": status,
            "navigation_id": navigation_id,
            "goal": goal,
            "debug": debug,
            "summary": {
                "routes": routes.len(),
                "forms": form_count,
                "api_endpoints": api_endpoints.len(),
                "network_sources": network_sources.len(),
                "escalations": escalations.len(),
            },
            "routes": routes,
            "forms": forms,
            "api_endpoints": api_endpoints,
            "network_sources": network_sources,
            "navigate_summary": navigate_summary,
            "route_discover_summary": route_discover_summary,
            "network_extract_summary": network_extract_summary,
            "escalations": escalations,
            "errors": discovery_errors,
        });
        if debug && let Some(map) = result.as_object_mut() {
            map.insert("navigate".to_string(), nav.unwrap_or(Value::Null));
            map.insert("route_discover".to_string(), route_model);
            map.insert("network_extract".to_string(), network_extract_result);
        }
        Ok(result)
    }

    fn attach_page_model_network_objects(
        &self,
        model: &mut Value,
        goal: Option<&str>,
        types: Option<&Value>,
        limit: usize,
    ) {
        let Ok(network) = self.network_extract(goal, types, limit.clamp(10, 30), None, None) else {
            return;
        };
        let network_objects = network
            .get("objects")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let network_count = network_objects.as_array().map(|a| a.len()).unwrap_or(0);
        let capture_count = network
            .get("capture_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let Some(map) = model.as_object_mut() else {
            return;
        };
        map.insert("network_objects".to_string(), network_objects);
        if let Some(summary) = map.get_mut("summary").and_then(|v| v.as_object_mut()) {
            summary.insert("network_objects".to_string(), json!(network_count));
            summary.insert("network_captures".to_string(), json!(capture_count));
        }
    }

    fn attach_page_model_limitations(&self, model: &mut Value) {
        let Some(map) = model.as_object_mut() else {
            return;
        };
        let mut strict = Vec::new();
        if let Some(challenge) = &self.last_challenge {
            strict.push(json!({
                "kind": "limitation",
                "reason": "challenge_required",
                "confidence": challenge.get("confidence").cloned().unwrap_or(json!(0.9)),
                "provider": challenge.get("provider").cloned().unwrap_or(Value::Null),
                "evidence": ["navigate.challenge"],
                "hint": challenge.get("hint").cloned().unwrap_or_else(|| json!("The page returned a challenge before useful content could be modeled.")),
            }));
        } else if let Some(rate_limit) = &self.last_rate_limit {
            strict.push(json!({
                "kind": "limitation",
                "reason": "rate_limited",
                "confidence": 0.9,
                "status": rate_limit.get("status").cloned().unwrap_or(Value::Null),
                "retry_after": rate_limit.get("retry_after").cloned().unwrap_or(Value::Null),
                "retry_after_seconds": rate_limit.get("retry_after_seconds").cloned().unwrap_or(Value::Null),
                "evidence": ["navigate.rate_limit"],
                "hint": rate_limit.get("hint").cloned().unwrap_or_else(|| json!("Back off and retry later.")),
            }));
        } else if let Some(route) = &self.last_browser_route {
            strict.push(json!({
                "kind": "limitation",
                "reason": route.get("reason").cloned().unwrap_or_else(|| json!("unbrowser_limit")),
                "confidence": route.get("confidence").cloned().unwrap_or(json!(0.7)),
                "evidence": route.get("evidence").cloned().unwrap_or_else(|| json!(["navigate.browser_route"])),
                "hint": "Strict unbrowser could not find a non-rendered content/action surface for this page.",
            }));
        }

        if strict.is_empty() {
            return;
        }
        let limitations = map
            .entry("limitations".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let mut limitation_count = None;
        if let Some(arr) = limitations.as_array_mut() {
            arr.extend(strict);
            limitation_count = Some(arr.len());
        }
        if let Some(count) = limitation_count
            && let Some(summary) = map.get_mut("summary").and_then(|v| v.as_object_mut())
        {
            summary.insert("limitations".to_string(), json!(count));
        }
    }

    // Auto-strategy extraction. Tries JSON-LD → __NEXT_DATA__ → Nuxt →
    // OpenGraph/meta → microdata → text_main fallback, returns the
    // highest-confidence hit. Pass strategy="json_ld" etc. to force one.
    fn extract(&self, strategy: Option<&str>) -> Result<Value> {
        let opts = match strategy {
            Some(s) => format!("{{ strategy: {} }}", serde_json::to_string(s)?),
            None => "{}".to_string(),
        };
        self.eval(&format!("__extract({opts})"))
    }

    // Pull a <table> into {headers, rows, row_count}. Right tool for
    // pricing/specs/listings tables — saves the agent from writing a
    // querySelectorAll('tr') + per-cell mapping eval.
    fn extract_table(&self, selector: &str) -> Result<Value> {
        let sel_lit = serde_json::to_string(selector)?;
        self.eval(&format!("__extractTable({sel_lit})"))
    }

    // Pull a repeated card pattern into [{...}, ...]. `fields` maps field
    // name -> CSS sub-selector (with optional " @attr" suffix for an
    // attribute extraction). Right tool for HN-style lists, search results,
    // product grids — collapses per-site eval boilerplate to one call.
    fn extract_list(&self, item: &str, fields: &Value, limit: u32) -> Result<Value> {
        let item_lit = serde_json::to_string(item)?;
        let fields_lit = serde_json::to_string(fields)?;
        self.eval(&format!("__extractList({item_lit}, {fields_lit}, {limit})"))
    }

    // Auto-detect repeated cards/articles/products/courses and normalize to
    // {title, url, snippet, price, condition, availability, meta, image_alt,
    // score}. A selector can scope or override detection when the page has a
    // known card container.
    fn extract_cards(
        &self,
        selector: Option<&str>,
        limit: u32,
        kind: Option<&str>,
    ) -> Result<Value> {
        let selector_lit = match selector {
            Some(s) => serde_json::to_string(s)?,
            None => "null".to_string(),
        };
        let kind_lit = match kind {
            Some(k) => serde_json::to_string(k)?,
            None => "null".to_string(),
        };
        self.eval(&format!(
            "__extractCards({selector_lit}, {limit}, {kind_lit})"
        ))
    }

    fn text_clean(&self, selector: Option<&str>, max_chars: Option<u32>) -> Result<Value> {
        let opts = json!({ "selector": selector, "max_chars": max_chars });
        self.eval(&format!("__textClean({})", serde_json::to_string(&opts)?))
    }

    fn find_text(
        &self,
        text: &str,
        selector: Option<&str>,
        exact: bool,
        limit: u32,
        context_chars: u32,
    ) -> Result<Value> {
        let opts = json!({
            "text": text,
            "selector": selector,
            "exact": exact,
            "limit": limit,
            "context_chars": context_chars,
        });
        self.eval(&format!("__findText({})", serde_json::to_string(&opts)?))
    }

    fn text_around(
        &self,
        ref_: Option<&str>,
        text: Option<&str>,
        selector: Option<&str>,
        context_chars: u32,
    ) -> Result<Value> {
        let opts = json!({
            "ref": ref_,
            "text": text,
            "selector": selector,
            "context_chars": context_chars,
        });
        self.eval(&format!("__textAround({})", serde_json::to_string(&opts)?))
    }

    // Drain the JS event loop: alternately runs queued microtasks (Promise
    // resolutions, queueMicrotask, etc.) and fires expired setTimeout/Interval
    // callbacks, sleeping to the next deadline when only timers remain.
    // Returns when the queue is fully empty OR `max_ms` elapses OR `max_iters`
    // iterations complete (whichever first).
    //
    // Iteration model:
    //   1. Drain all pending microtasks (via Runtime::execute_pending_job).
    //   2. Pump expired timers (JS-side __pumpTimers).
    //   3. If neither produced work and timers are pending, sleep to the
    //      earliest deadline (capped by remaining max_ms).
    //   4. If nothing is pending at all, exit.
    async fn settle(&self, max_ms: u64, max_iters: u32) -> Result<Value> {
        let start = std::time::Instant::now();
        let mut iters: u32 = 0;
        let mut total_microtasks: u64 = 0;
        let mut total_timers: u64 = 0;
        let mut total_fetches: u64 = 0;
        // Polling-detection: when a setInterval keeps firing but its
        // callback produces no microtasks and no new fetches, that's
        // live-polling (price ticker, animation frame, debounce timer).
        // It will never go idle. Count consecutive iters of "timers fired
        // but nothing else happened" — bail after 3 in a row to save the
        // remaining settle budget for pages that legitimately need it.
        // Polymarket's after_dcl/after_load both burned the full 2s+1.5s
        // budget firing live-price intervals before this check existed.
        let mut polling_iters: u32 = 0;

        // Why settle exited. "idle" is the success case (all queues drained);
        // the others mean we hit a budget. Drivers can use this to pick a
        // failure mode: budget_exhausted suggests bumping max_ms or skipping
        // more scripts; max_iters suggests an infinite-microtask-loop
        // pattern (very rare in practice, but happens with libraries that
        // queueMicrotask in a loop). See PR review feedback / SPA proposal #6.
        // Filled in at every break path — clippy enforces no implicit default.
        let reason: &'static str;

        loop {
            if iters >= max_iters {
                reason = "max_iters";
                break;
            }
            let elapsed_ms = start.elapsed().as_millis() as u64;
            if elapsed_ms >= max_ms {
                reason = "budget_exhausted";
                break;
            }

            // 1. Drain microtasks. The inner loop honors max_ms — without
            // this, a MutationObserver→mutate→MutationObserver cascade (common
            // on hydrating Next.js / React SPAs like Vercel, Tailwind, Next.js
            // homepage) can run thousands of microtasks in a single outer
            // iter and blow past max_ms by 10–20×. Cap at 2000 microtasks per
            // pass as defense-in-depth (a normal page's burst is well under
            // a few hundred; 2k is 10× headroom).
            let mut mt_this_iter: u64 = 0;
            loop {
                let had_more = self
                    .js_rt
                    .execute_pending_job()
                    .map_err(|e| anyhow!("microtask exception: {e:?}"))?;
                if !had_more {
                    break;
                }
                mt_this_iter += 1;
                if mt_this_iter > 2_000 {
                    break;
                }
                if start.elapsed().as_millis() as u64 >= max_ms {
                    break;
                }
            }
            total_microtasks += mt_this_iter;

            // 2. Pump expired timers.
            let fired = self.eval("__pumpTimers()")?.as_u64().unwrap_or(0);
            total_timers += fired;

            // 3. Drain fetch responses (resolves pending Promises JS-side).
            // Note: pending_fetches covers BOTH JS-issued fetch()/XHR AND
            // dynamic-script loads from PR #6's __maybeHandleDynamicScript
            // (it routes through fetch). MutationObserver / IntersectionObserver
            // / ResizeObserver callbacks (PR #8) fire via queueMicrotask, so
            // they're covered by the microtask drain in step 1. We don't need
            // separate pending counters for those.
            let resolved = self.eval("__pollFetches()")?.as_u64().unwrap_or(0);
            total_fetches += resolved;

            // 4. Decide whether to keep going.
            let pending_timers = self.eval("__pendingTimers()")?.as_u64().unwrap_or(0);
            let pending_fetches = self.eval("__pendingFetches()")?.as_u64().unwrap_or(0);
            let microtasks_pending = self.js_rt.is_job_pending();

            if pending_timers == 0 && pending_fetches == 0 && !microtasks_pending {
                reason = "idle";
                break; // queue fully empty — the success case
            }

            // Polling detection. The streak counts iters where no useful
            // work happened (no microtasks ran, no fetches resolved) but
            // timers either fired without consequence OR are pending. It
            // resets only when something productive happens.
            //
            // Note that the loop alternates "timer fires" iters and "sleep
            // until next deadline" iters, so we can't gate this on
            // `fired > 0` — that would reset every sleep iter and never
            // accumulate. After 3 streak iters with no useful work and
            // no pending fetches/microtasks, assume the page is stuck on
            // a poll loop (live price, animation frame, debounce timer)
            // and exit with the work so far.
            let useful_work = mt_this_iter > 0 || resolved > 0;
            if useful_work {
                polling_iters = 0;
            } else if !microtasks_pending && pending_fetches == 0 {
                polling_iters += 1;
                if polling_iters >= 3 {
                    reason = "polling_detected";
                    break;
                }
            }

            let did_work_this_iter = mt_this_iter > 0 || fired > 0 || resolved > 0;
            if !did_work_this_iter && !microtasks_pending && pending_fetches > 0 {
                // Only fetches in flight — sleep briefly waiting for the worker
                // thread to push results.
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            } else if !did_work_this_iter && !microtasks_pending && pending_timers > 0 {
                // Only timers are pending and none expired this iter — sleep
                // to the earliest deadline (capped by remaining time budget).
                let deadline = self.eval("__nextTimerDeadline()")?.as_f64();
                if let Some(deadline_ms) = deadline {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as f64)
                        .unwrap_or(0.0);
                    let remaining_budget = (max_ms.saturating_sub(elapsed_ms)) as f64;
                    let wait_ms = (deadline_ms - now_ms).max(0.0).min(remaining_budget);
                    if wait_ms > 0.0 {
                        tokio::time::sleep(std::time::Duration::from_millis(wait_ms as u64)).await;
                    }
                }
            }

            iters += 1;
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(json!({
            "iters": iters,
            "elapsed_ms": elapsed_ms,
            "microtasks_run": total_microtasks,
            "timers_fired": total_timers,
            "fetches_resolved": total_fetches,
            "pending_timers": self.eval("__pendingTimers()")?.as_u64().unwrap_or(0),
            "pending_fetches": self.eval("__pendingFetches()")?.as_u64().unwrap_or(0),
            "pending_microtasks": self.js_rt.is_job_pending(),
            // Why settle exited. One of:
            //   "idle"               — all queues drained (success)
            //   "budget_exhausted"   — wall-clock max_ms hit
            //   "max_iters"          — iteration cap hit (pathological microtask loop)
            //   "polling_detected"   — timers kept firing without producing
            //                          microtasks/fetches (live-poll setInterval);
            //                          page is content-stable, just poll-perpetual
            // Drivers should use this to choose a recovery action.
            "reason": reason,
            // Back-compat: timed_out=true matches either budget_exhausted or
            // max_iters. New consumers should prefer `reason`. Kept so existing
            // drivers (and the `policy_trace` consumers) don't break. Drop in
            // a follow-up once everyone's migrated.
            "timed_out": reason != "idle",
        }))
    }

    fn seed_dom(&self, tree: &Value) -> Result<()> {
        let tree_str = serde_json::to_string(tree)?;
        // Embed the JSON string as a JS string literal (double-encode to escape safely).
        let js_literal = serde_json::to_string(&tree_str)?;
        let code = format!("__seedDOM(JSON.parse({js_literal}))");
        self.js_ctx.with(|ctx| -> Result<()> {
            ctx.eval::<(), _>(code)
                .map_err(|e| anyhow!("seed dom: {e}"))?;
            Ok(())
        })
    }

    fn query(&self, selector: &str) -> Result<Value> {
        let sel_lit = serde_json::to_string(selector)?;
        let code = format!(
            "(function(){{ \
                var els = document.querySelectorAll({sel_lit}); \
                return els.map(function(e){{ \
                    var text = (e.textContent || '').trim(); \
                    return {{ \
                        ref: 'e:' + e._id, \
                        tag: e.tagName.toLowerCase(), \
                        attrs: e._attributes, \
                        text: text.slice(0, 200), \
                        text_chars: text.length, \
                        text_truncated: text.length > 200 \
                    }}; \
                }}); \
            }})()"
        );
        self.eval(&code)
    }

    fn query_debug(&self, selector: &str, limit: u32) -> Result<Value> {
        let sel_lit = serde_json::to_string(selector)?;
        let code = format!(
            r#"(function(){{
                var selector = {sel_lit};
                var limit = Math.max(0, Math.min({limit}, 50));
                function clean(s) {{ return String(s || '').replace(/\s+/g, ' ').trim(); }}
                function q(sel) {{ try {{ return document.querySelectorAll(sel) || []; }} catch (e) {{ return []; }} }}
                function sample(el) {{
                    var text = clean(el.textContent);
                    return {{
                        ref: 'e:' + el._id,
                        tag: (el.tagName || '').toLowerCase(),
                        attrs: el._attributes || {{}},
                        text: text.slice(0, 200),
                        text_chars: text.length,
                        text_truncated: text.length > 200
                    }};
                }}
                function countMapTop(map, max) {{
                    var rows = [];
                    for (var k in map) {{ if (Object.prototype.hasOwnProperty.call(map, k)) rows.push({{ value: k, count: map[k] }}); }}
                    rows.sort(function(a, b) {{ return b.count - a.count || a.value.localeCompare(b.value); }});
                    return rows.slice(0, max);
                }}
                var matches = [];
                var valid = true;
                var error = null;
                try {{
                    matches = document.querySelectorAll(selector) || [];
                }} catch (e) {{
                    valid = false;
                    error = String((e && e.message) || e);
                }}

                var all = q('*');
                var scripts = q('script');
                var links = q('a[href]');
                var images = q('img');
                var tables = q('table');
                var inputs = q('input, textarea, select');
                var buttons = q('button, input[type=button], input[type=submit], [role=button]');
                var headings = q('h1, h2, h3, h4, h5, h6');
                var jsonScripts = 0;
                for (var js = 0; js < scripts.length; js++) {{
                    var t = String(scripts[js].getAttribute('type') || '').toLowerCase();
                    if (t.indexOf('json') !== -1 || t.indexOf('x-magento') !== -1 || t.indexOf('x-shopify') !== -1 || t.indexOf('x-component') !== -1) jsonScripts++;
                }}
                var bodyTextChars = clean(document.body && document.body.textContent).length;

                var topTags = {{}};
                var topClasses = {{}};
                var dataAttrs = {{}};
                var ids = [];
                for (var i = 0; i < all.length; i++) {{
                    var el = all[i];
                    var tag = (el.tagName || '').toLowerCase();
                    if (tag) topTags[tag] = (topTags[tag] || 0) + 1;
                    var cls = String(el.getAttribute('class') || '').split(/\s+/);
                    for (var c = 0; c < cls.length; c++) {{
                        if (cls[c]) topClasses[cls[c]] = (topClasses[cls[c]] || 0) + 1;
                    }}
                    var id = el.getAttribute('id');
                    if (id && ids.length < 25) ids.push(id);
                    var attrs = el._attributes || {{}};
                    for (var name in attrs) {{
                        if (!Object.prototype.hasOwnProperty.call(attrs, name)) continue;
                        if (name.indexOf('data-') === 0 || name === 'role' || name === 'aria-label') dataAttrs[name] = (dataAttrs[name] || 0) + 1;
                    }}
                }}

                var samples = [];
                for (var s = 0; s < matches.length && s < limit; s++) samples.push(sample(matches[s]));
                var hints = [];
                if (!valid) {{
                    hints.push('selector_error: selector could not be evaluated by unbrowser CSS support');
                }} else if (matches.length === 0) {{
                    hints.push('selector_miss: valid selector, no matching elements in the current DOM');
                    if (all.length < 30 || (bodyTextChars < 4000 && scripts.length >= 10 && links.length < 30)) hints.push('thin_shell: DOM has little visible content; page may require browser rendering');
                    if (jsonScripts > 0) hints.push('embedded_json: JSON-bearing scripts exist; try extract() or eval() against script data');
                    if (links.length > 0) hints.push('try query_text/find_text for visible labels, or inspect selector_hints');
                }} else if (matches.length > limit) {{
                    hints.push('truncated_samples: matched_count exceeds returned sample limit');
                }}

                return {{
                    selector: selector,
                    valid_selector: valid,
                    error: error,
                    matched_count: matches.length,
                    sample_count: samples.length,
                    samples: samples,
                    dom_summary: {{
                        total_elements: all.length,
                        body_text_chars: bodyTextChars,
                        links: links.length,
                        buttons: buttons.length,
                        inputs: inputs.length,
                        images: images.length,
                        tables: tables.length,
                        headings: headings.length,
                        scripts: scripts.length,
                        json_scripts: jsonScripts
                    }},
                    selector_hints: {{
                        top_tags: countMapTop(topTags, 12),
                        top_classes: countMapTop(topClasses, 20),
                        data_attrs: countMapTop(dataAttrs, 20),
                        ids: ids
                    }},
                    hints: hints
                }};
            }})()"#
        );
        self.eval(&code)
    }

    fn text(&self, selector: &str) -> Result<Value> {
        let sel_lit = serde_json::to_string(selector)?;
        let code = format!(
            "(function(){{ \
                var el = document.querySelector({sel_lit}); \
                return el ? (el.textContent || '').trim() : null; \
            }})()"
        );
        self.eval(&code)
    }

    // Find elements by visible text content, skipping chrome (header/nav/
    // footer/aside/script/style). Returns the smallest/deepest element
    // whose textContent matches the needle. Anchor-promotion: if the deepest
    // match is a <span>/<strong>/etc. whose direct parent is <a>, the anchor
    // is returned instead (so click() targets the actionable element).
    //
    // Right tool for sites where CSS selectors are unstable (React-rendered
    // pages with hashed class names) but the visible text is reliable.
    fn query_text(
        &self,
        text: &str,
        selector: Option<&str>,
        exact: bool,
        limit: u32,
    ) -> Result<Value> {
        let text_lit = serde_json::to_string(text)?;
        let sel_lit = match selector {
            Some(s) => serde_json::to_string(s)?,
            None => "null".to_string(),
        };
        let code = format!(
            r#"(function(){{
                var needle = {text_lit};
                var sel = {sel_lit};
                var exact = {exact};
                var limit = {limit};
                var lowerNeedle = needle.toLowerCase();
                function clean(s) {{ return (s || '').replace(/\s+/g, ' ').trim(); }}
                function isChromeTag(t) {{
                    return t === 'header' || t === 'nav' || t === 'footer' ||
                           t === 'aside' || t === 'script' || t === 'style' ||
                           t === 'noscript';
                }}
                // Pre-filter (descent gate): always substring — we need to
                // recurse if any descendant might match, regardless of mode.
                function contains(t) {{
                    return clean(t).toLowerCase().indexOf(lowerNeedle) !== -1;
                }}
                // Final match test (decides whether to push this node):
                // exact requires equality, otherwise substring is enough.
                function isMatch(t) {{
                    var c = clean(t);
                    return exact ? (c === needle) : (c.toLowerCase().indexOf(lowerNeedle) !== -1);
                }}
                var hits = [];
                function visit(node) {{
                    if (hits.length >= limit) return;
                    if (!node || node.nodeType !== 1) return;
                    var tag = node.tagName.toLowerCase();
                    if (isChromeTag(tag)) return;
                    var text = node.textContent || '';
                    if (!contains(text)) return;
                    var beforeCount = hits.length;
                    for (var i = 0; i < node.childNodes.length; i++) {{
                        visit(node.childNodes[i]);
                        if (hits.length >= limit) return;
                    }}
                    if (hits.length === beforeCount && isMatch(text)) {{
                        var target = node;
                        if (node.parentNode && node.parentNode.tagName === 'A' &&
                            ['SPAN','STRONG','EM','B','I','SMALL','MARK'].indexOf(node.tagName) !== -1) {{
                            target = node.parentNode;
                        }}
                        hits.push(target);
                    }}
                }}
                var roots;
                if (sel) {{
                    var nodeList = document.querySelectorAll(sel);
                    roots = [];
                    for (var i = 0; i < nodeList.length; i++) roots.push(nodeList[i]);
                }} else {{
                    roots = [document.body];
                }}
                for (var i = 0; i < roots.length; i++) visit(roots[i]);
                return hits.map(function(el) {{
                    return {{
                        ref: 'e:' + el._id,
                        tag: el.tagName.toLowerCase(),
                        attrs: el._attributes,
                        text: clean(el.textContent).slice(0, 200),
                    }};
                }});
            }})()"#
        );
        self.eval(&code)
    }

    // Returns the textContent of the page's main content area, excluding chrome
    // (header, nav, footer, aside, script, style) — recursively, so even
    // chrome nested INSIDE <main> (e.g. Wikipedia's table-of-contents <nav>)
    // is skipped.
    //
    // Strategy:
    //  1. <main> or [role=main] if present (walk inside, skip chrome)
    //  2. exactly one <article>
    //  3. fallback: the whole body with chrome subtrees stripped
    fn text_main(&self) -> Result<Value> {
        let code = r#"(function(){
            function clean(s){ return (s || '').replace(/\s+/g, ' ').trim(); }
            // Walk subtree, concatenate text, skipping chrome tags.
            function nonChromeText(root){
                var out = [];
                (function walk(node){
                    if (!node) return;
                    if (node.nodeType === 3) {
                        out.push(node.textContent);
                        return;
                    }
                    if (node.nodeType !== 1) return;
                    var t = (node.tagName || '').toLowerCase();
                    if (t === 'script' || t === 'style' ||
                        t === 'header' || t === 'nav' ||
                        t === 'footer' || t === 'aside' ||
                        t === 'noscript') return;
                    for (var i = 0; i < node.childNodes.length; i++) walk(node.childNodes[i]);
                })(root);
                return clean(out.join(' '));
            }

            var main = document.querySelector('main, [role="main"]');
            if (main) {
                var t = nonChromeText(main);
                if (t.length > 0) return t;
            }
            var articles = document.querySelectorAll('article');
            if (articles.length === 1) {
                var t = nonChromeText(articles[0]);
                if (t.length > 0) return t;
            }
            return nonChromeText(document.body);
        })()"#;
        self.eval(code)
    }

    fn activation_target_by_text(&self, text: &str) -> Result<Option<Value>> {
        let needle = serde_json::to_string(text)?;
        let code = format!(
            r#"(function(needle) {{
                function clean(s) {{ return String(s || '').replace(/\s+/g, ' ').trim(); }}
                function attr(el, name) {{ return el && el.getAttribute ? el.getAttribute(name) : null; }}
                function label(el) {{
                    var tag = (el.tagName || '').toLowerCase();
                    var parts = [el.textContent, attr(el, 'aria-label'), attr(el, 'title')];
                    if (tag === 'input' || tag === 'button') parts.push(el.value, attr(el, 'value'), attr(el, 'placeholder'));
                    return clean(parts.filter(Boolean).join(' '));
                }}
                var q = clean(needle).toLowerCase();
                if (!q) return null;
                var nodes = document.querySelectorAll('a[href], button, input[type="button"], input[type="submit"], input[type="image"], [role="button"], [role="link"], summary, label');
                var hits = [];
                for (var i = 0; i < nodes.length; i++) {{
                    var el = nodes[i];
                    var text = label(el);
                    if (!text || text.toLowerCase().indexOf(q) === -1) continue;
                    var tag = (el.tagName || '').toLowerCase();
                    var score = 1;
                    if (text.toLowerCase() === q) score += 4;
                    if (tag === 'button' || tag === 'a') score += 3;
                    if (attr(el, 'role') === 'button' || attr(el, 'role') === 'link') score += 2;
                    hits.push({{ el: el, score: score, text: text }});
                }}
                hits.sort(function(a, b) {{ return b.score - a.score; }});
                if (!hits.length) return null;
                var best = hits[0].el;
                return {{
                    ref: 'e:' + best._id,
                    tag: (best.tagName || '').toLowerCase(),
                    attrs: best._attributes || {{}},
                    text: hits[0].text.slice(0, 200),
                    source: 'text'
                }};
            }})({needle})"#
        );
        let value = self.eval(&code)?;
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }

    fn activation_snapshot(&self) -> Result<Value> {
        let blockmap = self.blockmap().unwrap_or(Value::Null);
        let blockmap_summary = summarize_blockmap_for_activation(&blockmap);
        let page_model_summary = self
            .page_model(None, None, 20)
            .ok()
            .and_then(|v| v.get("summary").cloned())
            .unwrap_or(Value::Null);
        let text = self
            .text_clean(None, Some(8000))
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let text_hash = hash_string(&text);
        let network = self.network_counts();
        let url = self.last_url.clone();
        let dom_hash = hash_value(&json!({
            "url": url,
            "blockmap": blockmap_summary.clone(),
            "text_hash": text_hash,
        }));
        Ok(json!({
            "url": url,
            "title": blockmap.get("title").cloned().unwrap_or(Value::Null),
            "blockmap": blockmap_summary,
            "page_model": page_model_summary,
            "network": network,
            "text_hash": text_hash,
            "dom_hash": dom_hash,
        }))
    }

    async fn activate(&mut self, ref_: Option<&str>, text: Option<&str>) -> Result<Value> {
        if ref_.is_none() && text.is_none() {
            return Err(anyhow!("missing 'ref' or 'text'"));
        }
        let before = self.activation_snapshot()?;
        let target = if let Some(r) = ref_ {
            json!({ "ref": r, "source": "ref" })
        } else if let Some(t) = text {
            match self.activation_target_by_text(t)? {
                Some(v) => v,
                None => {
                    return Ok(json!({
                        "ok": false,
                        "classification": "unsupported",
                        "reason": "no_actionable_text_match",
                        "requested": { "ref": ref_, "text": text },
                        "before": before,
                        "after": before,
                    }));
                }
            }
        } else {
            unreachable!();
        };
        let target_ref = target
            .get("ref")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("activation target has no ref"))?
            .to_string();

        let click_result = self.click(&target_ref).await?;
        let settle_result = self
            .settle(1500, 50)
            .await
            .unwrap_or_else(|e| json!({ "error": e.to_string() }));
        let after = self.activation_snapshot()?;
        let click_ok = click_result
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let before_url = before.get("url").and_then(|v| v.as_str());
        let after_url = after.get("url").and_then(|v| v.as_str());
        let before_hash = before.get("dom_hash").and_then(|v| v.as_str());
        let after_hash = after.get("dom_hash").and_then(|v| v.as_str());
        let before_network = before
            .get("network")
            .and_then(|v| v.get("all_count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let after_network = after
            .get("network")
            .and_then(|v| v.get("all_count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let url_changed = before_url != after_url;
        let dom_changed = before_hash != after_hash;
        let network_changed = after_network > before_network;
        let network_delta = after_network.saturating_sub(before_network);
        let classification = if !click_ok {
            "unsupported"
        } else if url_changed {
            "navigated"
        } else if dom_changed {
            "dom_changed"
        } else if network_changed {
            "network_changed"
        } else {
            "no_effect"
        };

        Ok(json!({
            "ok": click_ok,
            "classification": classification,
            "requested": { "ref": ref_, "text": text },
            "target": target,
            "click": click_result,
            "settle": settle_result,
            "before": before,
            "after": after,
            "signals": {
                "url_changed": url_changed,
                "dom_changed": dom_changed,
                "network_changed": network_changed,
                "network_delta": network_delta,
            },
        }))
    }

    async fn click(&mut self, ref_: &str) -> Result<Value> {
        let lit = serde_json::to_string(ref_)?;
        let result = self.eval(&format!("__click({lit})"))?;
        if let Some(false) = result.get("ok").and_then(|v| v.as_bool()) {
            return Ok(result);
        }
        // Auto-follow <a href> clicks unless preventDefault'd (which sets follow=null).
        let follow = result.get("follow").and_then(|v| v.as_str()).unwrap_or("");
        if !follow.is_empty() {
            let target = self.resolve_url(follow)?;
            // If the resolved target is a known tracker URL (Bing/Google/DDG
            // result-link wrapper), decode to the real destination so we
            // don't land on the tracker's JS-redirect shell.
            let target = decode_tracker(&target).unwrap_or(target);
            return self.navigate(&target, false).await;
        }
        Ok(result)
    }

    fn type_(&self, ref_: &str, text: &str) -> Result<Value> {
        let r = serde_json::to_string(ref_)?;
        let t = serde_json::to_string(text)?;
        self.eval(&format!("__type({r}, {t})"))
    }

    async fn submit(&mut self, ref_: &str) -> Result<Value> {
        let lit = serde_json::to_string(ref_)?;
        let info = self.eval(&format!("__formData({lit})"))?;
        if let Some(false) = info.get("ok").and_then(|v| v.as_bool()) {
            return Ok(info);
        }
        let action = info.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let method = info.get("method").and_then(|v| v.as_str()).unwrap_or("get");
        let enctype = info
            .get("enctype")
            .and_then(|v| v.as_str())
            .unwrap_or("application/x-www-form-urlencoded");
        let pairs: Vec<(String, String)> = info
            .get("fields")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| {
                let arr = f.as_array()?;
                if arr.len() != 2 {
                    return None;
                }
                Some((arr[0].as_str()?.to_string(), arr[1].as_str()?.to_string()))
            })
            .collect();

        let target_url = self.resolve_url(action)?;

        match method {
            "get" => {
                let mut target =
                    url::Url::parse(&target_url).map_err(|e| anyhow!("resolve action url: {e}"))?;
                {
                    let mut qp = target.query_pairs_mut();
                    qp.clear();
                    for (n, v) in &pairs {
                        qp.append_pair(n, v);
                    }
                }
                self.navigate(target.as_str(), false).await
            }
            "post" => {
                if !enctype.starts_with("application/x-www-form-urlencoded") {
                    // multipart/form-data needs a different request shape
                    // (boundary, Content-Type, per-part headers). Defer until
                    // there's a real use case to model the surface against.
                    return Err(anyhow!(
                        "POST enctype '{enctype}' not supported (only application/x-www-form-urlencoded)"
                    ));
                }
                let body = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(pairs.iter().map(|(n, v)| (n.as_str(), v.as_str())))
                    .finish();
                let req = self
                    .http
                    .post(&target_url)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(body);
                self.navigate_with(req, &target_url, false, false).await
            }
            other => Err(anyhow!("unsupported form method '{other}'")),
        }
    }

    fn resolve_url(&self, href: &str) -> Result<String> {
        if href.is_empty() {
            return self
                .last_url
                .clone()
                .ok_or_else(|| anyhow!("no current page — call navigate first"));
        }
        if let Ok(u) = url::Url::parse(href)
            && u.has_host()
        {
            return Ok(u.to_string());
        }
        let base = self
            .last_url
            .as_deref()
            .ok_or_else(|| anyhow!("no current page — call navigate first"))?;
        let base_url = url::Url::parse(base).map_err(|e| anyhow!("base url: {e}"))?;
        Ok(base_url
            .join(href)
            .map_err(|e| anyhow!("join '{href}': {e}"))?
            .to_string())
    }
}

fn apply_status_to_blockmap(blockmap: &mut Value, status: u16) {
    if status < 400 {
        return;
    }
    let Some(density) = blockmap.get_mut("density").and_then(|v| v.as_object_mut()) else {
        return;
    };
    let shell_signaled = density
        .get("thin_shell")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || density
            .get("script_heavy_shell")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    density.insert("http_error_status".to_string(), json!(status));
    if shell_signaled {
        density.insert("shell_signals_suppressed".to_string(), json!(true));
        density.insert("thin_shell".to_string(), json!(false));
        density.insert("script_heavy_shell".to_string(), json!(false));
    }
    density.insert("likely_js_filled".to_string(), json!(false));
}

fn discovery_route_key(raw_url: &str) -> String {
    if let Ok(mut url) = url::Url::parse(raw_url) {
        url.set_fragment(None);
        let mut pairs: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(k, _)| !is_tracking_query_param(k))
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if pairs.is_empty() {
            url.set_query(None);
        } else {
            pairs.sort();
            url.query_pairs_mut().clear().extend_pairs(pairs);
        }
        if url.path() != "/" && url.path().ends_with('/') {
            let trimmed = url.path().trim_end_matches('/').to_string();
            url.set_path(&trimmed);
        }
        return url.to_string();
    }
    raw_url.trim().to_string()
}

fn is_tracking_query_param(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_")
        || matches!(
            key.as_str(),
            "fbclid" | "gclid" | "msclkid" | "mc_cid" | "mc_eid" | "igshid" | "ref" | "ref_src"
        )
}

fn static_dom_route_keys_from_body(body: &str, base_url: &str) -> HashSet<String> {
    let tree = parse_html_to_tree(body);
    let mut out = HashSet::new();
    let base = url::Url::parse(base_url).ok();
    collect_static_dom_route_keys(&tree, base.as_ref(), &mut out);
    out
}

fn collect_static_dom_route_keys(node: &Value, base: Option<&url::Url>, out: &mut HashSet<String>) {
    let Some(obj) = node.as_object() else {
        return;
    };
    if obj.get("type").and_then(|t| t.as_str()) == Some("element") {
        let tag = obj.get("tag").and_then(|t| t.as_str()).unwrap_or("");
        let attrs = obj.get("attrs").and_then(|a| a.as_object());
        let route_attr = match tag {
            "a" | "area" => attrs.and_then(|a| a.get("href")).and_then(|v| v.as_str()),
            "form" => attrs.and_then(|a| a.get("action")).and_then(|v| v.as_str()),
            _ => None,
        };
        if let Some(raw) = route_attr.filter(|s| !s.trim().is_empty()) {
            if let Some(base) = base {
                if let Ok(resolved) = base.join(raw) {
                    out.insert(discovery_route_key(resolved.as_str()));
                }
            } else {
                out.insert(discovery_route_key(raw));
            }
        }
    }
    if let Some(children) = obj.get("children").and_then(|c| c.as_array()) {
        for child in children {
            collect_static_dom_route_keys(child, base, out);
        }
    }
}

fn discovery_page_owned(base: &str, raw_url: &str) -> bool {
    let Ok(base_url) = url::Url::parse(base) else {
        return false;
    };
    let target = url::Url::parse(raw_url).or_else(|_| base_url.join(raw_url));
    let Ok(target) = target else {
        return false;
    };
    match (base_url.host_str(), target.host_str()) {
        (Some(base_host), Some(target_host)) => {
            target_host == base_host || target_host.ends_with(&format!(".{base_host}"))
        }
        _ => false,
    }
}

fn is_api_like_url(raw_url: &str) -> bool {
    let lower = raw_url.to_lowercase();
    lower.contains("/api/")
        || lower.contains("/graphql")
        || lower.contains("/gql")
        || lower.contains(".json")
        || lower.contains("/_next/data/")
        || lower.contains("/__data.json")
}

fn summarize_discover_navigate(nav: Option<&Value>) -> Value {
    let Some(nav) = nav else {
        return Value::Null;
    };
    let density = nav
        .get("blockmap")
        .and_then(|v| v.get("density"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "navigation_id": nav.get("navigation_id").cloned().unwrap_or(Value::Null),
        "status": nav.get("status").cloned().unwrap_or(Value::Null),
        "url": nav.get("url").cloned().unwrap_or(Value::Null),
        "challenge": nav.get("challenge").cloned().unwrap_or(Value::Null),
        "rate_limit": nav.get("rate_limit").cloned().unwrap_or(Value::Null),
        "browser_route": nav.get("browser_route").cloned().unwrap_or(Value::Null),
        "tool_recommendations": nav.get("tool_recommendations").cloned().unwrap_or(Value::Null),
        "tool_confidence": nav.get("tool_confidence").cloned().unwrap_or(Value::Null),
        "tool_margin": nav.get("tool_margin").cloned().unwrap_or(Value::Null),
        "density": density,
        "network_stores": nav.get("network_stores").cloned().unwrap_or(Value::Null),
    })
}

fn raw_network_route_candidates(
    capture: &network_store::NetworkCapture,
    limit: usize,
) -> Vec<Value> {
    if capture.body_truncated || limit == 0 {
        return Vec::new();
    }
    let mut roots = Vec::new();
    if matches!(capture.kind, network_store::ContentKind::Ndjson) {
        for (idx, line) in capture.body_preview.lines().enumerate().take(100) {
            if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
                roots.push((value, format!("$[{idx}]")));
            }
        }
    } else if let Ok(value) = serde_json::from_str::<Value>(&capture.body_preview) {
        roots.push((value, "$".to_string()));
    }

    let mut state = RawNetworkRouteState {
        capture,
        limit,
        visited: 0,
        seen: HashSet::new(),
        out: Vec::new(),
    };
    for (value, path) in roots {
        collect_raw_network_routes(&value, &path, 0, &mut state);
        if state.out.len() >= limit {
            break;
        }
    }
    state.out
}

struct RawNetworkRouteState<'a> {
    capture: &'a network_store::NetworkCapture,
    limit: usize,
    visited: usize,
    seen: HashSet<String>,
    out: Vec<Value>,
}

fn collect_raw_network_routes(
    value: &Value,
    path: &str,
    depth: usize,
    state: &mut RawNetworkRouteState<'_>,
) {
    if depth > RAW_NETWORK_ROUTE_MAX_DEPTH
        || state.visited >= state.limit.saturating_mul(120).max(400)
        || state.out.len() >= state.limit
    {
        return;
    }
    state.visited += 1;
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = json_path_child(path, key);
                if let Some(raw) = child.as_str()
                    && is_route_like_json_key(key)
                    && let Some(url) = resolve_json_url(raw, &state.capture.url)
                {
                    push_raw_network_route_candidate(
                        &url,
                        raw,
                        key,
                        &child_path,
                        state.capture,
                        &mut state.seen,
                        &mut state.out,
                    );
                    if state.out.len() >= state.limit {
                        return;
                    }
                }
                if let Value::Array(arr) = child
                    && is_route_like_json_key(key)
                {
                    for (idx, entry) in arr.iter().take(100).enumerate() {
                        if let Some(raw) = entry.as_str()
                            && let Some(url) = resolve_json_url(raw, &state.capture.url)
                        {
                            let entry_path = format!("{child_path}[{idx}]");
                            push_raw_network_route_candidate(
                                &url,
                                raw,
                                key,
                                &entry_path,
                                state.capture,
                                &mut state.seen,
                                &mut state.out,
                            );
                            if state.out.len() >= state.limit {
                                return;
                            }
                        }
                    }
                }
                if matches!(child, Value::Object(_) | Value::Array(_)) {
                    collect_raw_network_routes(child, &child_path, depth + 1, state);
                    if state.out.len() >= state.limit {
                        return;
                    }
                }
            }
        }
        Value::Array(arr) => {
            for (idx, child) in arr.iter().take(250).enumerate() {
                let child_path = format!("{path}[{idx}]");
                collect_raw_network_routes(child, &child_path, depth + 1, state);
                if state.out.len() >= state.limit {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn push_raw_network_route_candidate(
    url: &str,
    label: &str,
    key: &str,
    path: &str,
    capture: &network_store::NetworkCapture,
    seen: &mut HashSet<String>,
    out: &mut Vec<Value>,
) {
    let route_key = discovery_route_key(url);
    if !seen.insert(route_key) {
        return;
    }
    let normalized = normalized_key(key);
    let kind = if is_api_like_url(url)
        || normalized.contains("endpoint")
        || normalized == "api"
        || normalized.ends_with("api")
    {
        "api_endpoint"
    } else {
        "network_route"
    };
    out.push(json!({
        "url": url,
        "label": label,
        "kind": kind,
        "score": if kind == "api_endpoint" { DISCOVERY_RAW_NETWORK_API_SCORE } else { DISCOVERY_RAW_NETWORK_ROUTE_SCORE },
        "confidence": if kind == "api_endpoint" { DISCOVERY_RAW_NETWORK_API_CONFIDENCE } else { DISCOVERY_RAW_NETWORK_ROUTE_CONFIDENCE },
        "matched_terms": [],
        "provenance": [{
            "source": "network",
            "capture_id": capture.capture_id,
            "url": capture.url,
            "path": path,
            "reason": "route-like JSON string field"
        }]
    }));
}

fn is_route_like_json_key(key: &str) -> bool {
    let key = normalized_key(key);
    matches!(
        key.as_str(),
        "url" | "href" | "link" | "path" | "route" | "permalink" | "endpoint" | "api" | "weburl"
    ) || key.ends_with("url")
        || key.ends_with("href")
        || key.ends_with("path")
        || key.contains("endpoint")
}

fn push_source(route: &mut Value, source: &str) {
    let Some(map) = route.as_object_mut() else {
        return;
    };
    let sources = map
        .entry("sources".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(arr) = sources.as_array_mut()
        && !arr.iter().any(|v| v.as_str() == Some(source))
    {
        arr.push(Value::String(source.to_string()));
    }
}

fn append_provenance(route: &mut Value, provenance: &Value) {
    let Some(map) = route.as_object_mut() else {
        return;
    };
    let target = map
        .entry("provenance".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(target_arr) = target.as_array_mut() else {
        return;
    };
    if let Some(items) = provenance.as_array() {
        target_arr.extend(items.iter().cloned());
    }
}

fn upsert_discovery_route(
    routes: &mut Vec<Value>,
    index: &mut HashMap<String, usize>,
    item: &Value,
    source: &str,
    default_kind: &str,
    default_confidence: f64,
    base_url: &str,
) {
    let Some(url) = item.get("url").and_then(|v| v.as_str()) else {
        return;
    };
    if url.trim().is_empty() {
        return;
    }
    let key = discovery_route_key(url);
    if let Some(idx) = index.get(&key).copied() {
        if let Some(route) = routes.get_mut(idx) {
            push_source(route, source);
            append_provenance(route, item.get("provenance").unwrap_or(&Value::Null));
            let existing = route.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let incoming = item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if incoming > existing
                && let Some(map) = route.as_object_mut()
            {
                map.insert("score".to_string(), json!(round3(incoming)));
            }
        }
        return;
    }

    let label = item
        .get("label")
        .or_else(|| item.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or(url);
    let kind = item
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or(default_kind);
    let score = item.get("score").and_then(|v| v.as_f64()).unwrap_or(0.45);
    let confidence = item
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(default_confidence);
    let page_owned = item
        .get("page_owned")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| discovery_page_owned(base_url, url));
    let matched_terms = item
        .get("matched_terms")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let provenance = item
        .get("provenance")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    let route = json!({
        "url": url,
        "label": label,
        "kind": kind,
        "source": source,
        "sources": [source],
        "score": round3(score),
        "confidence": round3(confidence),
        "matched_terms": matched_terms,
        "page_owned": page_owned,
        "provenance": provenance,
    });
    index.insert(key, routes.len());
    routes.push(route);
}

fn discovery_escalations(
    challenge: &Option<Value>,
    rate_limit: &Option<Value>,
    browser_route: &Option<Value>,
    route_count: usize,
    form_count: usize,
) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(challenge) = challenge {
        out.push(json!({
            "reason": "challenge_required",
            "recommended_tool": "chrome",
            "confidence": challenge.get("confidence").cloned().unwrap_or(json!(0.9)),
            "provider": challenge.get("provider").cloned().unwrap_or(Value::Null),
            "evidence": ["navigate.challenge"],
            "hint": challenge.get("hint").cloned().unwrap_or_else(|| json!("Solve the challenge in Chrome or replay clearance cookies.")),
        }));
        return out;
    }
    if let Some(rate_limit) = rate_limit {
        out.push(json!({
            "reason": "rate_limited",
            "recommended_tool": "retry_later",
            "confidence": 0.9,
            "status": rate_limit.get("status").cloned().unwrap_or(Value::Null),
            "retry_after": rate_limit.get("retry_after").cloned().unwrap_or(Value::Null),
            "evidence": ["navigate.rate_limit"],
        }));
        return out;
    }
    if let Some(route) = browser_route {
        let reason = route.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        let weak_missing_action =
            reason == "missing_primary_action" && (route_count > 0 || form_count > 0);
        if !weak_missing_action {
            out.push(json!({
                "reason": reason,
                "recommended_tool": "chrome",
                "confidence": route.get("confidence").cloned().unwrap_or(json!(0.7)),
                "evidence": route.get("evidence").cloned().unwrap_or_else(|| json!(["navigate.browser_route"])),
                "hint": route.get("hint").cloned().unwrap_or_else(|| json!("Route this page to real browser automation.")),
            }));
        }
    }
    out
}

#[derive(Debug, Clone)]
struct NetworkObjectCandidate {
    kind: String,
    title: Option<String>,
    url: Option<String>,
    text: Option<String>,
    fields: serde_json::Map<String, Value>,
    score: f64,
    confidence: f64,
    matched_terms: Vec<String>,
    capture_id: u64,
    capture_url: String,
    source_kind: String,
    path: String,
}

impl NetworkObjectCandidate {
    fn to_value(&self, id: usize) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("id".to_string(), json!(format!("net:{id}")));
        obj.insert("kind".to_string(), json!(self.kind));
        if let Some(title) = &self.title {
            obj.insert("title".to_string(), json!(title));
        }
        if let Some(url) = &self.url {
            obj.insert("url".to_string(), json!(url));
            obj.insert(
                "actions".to_string(),
                json!([{ "kind": "open", "url": url }]),
            );
        }
        if let Some(text) = &self.text {
            obj.insert("text".to_string(), json!(text));
            obj.insert("snippet".to_string(), json!(text));
        }
        obj.insert("fields".to_string(), Value::Object(self.fields.clone()));
        obj.insert("score".to_string(), json!(round3(self.score)));
        obj.insert("confidence".to_string(), json!(round3(self.confidence)));
        obj.insert("matched_terms".to_string(), json!(self.matched_terms));
        obj.insert(
            "provenance".to_string(),
            json!([{
                "source": "network",
                "capture_id": self.capture_id,
                "url": self.capture_url,
                "kind": self.source_kind,
                "path": self.path,
                "reason": "content-bearing JSON object"
            }]),
        );
        Value::Object(obj)
    }
}

struct NetworkCandidateWalk<'a> {
    capture: &'a network_store::NetworkCapture,
    terms: &'a [String],
    out: &'a mut Vec<NetworkObjectCandidate>,
    visited: usize,
    max_nodes: usize,
    max_objects: usize,
}

fn parse_object_type_filter(types: Option<&Value>) -> Option<HashSet<String>> {
    let set: HashSet<String> = types?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect();
    if set.is_empty() { None } else { Some(set) }
}

fn network_kind_allowed(kind: &str, allowed: &HashSet<String>) -> bool {
    allowed.contains(kind)
        || (kind.ends_with("_card") && allowed.contains("card"))
        || (kind == "network_object" && allowed.contains("network_objects"))
}

fn dedupe_network_objects(objects: &mut Vec<NetworkObjectCandidate>) {
    let mut seen = HashSet::new();
    objects.retain(|obj| {
        let key = format!(
            "{}\n{}\n{}",
            obj.title.as_deref().unwrap_or(""),
            obj.url.as_deref().unwrap_or(""),
            obj.text.as_deref().unwrap_or("")
        );
        if key.trim().is_empty() {
            return true;
        }
        seen.insert(key)
    });
}

fn extract_network_objects_from_capture(
    capture: &network_store::NetworkCapture,
    terms: &[String],
    max_objects: usize,
) -> std::result::Result<Vec<NetworkObjectCandidate>, String> {
    let mut roots = Vec::new();
    if matches!(capture.kind, network_store::ContentKind::Ndjson) {
        for (idx, line) in capture.body_preview.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(trimmed)
                .map_err(|e| format!("parse ndjson line {}: {e}", idx + 1))?;
            roots.push((value, format!("$[{idx}]")));
        }
    } else {
        let value: Value = serde_json::from_str(&capture.body_preview).map_err(|e| {
            if capture.body_truncated {
                format!("body_preview is truncated and does not parse as JSON: {e}")
            } else {
                format!("parse json: {e}")
            }
        })?;
        roots.push((value, "$".to_string()));
    }

    let mut out = Vec::new();
    let max_nodes = max_objects.saturating_mul(80).max(400);
    {
        let mut walk = NetworkCandidateWalk {
            capture,
            terms,
            out: &mut out,
            visited: 0,
            max_nodes,
            max_objects,
        };
        for (value, path) in &roots {
            collect_network_candidates(value, path, &mut walk);
            if walk.out.len() >= max_objects {
                break;
            }
        }
    }
    Ok(out)
}

fn collect_network_candidates(value: &Value, path: &str, walk: &mut NetworkCandidateWalk<'_>) {
    if walk.visited >= walk.max_nodes || walk.out.len() >= walk.max_objects {
        return;
    }
    walk.visited += 1;
    match value {
        Value::Object(map) => {
            if let Some(candidate) =
                network_candidate_from_object(map, path, walk.capture, walk.terms)
            {
                walk.out.push(candidate);
                if walk.out.len() >= walk.max_objects {
                    return;
                }
            }
            for (key, child) in map {
                if matches!(child, Value::Array(_) | Value::Object(_)) {
                    let child_path = json_path_child(path, key);
                    collect_network_candidates(child, &child_path, walk);
                    if walk.visited >= walk.max_nodes || walk.out.len() >= walk.max_objects {
                        return;
                    }
                }
            }
        }
        Value::Array(arr) => {
            for (idx, child) in arr.iter().take(250).enumerate() {
                let child_path = format!("{path}[{idx}]");
                collect_network_candidates(child, &child_path, walk);
                if walk.visited >= walk.max_nodes || walk.out.len() >= walk.max_objects {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn network_candidate_from_object(
    map: &serde_json::Map<String, Value>,
    path: &str,
    capture: &network_store::NetworkCapture,
    terms: &[String],
) -> Option<NetworkObjectCandidate> {
    let fields = summarize_network_fields(map);
    if fields.is_empty() {
        return None;
    }

    let text = first_scalar_field(
        map,
        &[
            "description",
            "summary",
            "snippet",
            "excerpt",
            "text",
            "body",
            "content",
            "caption",
            "abstract",
        ],
    );
    let mut title = first_scalar_field(
        map,
        &[
            "title",
            "name",
            "headline",
            "label",
            "displayname",
            "fullname",
            "modelid",
            "model",
            "question",
            "term",
            "id",
        ],
    );
    if title.is_none() {
        title = text.as_ref().map(|s| truncate_clean(s, 90));
    }
    let raw_url = first_scalar_field(
        map,
        &[
            "url",
            "href",
            "link",
            "permalink",
            "htmlurl",
            "canonicalurl",
            "weburl",
            "externalurl",
        ],
    );
    let url = raw_url
        .as_deref()
        .and_then(|u| resolve_json_url(u, &capture.url));

    let field_text = serde_json::to_string(&fields).unwrap_or_default();
    let combined = format!(
        "{} {} {} {} {}",
        path,
        title.as_deref().unwrap_or(""),
        url.as_deref().unwrap_or(""),
        text.as_deref().unwrap_or(""),
        field_text
    );
    let matched_terms = network_matched_terms(&combined, terms);
    if title.is_none() && url.is_none() && text.is_none() && matched_terms.is_empty() {
        return None;
    }
    if title.as_deref().unwrap_or("").len() < 2 && fields.len() < 2 {
        return None;
    }

    let kind =
        infer_network_object_kind(map, title.as_deref(), url.as_deref(), text.as_deref(), path);
    let term_boost = if terms.is_empty() {
        0.0
    } else {
        (matched_terms.len() as f64 / terms.len() as f64 * 0.35).min(0.35)
    };
    let mut score = 0.36 + (capture.score.min(120) as f64 / 1000.0) + term_boost;
    if title.is_some() {
        score += 0.14;
    }
    if text.is_some() {
        score += 0.07;
    }
    if url.is_some() {
        score += 0.05;
    }
    if kind != "network_object" {
        score += 0.08;
    }
    if fields.len() >= 4 {
        score += 0.04;
    }
    let confidence = (0.48
        + if title.is_some() { 0.12 } else { 0.0 }
        + if text.is_some() { 0.08 } else { 0.0 }
        + if kind != "network_object" { 0.08 } else { 0.0 }
        + (capture.score.min(100) as f64 / 1000.0))
        .min(0.94);

    Some(NetworkObjectCandidate {
        kind,
        title,
        url,
        text: text.map(|s| truncate_clean(&s, 500)),
        fields,
        score: score.min(1.0),
        confidence,
        matched_terms,
        capture_id: capture.capture_id,
        capture_url: capture.url.clone(),
        source_kind: serde_json::to_value(capture.kind)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| format!("{:?}", capture.kind)),
        path: path.to_string(),
    })
}

fn summarize_network_fields(
    map: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut fields = serde_json::Map::new();
    for (key, value) in map {
        if fields.len() >= 24 {
            break;
        }
        let summary = if is_sensitive_field(key) {
            Some(json!("[REDACTED]"))
        } else {
            summarize_network_value(value)
        };
        if let Some(summary) = summary {
            fields.insert(key.clone(), summary);
        }
    }
    fields
}

fn summarize_network_value(value: &Value) -> Option<Value> {
    match value {
        Value::String(s) => Some(json!(truncate_clean(s, 500))),
        Value::Number(_) | Value::Bool(_) => Some(value.clone()),
        Value::Array(arr) => {
            let values: Vec<Value> = arr
                .iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(json!(truncate_clean(s, 160))),
                    Value::Number(_) | Value::Bool(_) => Some(v.clone()),
                    _ => None,
                })
                .take(12)
                .collect();
            if values.is_empty() {
                None
            } else {
                Some(Value::Array(values))
            }
        }
        _ => None,
    }
}

fn first_scalar_field(map: &serde_json::Map<String, Value>, wanted: &[&str]) -> Option<String> {
    for want in wanted {
        for (key, value) in map {
            if normalized_key(key) == *want && !is_sensitive_field(key) {
                let text = scalar_to_string(value)?;
                if !text.is_empty() {
                    return Some(truncate_clean(&text, 240));
                }
            }
        }
    }
    None
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(clean_ws(s)),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn infer_network_object_kind(
    map: &serde_json::Map<String, Value>,
    title: Option<&str>,
    url: Option<&str>,
    text: Option<&str>,
    path: &str,
) -> String {
    let keys = map.keys().cloned().collect::<Vec<_>>().join(" ");
    let hay = format!(
        "{} {} {} {} {}",
        keys,
        title.unwrap_or(""),
        url.unwrap_or(""),
        text.unwrap_or(""),
        path
    )
    .to_lowercase();
    if hay.contains("huggingface")
        || hay.contains("pipeline_tag")
        || hay.contains("modelid")
        || hay.contains("model_id")
        || title.map(|t| t.contains('/')).unwrap_or(false)
    {
        return "model_card".to_string();
    }
    if hay.contains("course")
        || hay.contains("instructor")
        || hay.contains("enroll")
        || hay.contains("university")
    {
        return "course_card".to_string();
    }
    if hay.contains("price")
        || hay.contains("sku")
        || hay.contains("product")
        || hay.contains("rating")
        || hay.contains("brand")
    {
        return "product_card".to_string();
    }
    if hay.contains("headline")
        || hay.contains("article")
        || hay.contains("datepublished")
        || hay.contains("date_published")
        || hay.contains("author")
        || hay.contains("news")
        || hay.contains("story")
    {
        return "article_card".to_string();
    }
    "network_object".to_string()
}

fn network_terms(input: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for raw in input
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(|s| s.to_ascii_lowercase())
    {
        if raw.len() <= 2 || is_network_stop_word(&raw) || terms.contains(&raw) {
            continue;
        }
        terms.push(raw);
    }
    terms
}

fn network_matched_terms(haystack: &str, terms: &[String]) -> Vec<String> {
    let haystack = haystack.to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .cloned()
        .collect()
}

fn is_network_stop_word(term: &str) -> bool {
    matches!(
        term,
        "the"
            | "and"
            | "for"
            | "with"
            | "from"
            | "that"
            | "this"
            | "into"
            | "what"
            | "when"
            | "where"
            | "find"
            | "list"
            | "show"
            | "many"
            | "currently"
            | "available"
            | "make"
            | "sure"
            | "can"
            | "perform"
            | "give"
            | "get"
    )
}

fn is_sensitive_field(key: &str) -> bool {
    let key = normalized_key(key);
    key.contains("password")
        || key.contains("passwd")
        || key.contains("token")
        || key.contains("secret")
        || key.contains("credential")
        || key.contains("session")
        || key.contains("cookie")
        || key.contains("authorization")
}

fn normalized_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn json_path_child(path: &str, key: &str) -> String {
    if key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        format!("{path}.{key}")
    } else {
        format!("{path}[{}]", serde_json::to_string(key).unwrap_or_default())
    }
}

fn resolve_json_url(raw: &str, base: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with("javascript:") || raw.starts_with("data:") {
        return None;
    }
    if let Ok(parsed) = url::Url::parse(raw)
        && parsed.has_host()
    {
        return Some(parsed.to_string());
    }
    url::Url::parse(base)
        .ok()
        .and_then(|base| base.join(raw).ok())
        .map(|u| u.to_string())
}

fn clean_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_clean(s: &str, max: usize) -> String {
    let cleaned = clean_ws(s);
    if cleaned.len() <= max {
        return cleaned;
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", cleaned[..end].trim_end())
}

fn round3(n: f64) -> f64 {
    (n * 1000.0).round() / 1000.0
}

fn hash_string(s: &str) -> String {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn hash_value(value: &Value) -> String {
    hash_string(&serde_json::to_string(value).unwrap_or_default())
}

fn summarize_blockmap_for_activation(blockmap: &Value) -> Value {
    let interactives = blockmap.get("interactives").unwrap_or(&Value::Null);
    json!({
        "title": blockmap.get("title").cloned().unwrap_or(Value::Null),
        "structure_count": blockmap.get("structure").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        "heading_count": blockmap.get("headings").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        "links": interactives.get("links").and_then(|v| v.as_u64()).unwrap_or(0),
        "buttons": interactives.get("buttons").and_then(|v| v.as_u64()).unwrap_or(0),
        "inputs": interactives.get("inputs").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        "forms": interactives.get("forms").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        "density": blockmap.get("density").cloned().unwrap_or(Value::Null),
    })
}

// Search engines wrap result links in tracker URLs (so they can record
// click-throughs) that the destination's own server never sees. When an
// agent click-follows one of these, the right behavior is to land on the
// real destination, not the tracker page — which is often a JS-redirect
// shell our static fetch can't actually follow.
//
// Returns Some(decoded_url) for known tracker shapes, None otherwise.
// Caller (click follow) substitutes the decoded URL when present.
fn decode_tracker(href: &str) -> Option<String> {
    use base64::Engine;
    let parsed = url::Url::parse(href).ok()?;
    let host = parsed.host_str()?;

    // Bing — bing.com/ck/a?...&u=a1<urlsafe-base64>&...
    // The 'a1' prefix is Bing's "this is a base64-encoded URL" marker.
    if host.ends_with("bing.com") && parsed.path() == "/ck/a" {
        let u = parsed.query_pairs().find(|(k, _)| k == "u")?.1;
        let payload = u.strip_prefix("a1").unwrap_or(&u);
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.as_bytes())
            .ok()?;
        return String::from_utf8(bytes).ok();
    }

    // Google — google.com/url?q=<urlencoded>&...
    // The url crate's query_pairs() already URL-decodes for us.
    if host.ends_with("google.com") && parsed.path() == "/url" {
        return parsed
            .query_pairs()
            .find(|(k, _)| k == "q")
            .map(|(_, v)| v.into_owned());
    }

    // DuckDuckGo HTML version — duckduckgo.com/l/?uddg=<urlencoded>&...
    if host.ends_with("duckduckgo.com") && parsed.path() == "/l/" {
        return parsed
            .query_pairs()
            .find(|(k, _)| k == "uddg")
            .map(|(_, v)| v.into_owned());
    }

    None
}

fn format_js_exception(ex: rquickjs::Value) -> String {
    if let Some(obj) = ex.as_object() {
        let name: String = obj.get("name").unwrap_or_else(|_| "Error".to_string());
        let msg: String = obj.get("message").unwrap_or_default();
        if !msg.is_empty() {
            return format!("{name}: {msg}");
        }
        return name;
    }
    if let Some(s) = ex.as_string().and_then(|s| s.to_string().ok()) {
        return s;
    }
    "<unknown JS exception>".to_string()
}

// One <script> element from a parsed page.
//
// `kind` distinguishes async from non-async. Inline and Defer are treated as
// Sync for execution because we don't have incremental HTML parsing — by the
// time scripts run, the document is fully parsed, so "execute after parse in
// document order" (Defer's spec semantics) and "execute now in document order"
// (Sync) collapse to the same thing for us. Only `async` differs: async
// scripts may execute out of document order (we run them after the sync
// queue, in fetch-completion order).
//
// Inline scripts cannot be async (browsers ignore the attribute on inline),
// so we don't track kind for Inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Sync,
    Async,
}

enum ScriptItem {
    Inline(String),
    External { url: String, kind: ScriptKind },
}

// Heuristic: does this source look like an ES module? Checks the first few
// lines for `import` / `export` statements. Used by the static script-eval
// loop to route module-shaped sources through __loadModule (which fetches
// deps then evals) vs plain eval (which would throw SyntaxError on the
// import keyword and dispatch script_executed{error}).
//
// Conservative — false negatives just go through plain eval (and fail
// loudly via PR #6's eval-error path); false positives go through the
// module loader (which strips imports + evals — equivalent to plain eval
// for source with no actual imports). No correctness loss either way.
fn looks_like_module(source: &str) -> bool {
    for line in source.lines().take(50) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import ")
            || trimmed.starts_with("import{")
            || trimmed.starts_with("import\"")
            || trimmed.starts_with("import'")
            || trimmed.starts_with("import*")
            || trimmed.starts_with("export ")
            || trimmed.starts_with("export{")
            || trimmed.starts_with("export*")
            || trimmed.starts_with("export default")
        {
            return true;
        }
    }
    false
}

fn looks_like_enhanced_module(source: &str) -> bool {
    let bytes = source.as_bytes();
    let export_needles: [&[u8]; 8] = [
        b"export{",
        b"export*",
        b"export default",
        b"export const ",
        b"export let ",
        b"export var ",
        b"export function ",
        b"export class ",
    ];
    for i in 0..bytes.len() {
        let matched = match bytes[i] {
            b'i' => {
                starts_with_at(bytes, i, b"import.meta") || starts_with_at(bytes, i, b"import(")
            }
            b'e' => export_needles
                .iter()
                .any(|needle| starts_with_at(bytes, i, needle)),
            _ => false,
        };
        if matched {
            return true;
        }
    }
    false
}

fn starts_with_at(haystack: &[u8], idx: usize, needle: &[u8]) -> bool {
    haystack
        .get(idx..idx.saturating_add(needle.len()))
        .is_some_and(|chunk| chunk == needle)
}

// Walk the parsed HTML tree and collect <script> elements in document order.
// Skips:
//   - <script type="application/json"> (data, not code — accessible via eval)
//   - <script type="application/ld+json"> (structured data)
//   - any non-empty `type` other than text/javascript or module
// External srcs resolved against `base_url`; ones that fail to resolve are dropped.
fn collect_scripts(tree: &Value, base_url: &str) -> Vec<ScriptItem> {
    let mut out = Vec::new();
    let base = url::Url::parse(base_url).ok();
    walk_for_scripts(tree, base.as_ref(), &mut out);
    out
}

fn walk_for_scripts(node: &Value, base: Option<&url::Url>, out: &mut Vec<ScriptItem>) {
    let Some(obj) = node.as_object() else {
        return;
    };
    let is_element = obj.get("type").and_then(|t| t.as_str()) == Some("element");
    let tag = obj.get("tag").and_then(|t| t.as_str()).unwrap_or("");
    if is_element && tag == "script" {
        let attrs = obj.get("attrs").and_then(|a| a.as_object());
        let src = attrs.and_then(|a| a.get("src")).and_then(|v| v.as_str());
        let ty = attrs
            .and_then(|a| a.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_js = ty.is_empty()
            || ty.eq_ignore_ascii_case("module")
            || ty.to_ascii_lowercase().contains("javascript");
        if is_js {
            if let Some(src_url) = src {
                if !src_url.is_empty()
                    && let Some(b) = base
                    && let Ok(resolved) = b.join(src_url)
                {
                    // HTML treats `async` and `defer` as boolean attrs — any
                    // presence (even empty value) counts. `async` wins if both
                    // are set, matching browsers.
                    let is_async = attrs.and_then(|a| a.get("async")).is_some();
                    let kind = if is_async {
                        ScriptKind::Async
                    } else {
                        ScriptKind::Sync
                    };
                    out.push(ScriptItem::External {
                        url: resolved.to_string(),
                        kind,
                    });
                }
            } else if let Some(children) = obj.get("children").and_then(|c| c.as_array()) {
                let mut content = String::new();
                for child in children {
                    if let Some(cobj) = child.as_object()
                        && cobj.get("type").and_then(|t| t.as_str()) == Some("text")
                        && let Some(text) = cobj.get("content").and_then(|t| t.as_str())
                    {
                        content.push_str(text);
                    }
                }
                if !content.trim().is_empty() {
                    out.push(ScriptItem::Inline(content));
                }
            }
        }
    }
    if let Some(children) = obj.get("children").and_then(|c| c.as_array()) {
        for child in children {
            walk_for_scripts(child, base, out);
        }
    }
}

fn parse_html_to_tree(html: &str) -> Value {
    let dom = html5ever::parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .unwrap_or_else(|_| RcDom::default());
    // The Document node's children include doctype + the <html> element.
    for child in dom.document.children.borrow().iter() {
        if let NodeData::Element { name, .. } = &child.data
            && name.local.as_ref() == "html"
        {
            return node_to_json(child);
        }
    }
    json!({"type": "element", "tag": "html", "attrs": {}, "children": []})
}

// Parse an HTML fragment (e.g. the rhs of `el.innerHTML = '<p>...</p>'`).
// Context element is <body> — matches what real browsers do for innerHTML
// on most elements. (Tables and selects use different contexts; v1 punts
// on those — they parse OK under <body> in practice for typical uses.)
//
// Returns a JSON string with the shape {type: "element", tag: "fragment",
// attrs: {}, children: [...]} where each child matches the format from
// parse_html_to_tree. Caller is JS-side __parseHTMLFragment() in
// dom.js — it JSON.parses the string and feeds children to buildChildren.
//
// Why JSON-string instead of constructing a JS object: avoids reaching
// across the rquickjs binding boundary to build nested objects, which
// is significantly more lines and harder to maintain than a JSON dance.
fn parse_html_fragment_to_json(html: &str) -> String {
    use html5ever::interface::QualName;
    use html5ever::{local_name, ns};

    let context = QualName::new(None, ns!(html), local_name!("body"));
    let dom = html5ever::parse_fragment(
        RcDom::default(),
        Default::default(),
        context,
        Vec::new(),
        false,
    )
    .from_utf8()
    .read_from(&mut html.as_bytes())
    .unwrap_or_else(|_| RcDom::default());

    // parse_fragment produces a synthetic context element under
    // dom.document — its children are the actual fragment.
    let mut children: Vec<Value> = Vec::new();
    for ctx_child in dom.document.children.borrow().iter() {
        if matches!(&ctx_child.data, NodeData::Element { .. }) {
            for inner in ctx_child.children.borrow().iter() {
                if let Some(v) = child_to_json(inner) {
                    children.push(v);
                }
            }
            break;
        }
    }
    let tree = json!({
        "type": "element",
        "tag": "fragment",
        "attrs": {},
        "children": children,
    });
    serde_json::to_string(&tree).unwrap_or_else(|_| "{}".to_string())
}

fn node_to_json(handle: &Handle) -> Value {
    match &handle.data {
        NodeData::Element { name, attrs, .. } => {
            let mut attr_map = serde_json::Map::new();
            for attr in attrs.borrow().iter() {
                attr_map.insert(
                    attr.name.local.to_string(),
                    Value::String(attr.value.to_string()),
                );
            }
            let children: Vec<Value> = handle
                .children
                .borrow()
                .iter()
                .filter_map(child_to_json)
                .collect();
            json!({
                "type": "element",
                "tag": name.local.as_ref(),
                "attrs": Value::Object(attr_map),
                "children": children,
            })
        }
        _ => Value::Null,
    }
}

fn child_to_json(handle: &Handle) -> Option<Value> {
    match &handle.data {
        NodeData::Text { contents } => {
            let s = contents.borrow().to_string();
            Some(json!({"type": "text", "content": s}))
        }
        NodeData::Element { .. } => Some(node_to_json(handle)),
        // Skip Doctype, Comment, ProcessingInstruction, Document.
        _ => None,
    }
}

fn ok_response(id: Value, result: Value) -> Response {
    Response {
        id,
        result: Some(result),
        error: None,
    }
}

fn err_response(id: Value, code: i32, message: impl Into<String>) -> Response {
    Response {
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: message.into(),
        }),
    }
}

fn write_response(out: &mut impl Write, resp: &Response) -> Result<()> {
    writeln!(out, "{}", serde_json::to_string(resp)?)?;
    out.flush()?;
    Ok(())
}

fn emit_event(name: &str, fields: Value) {
    if !EVENTS_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let payload = json!({ "event": name, "data": fields });
    eprintln!("{}", serde_json::to_string(&payload).unwrap_or_default());
}

// Lowercased host extracted from a URL; "" on parse failure or hostless.
// Used by script_decision events. Centralized so the event shape stays
// consistent across the first-pass and assembly-pass emissions.
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_lowercase()))
        .unwrap_or_default()
}

fn script_kind_str(kind: ScriptKind) -> &'static str {
    match kind {
        ScriptKind::Sync => "sync",
        ScriptKind::Async => "async",
    }
}

// One per-script decision recorded during navigate_with. Accumulated so
// that derive_outcome() can emit a paired `outcome_for_decision` event per
// decision after the navigate's success/fail is determined. T2 (offline
// aggregator) joins these to fit `block:<host>` / `allow:<host>` posteriors.
//
// Keep this minimal: we only need the policy-relevant key (skip vs queued
// + host) plus the per-event action string for downstream filtering.
#[derive(Debug, Clone)]
struct DecisionRecord {
    /// "skip" or "queued". "fetch_failed" decisions are not attributable to
    /// a policy choice (the network failed, not us), so they're never
    /// recorded here — the script_decision event still fires for visibility.
    action: &'static str,
    /// Lowercased host of the script URL. Empty on parse failure (URLs we
    /// can't parse fall through with action skipped silently from the
    /// outcome stream — they wouldn't bind to a useful key anyway).
    host: String,
}

impl DecisionRecord {
    /// Decision key under which T2 buckets posteriors. The two kinds we
    /// emit today: `block:<host>` for any skip (blocklist or prefit_blocklist)
    /// and `allow:<host>` for queued. Future kinds (api_stub, settle, ...)
    /// will have their own prefixes (`stub:<api>`, `settle:<domain>`).
    fn decision_key(&self) -> Option<String> {
        if self.host.is_empty() {
            return None;
        }
        match self.action {
            "skip" => Some(format!("block:{}", self.host)),
            "queued" => Some(format!("allow:{}", self.host)),
            _ => None,
        }
    }
}

// === Synthetic outcome derivation ===
//
// After navigate completes, derive a binary success/failure verdict from
// the result alone — no driver involvement required. The outcome stream
// feeds T2's Bayesian posteriors over `block:<host>` etc. (see
// docs/probabilistic-policy.md §4.5).
//
// The thresholds below are defaults; they will be tuned offline once we
// have a labeled corpus. Document each one inline so the calibration
// target is obvious in code review.

/// Strong-success: extract object must have at least this many top-level
/// keys to count as "non-trivial". 3 catches anything richer than a bare
/// {error, hint} stub but admits compact JSON-LD pages (Article schema
/// has ~6-10 keys).
const EXTRACT_OBJECT_MIN_KEYS: usize = 3;

/// Strong-success: blockmap must show at least this many semantic regions
/// AND one of them must contain interactives. 3 reflects a minimum useful
/// page shape (header + main + footer) — fewer means we got a thin shell
/// or an error page that the body+title heuristic should handle instead.
const BLOCKMAP_MIN_STRUCTURE: usize = 3;

/// Strong-failure: if more than this fraction of executed-OR-attempted
/// scripts hit the watchdog interrupt, the page is pathological (infinite
/// loop in framework init, runaway hydration). 0.5 = "majority failed for
/// the same reason" — this is the conservative threshold; a mild
/// interrupt count (1 of 50) shouldn't sink the verdict.
const SCRIPT_INTERRUPT_FAIL_RATIO: f64 = 0.5;

/// Tie-breaker (weak heuristic): minimum heading count for a page with no
/// other strong signals to still count as "got something". 1 heading is
/// the floor — many landing pages are just a hero h1.
const TIEBREAKER_MIN_HEADINGS: usize = 1;

/// Tie-breaker (weak heuristic): minimum title length for a page with no
/// other strong signals to count as "got something". 5 chars rules out
/// the empty/single-letter titles that error pages and CDN holds emit
/// while admitting normal page titles ("Home", "Login", "404"). The 404
/// inclusion is intentional — we did successfully fetch *a* page.
const TIEBREAKER_MIN_TITLE_LEN: usize = 5;

/// Pure outcome derivation. Returns `(success, reasons, signals)`.
///
/// - `success`: the binary verdict T2 will train against.
/// - `reasons`: human-readable strings describing which signals fired —
///   surfaced in the NDJSON event so the heuristic is debuggable.
/// - `signals`: a JSON object of the underlying inputs so we can
///   re-derive offline if the heuristic changes (e.g., raise a threshold
///   without re-running the whole corpus).
///
/// Pure: no I/O, no globals. Inputs are everything the navigate result
/// computes; output is fully determined by inputs. Tests in this file
/// exercise the full surface.
fn derive_outcome(
    status: u16,
    exec_scripts: bool,
    challenge: &Value,
    blockmap: &Value,
    extract: &Value,
    network_stores: &Value,
    scripts: &Value,
) -> (bool, Vec<String>, Value) {
    let mut reasons: Vec<String> = Vec::new();

    // === Read signals out of the result. Tolerate missing/null fields —
    // navigate emits Value::Null on the no-exec_scripts path, and we want
    // derive_outcome to gracefully degrade rather than panic. ===

    // Extract: present iff non-null, "non-trivial" iff object>=3 keys or
    // array with at least one structured element.
    let extract_present = !extract.is_null();
    let extract_strategy = extract
        .get("strategy")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let extract_data = extract.get("data");
    let extract_nontrivial = match extract_data {
        Some(Value::Object(m)) => m.len() >= EXTRACT_OBJECT_MIN_KEYS,
        Some(Value::Array(a)) => a
            .iter()
            .any(|e| matches!(e, Value::Object(_) | Value::Array(_))),
        _ => false,
    };
    // An extract object that's the truncated stub (data: null + truncated:
    // true) still counts as a strong success — the strategy fired, the
    // payload is just too big to inline. all_hits_summary or
    // primary_truncated metadata indicates the same shape.
    let extract_truncated = extract
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || extract
            .get("primary_truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    // Blockmap: structure count + total interactives across the structure.
    // Iterate over an empty slice on missing/malformed structure rather than
    // bind a temp `Vec<Value>` (E0716 — const can't back a `&Vec` borrow
    // that outlives the expression). [Value] is the right shape here anyway.
    let empty_slice: &[Value] = &[];
    let blockmap_structure: &[Value] = blockmap
        .get("structure")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(empty_slice);
    let blockmap_structure_count = blockmap_structure.len();
    let mut blockmap_interactives_total: u64 = 0;
    let mut blockmap_any_interactive = false;
    for s in blockmap_structure {
        if let Some(c) = s.get("counts") {
            let links = c.get("links").and_then(|v| v.as_u64()).unwrap_or(0);
            let buttons = c.get("buttons").and_then(|v| v.as_u64()).unwrap_or(0);
            let inputs = c.get("inputs").and_then(|v| v.as_u64()).unwrap_or(0);
            let sum = links + buttons + inputs;
            blockmap_interactives_total += sum;
            if sum > 0 {
                blockmap_any_interactive = true;
            }
        }
    }
    let blockmap_headings_count = blockmap
        .get("headings")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let blockmap_title_len = blockmap
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().chars().count())
        .unwrap_or(0);

    // Network stores: number of content-bearing responses captured.
    let network_capture_count = network_stores
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Scripts dict (null on the no-exec path).
    let scripts_inline = scripts
        .get("inline_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let scripts_external = scripts
        .get("external_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let scripts_executed = scripts
        .get("executed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let scripts_interrupted = scripts
        .get("interrupted")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let scripts_total = scripts_inline + scripts_external;

    let challenge_present = !challenge.is_null();
    let challenge_provider = challenge
        .get("provider")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // === Strong-failure short-circuits (any one fails the navigate). ===

    if challenge_present {
        let prov = challenge_provider
            .clone()
            .unwrap_or_else(|| "?".to_string());
        reasons.push(format!("challenge:{prov}"));
        return (
            false,
            reasons,
            build_signals(
                extract_present,
                extract_strategy,
                blockmap_structure_count,
                blockmap_interactives_total,
                network_capture_count,
                challenge_provider,
                scripts_executed,
                scripts_interrupted,
                scripts_total,
            ),
        );
    }

    if !(200..400).contains(&status) {
        reasons.push(format!("status:{status}"));
        return (
            false,
            reasons,
            build_signals(
                extract_present,
                extract_strategy,
                blockmap_structure_count,
                blockmap_interactives_total,
                network_capture_count,
                challenge_provider,
                scripts_executed,
                scripts_interrupted,
                scripts_total,
            ),
        );
    }

    // The two script-pathology checks only apply when scripts were actually
    // attempted. exec_scripts=false leaves scripts == null and these
    // signals don't contribute either way.
    if exec_scripts && !scripts.is_null() {
        if scripts_executed == 0 && scripts_total > 0 {
            reasons.push(format!(
                "scripts_all_failed:{scripts_total}_attempted_0_executed"
            ));
            return (
                false,
                reasons,
                build_signals(
                    extract_present,
                    extract_strategy,
                    blockmap_structure_count,
                    blockmap_interactives_total,
                    network_capture_count,
                    challenge_provider,
                    scripts_executed,
                    scripts_interrupted,
                    scripts_total,
                ),
            );
        }
        // "majority interrupted" requires at least one execution to compute
        // a ratio; the all-failed branch above already covers the zero case.
        if scripts_executed > 0
            && (scripts_interrupted as f64)
                > (scripts_executed as f64) * SCRIPT_INTERRUPT_FAIL_RATIO
        {
            reasons.push(format!(
                "scripts_pathological:{scripts_interrupted}_of_{scripts_executed}_interrupted"
            ));
            return (
                false,
                reasons,
                build_signals(
                    extract_present,
                    extract_strategy,
                    blockmap_structure_count,
                    blockmap_interactives_total,
                    network_capture_count,
                    challenge_provider,
                    scripts_executed,
                    scripts_interrupted,
                    scripts_total,
                ),
            );
        }
    }

    // === Strong-success signals (any one passes). ===

    if extract_present && (extract_nontrivial || extract_truncated) {
        let strat = extract_strategy.clone().unwrap_or_else(|| "?".to_string());
        let count = match extract_data {
            Some(Value::Object(m)) => m.len(),
            Some(Value::Array(a)) => a.len(),
            _ => 0,
        };
        let suffix = if extract_truncated && !extract_nontrivial {
            "_truncated".to_string()
        } else {
            format!("_with_{count}_entries")
        };
        reasons.push(format!("extract:{strat}{suffix}"));
    }

    if blockmap_structure_count >= BLOCKMAP_MIN_STRUCTURE && blockmap_any_interactive {
        reasons.push(format!(
            "blockmap:{blockmap_structure_count}_structure_{blockmap_interactives_total}_interactives"
        ));
    }

    if network_capture_count > 0 {
        reasons.push(format!("network_stores:{network_capture_count}_captures"));
    }

    if !reasons.is_empty() {
        return (
            true,
            reasons,
            build_signals(
                extract_present,
                extract_strategy,
                blockmap_structure_count,
                blockmap_interactives_total,
                network_capture_count,
                challenge_provider,
                scripts_executed,
                scripts_interrupted,
                scripts_total,
            ),
        );
    }

    // === Tie-breaker (weak heuristic). ===
    // No strong signals fired. Fall back to "did we get *any* page": at
    // least one heading OR a non-trivial title. Documented as weak so
    // anyone reading the trace knows the verdict is low-confidence.
    if blockmap_headings_count >= TIEBREAKER_MIN_HEADINGS
        || blockmap_title_len >= TIEBREAKER_MIN_TITLE_LEN
    {
        if blockmap_headings_count >= TIEBREAKER_MIN_HEADINGS {
            reasons.push(format!(
                "title_only:{blockmap_headings_count}_headings_weak"
            ));
        } else {
            reasons.push(format!("title_only:{blockmap_title_len}char_title_weak"));
        }
        return (
            true,
            reasons,
            build_signals(
                extract_present,
                extract_strategy,
                blockmap_structure_count,
                blockmap_interactives_total,
                network_capture_count,
                challenge_provider,
                scripts_executed,
                scripts_interrupted,
                scripts_total,
            ),
        );
    }

    reasons.push("no_signal".to_string());
    (
        false,
        reasons,
        build_signals(
            extract_present,
            extract_strategy,
            blockmap_structure_count,
            blockmap_interactives_total,
            network_capture_count,
            challenge_provider,
            scripts_executed,
            scripts_interrupted,
            scripts_total,
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_signals(
    extract_present: bool,
    extract_strategy: Option<String>,
    blockmap_structure_count: usize,
    blockmap_interactives_total: u64,
    network_capture_count: u64,
    challenge_provider: Option<String>,
    scripts_executed: u64,
    scripts_interrupted: u64,
    scripts_total: u64,
) -> Value {
    json!({
        "extract_present": extract_present,
        "extract_strategy": extract_strategy,
        "blockmap_structure_count": blockmap_structure_count,
        "blockmap_interactives_total": blockmap_interactives_total,
        "network_capture_count": network_capture_count,
        "challenge": challenge_provider,
        "scripts_executed": scripts_executed,
        "scripts_interrupted": scripts_interrupted,
        "scripts_total": scripts_total,
    })
}

fn score_to_probabilities(mut scores: Vec<(&'static str, f64)>) -> (Value, f64, f64) {
    if scores.is_empty() {
        return (Value::Object(serde_json::Map::new()), 0.0, 0.0);
    }

    let max_score = scores
        .iter()
        .map(|(_, score)| *score)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut total = 0.0;
    for (_, score) in &mut scores {
        *score = (*score - max_score).exp();
        total += *score;
    }

    let mut map = serde_json::Map::new();
    let mut top_prob = 0.0;
    let mut second_prob = 0.0;
    for (name, score) in scores {
        let prob = if total > 0.0 { score / total } else { 0.0 };
        if prob > top_prob {
            second_prob = top_prob;
            top_prob = prob;
        } else if prob > second_prob {
            second_prob = prob;
        }
        map.insert(name.to_string(), Value::from(prob));
    }
    (
        Value::Object(map),
        top_prob,
        (top_prob - second_prob).max(0.0),
    )
}

fn normalized_count(count: u64, scale: f64) -> f64 {
    if count == 0 || scale <= 0.0 {
        0.0
    } else {
        1.0 - (-(count as f64) / scale).exp()
    }
}

fn bool_score(flag: bool) -> f64 {
    if flag { 1.0 } else { 0.0 }
}

fn derive_tool_likelihoods(
    status: u16,
    exec_scripts: bool,
    blockmap: &Value,
    extract: &Value,
    network_stores: &Value,
    challenge: &Value,
    scripts: &Value,
) -> Value {
    // These scales are soft saturation points: a count near the scale value
    // contributes ~63% of the feature's max, and larger counts taper off.
    // The goal is to keep page-local signals comparable without letting one
    // noisy count dominate the ranking.
    let empty_slice: &[Value] = &[];
    let structure: &[Value] = blockmap
        .get("structure")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(empty_slice);
    let headings = blockmap
        .get("headings")
        .and_then(|v| v.as_array())
        .map(|a| a.len() as u64)
        .unwrap_or(0);
    // v2: main_headings was removed; derive from headings where chrome=false
    let main_headings = blockmap
        .get("headings")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter(|h| !h.get("chrome").and_then(|c| c.as_bool()).unwrap_or(false))
                .count() as u64
        })
        .unwrap_or(0);

    let interactives = blockmap.get("interactives").unwrap_or(&Value::Null);
    let links = interactives
        .get("links")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let buttons = interactives
        .get("buttons")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let inputs = interactives
        .get("inputs")
        .and_then(|v| v.as_array())
        .map(|a| a.len() as u64)
        .unwrap_or(0);
    let forms = interactives
        .get("forms")
        .and_then(|v| v.as_array())
        .map(|a| a.len() as u64)
        .unwrap_or(0);

    let density = blockmap.get("density").unwrap_or(&Value::Null);
    let tables = density.get("tables").unwrap_or(&Value::Null);
    let table_total = tables.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
    let table_filled = tables.get("filled").and_then(|v| v.as_u64()).unwrap_or(0);
    let table_ratio = tables
        .get("ratio")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| {
            if table_total > 0 {
                table_filled as f64 / table_total as f64
            } else {
                0.0
            }
        });
    let td = density.get("td").unwrap_or(&Value::Null);
    let td_total = td.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
    let td_ratio = td.get("ratio").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let li = density.get("li").unwrap_or(&Value::Null);
    let li_total = li.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
    let li_ratio = li.get("ratio").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let json_scripts = density
        .get("json_scripts")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let thin_shell = density
        .get("thin_shell")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let likely_js_filled = density
        .get("likely_js_filled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let script_heavy_shell = density
        .get("script_heavy_shell")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let script_tags = density
        .get("script_tags")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let selectors = blockmap.get("selectors").unwrap_or(&Value::Null);
    let data_testid = selectors
        .get("data_testid")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let aria_label = selectors
        .get("aria_label")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let role = selectors.get("role").and_then(|v| v.as_u64()).unwrap_or(0);

    let extract_confidence = extract
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let extract_truncated = extract
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || extract.get("primary_truncated").is_some();

    let network_capture_count = network_stores
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let network_total_bytes = network_stores
        .get("total_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let scripts_executed = scripts
        .get("executed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let scripts_interrupted = scripts
        .get("interrupted")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let scripts_total = scripts
        .get("inline_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        + scripts
            .get("external_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    let script_pathology = if exec_scripts && scripts_total > 0 {
        scripts_interrupted as f64 / scripts_total as f64
    } else {
        0.0
    };

    let challenge_score = if challenge.is_null() {
        0.0
    } else {
        challenge
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0)
            .max(0.5)
    };

    // Structure / semantics scales.
    let page_structure = normalized_count(structure.len() as u64, 3.0);
    let heading_signal = normalized_count(headings, 6.0);
    let main_heading_signal = normalized_count(main_headings, 4.0);
    // Selector / interaction scales.
    let selector_signal = normalized_count(data_testid + aria_label + role, 20.0);
    let data_testid_signal = normalized_count(data_testid, 12.0);
    let link_signal = normalized_count(links, 20.0);
    let button_signal = normalized_count(buttons, 4.0);
    let input_signal = normalized_count(inputs, 2.0);
    let form_signal = normalized_count(forms, 1.0);
    // Dense content / data scales.
    let table_signal = if table_total > 0 { table_ratio } else { 0.0 };
    let td_signal = if td_total > 0 { td_ratio } else { 0.0 };
    let list_signal = if li_total > 0 { li_ratio } else { 0.0 };
    let json_signal = normalized_count(json_scripts, 1.0);
    let script_count_signal = normalized_count(script_tags, 25.0);
    let shell_signal = bool_score(thin_shell || likely_js_filled || script_heavy_shell);
    let network_signal = normalized_count(network_capture_count, 1.0);
    let network_bytes_signal = normalized_count(network_total_bytes, 5_000.0);
    let status_signal = if (200..400).contains(&status) {
        0.0
    } else {
        1.0
    };

    let query_score = 0.15
        + 1.0 * page_structure
        + 0.55 * heading_signal
        + 0.35 * main_heading_signal
        + 0.55 * data_testid_signal
        + 0.25 * link_signal
        + 0.20 * button_signal
        + 0.10 * input_signal
        - 0.85 * bool_score(thin_shell)
        - 0.70 * bool_score(likely_js_filled)
        - 0.85 * bool_score(script_heavy_shell)
        - 1.10 * challenge_score;

    let query_text_score = 0.20
        + 0.70 * page_structure
        + 0.90 * heading_signal
        + 1.05 * main_heading_signal
        + 0.85 * selector_signal
        + 0.20 * link_signal
        - 0.35 * bool_score(thin_shell)
        - 0.20 * bool_score(likely_js_filled)
        - 0.55 * bool_score(script_heavy_shell)
        - 0.25 * challenge_score;

    let text_main_score = 0.15
        + 1.00 * main_heading_signal
        + 0.60 * page_structure
        + 0.20 * heading_signal
        + 0.10 * selector_signal
        - 0.45 * bool_score(thin_shell)
        - 0.30 * bool_score(likely_js_filled)
        - 0.55 * bool_score(script_heavy_shell)
        - 0.15 * challenge_score;

    let extract_score = 0.10
        + 1.05 * extract_confidence
        + 0.60 * json_signal
        + 0.20 * network_signal
        + 0.15 * network_bytes_signal
        + if extract_truncated { 0.25 } else { 0.0 }
        - 0.20 * bool_score(thin_shell)
        - 0.15 * challenge_score;

    let extract_table_score = 0.05
        + 1.35 * table_signal
        + 0.25 * normalized_count(table_total, 2.0)
        + 0.20 * td_signal
        + 0.10 * page_structure
        - 0.10 * bool_score(thin_shell)
        - 0.05 * challenge_score;

    let extract_list_score = 0.05
        + 1.25 * list_signal
        + 0.25 * normalized_count(li_total, 20.0)
        + 0.25 * main_heading_signal
        + 0.10 * page_structure
        + 0.10 * selector_signal
        - 0.10 * bool_score(thin_shell)
        - 0.05 * challenge_score;

    let extract_cards_score = 0.05
        + 1.35 * list_signal
        + 0.30 * normalized_count(li_total, 20.0)
        + 0.30 * main_heading_signal
        + 0.20 * selector_signal
        + 0.10 * page_structure
        - 0.10 * bool_score(thin_shell)
        - 0.05 * challenge_score;

    let network_stores_score = 0.05
        + 1.10 * network_signal
        + 0.15 * network_bytes_signal
        + 0.30 * json_signal
        + 0.10 * normalized_count(scripts_executed, 3.0)
        - 0.10 * challenge_score;

    let click_score = 0.10
        + 0.95 * link_signal
        + 0.75 * button_signal
        + 0.15 * page_structure
        + 0.10 * selector_signal
        - 0.10 * challenge_score;

    let type_score = 0.05 + 1.20 * input_signal + 0.85 * form_signal + 0.10 * selector_signal
        - 0.05 * challenge_score;

    let submit_score = 0.05 + 1.30 * form_signal + 0.60 * input_signal + 0.10 * page_structure
        - 0.05 * challenge_score;

    // Route discovery is favored by concrete route/form affordances, but it is
    // deliberately discounted for thin shells and JS-filled placeholders where
    // a route graph would likely be empty or misleading.
    let route_discover_score = 0.08
        + 0.70 * link_signal
        + 1.05 * form_signal
        + 0.35 * input_signal
        + 0.15 * page_structure
        + 0.10 * selector_signal
        - 0.25 * bool_score(thin_shell)
        - 0.20 * bool_score(likely_js_filled)
        - 0.40 * bool_score(script_heavy_shell)
        - 0.35 * challenge_score;

    let chrome_escalation_score = 0.02
        + 1.90 * challenge_score
        + 0.95 * bool_score(thin_shell)
        + 0.80 * bool_score(likely_js_filled)
        + 1.05 * bool_score(script_heavy_shell)
        + 0.25 * script_count_signal * shell_signal
        + 0.60 * script_pathology
        + 0.40 * status_signal;

    let ordered = vec![
        ("query", query_score),
        ("query_text", query_text_score),
        ("text_main", text_main_score),
        ("extract", extract_score),
        ("extract_table", extract_table_score),
        ("extract_list", extract_list_score),
        ("extract_cards", extract_cards_score),
        ("network_stores", network_stores_score),
        ("click", click_score),
        ("type", type_score),
        ("submit", submit_score),
        ("route_discover", route_discover_score),
        ("chrome_escalation", chrome_escalation_score),
    ];

    let mut top = ordered.clone();
    top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (tool_likelihoods, confidence, margin) = score_to_probabilities(ordered);

    json!({
        "confidence": confidence,
        "margin": margin,
        "tool_likelihoods": tool_likelihoods,
        "tool_recommendations": top.into_iter().map(|(name, _)| name).collect::<Vec<_>>(),
    })
}

fn apply_browser_route_tool_advice(tool_advice: &mut Value, browser_route: &Option<Value>) {
    let Some(route) = browser_route else {
        return;
    };
    if !route
        .get("needed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return;
    }
    let confidence = route
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let reason = route.get("reason").and_then(|v| v.as_str()).unwrap_or("");
    let threshold = if reason == "missing_primary_action" {
        challenge::MISSING_PRIMARY_ACTION_ESCALATION_THRESHOLD
    } else {
        0.70
    };
    if confidence < threshold {
        return;
    }
    if let Some(likelihoods) = tool_advice
        .get_mut("tool_likelihoods")
        .and_then(|v| v.as_object_mut())
    {
        let current = likelihoods
            .get("chrome_escalation")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        likelihoods.insert("chrome_escalation".into(), json!(current.max(confidence)));
    }
    if let Some(recs) = tool_advice
        .get_mut("tool_recommendations")
        .and_then(|v| v.as_array_mut())
    {
        recs.retain(|v| v.as_str() != Some("chrome_escalation"));
        recs.insert(0, Value::String("chrome_escalation".into()));
    }
}

// Phase A: validated outcome reporting. Shared between rpc_main and
// dispatch_tool so the validation and event shape stay canonical. v0
// just emits the NDJSON event — no posterior updates yet (see
// docs/probabilistic-policy.md §4.5).
//
// Returns Err with a human-readable message on schema violations.
// Unknown nav_id is rejected so an outcome can never silently corrupt
// future posterior attribution.
const TASK_CLASS_ENUM: &[&str] = &["extract", "query", "click", "form", "visual"];

fn validate_and_emit_outcome(
    session: &Session,
    params: &Value,
    nav_id: &str,
) -> std::result::Result<(), String> {
    if nav_id.is_empty() {
        return Err("missing 'navigation_id' param".to_string());
    }
    if !session.nav_id_is_known(nav_id) {
        return Err(format!(
            "unknown navigation_id '{nav_id}' — never issued by this session"
        ));
    }
    // success is required by the schema. Missing → reject (don't default to false).
    let success = match params.get("success") {
        Some(v) => v
            .as_bool()
            .ok_or_else(|| "'success' must be boolean".to_string())?,
        None => return Err("missing required 'success' param".to_string()),
    };
    let task_class = match params.get("task_class") {
        Some(Value::Null) | None => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "'task_class' must be string".to_string())?;
            if !TASK_CLASS_ENUM.contains(&s) {
                return Err(format!(
                    "'task_class' must be one of {TASK_CLASS_ENUM:?}, got '{s}'"
                ));
            }
            Some(s)
        }
    };
    let task_id = params.get("task_id").and_then(|v| v.as_str());
    let quality = params.get("quality").and_then(|v| v.as_f64());
    let error = params.get("error").and_then(|v| v.as_str());
    emit_event(
        "outcome_reported",
        json!({
            "schema_version": 1,
            "navigation_id": nav_id,
            "task_id": task_id,
            "task_class": task_class,
            "success": success,
            "quality": quality,
            "error": error,
        }),
    );
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().skip(1).any(|a| a == "--version" || a == "-V") {
        println!("unbrowser {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Some(i) = command_index(&args, "--session-daemon") {
        return session_daemon_cmd(&args, i).await;
    }
    if args.iter().skip(1).any(|a| is_help_flag(a)) {
        if command_index(&args, "navigate").is_some() {
            print_navigate_usage();
        } else if command_index(&args, "session").is_some()
            || command_index(&args, "exec").is_some()
        {
            print_session_usage();
        } else if command_index(&args, "policy-check").is_some() {
            print_policy_check_usage();
        } else {
            print_usage();
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--list-profiles") {
        for n in profile::Profile::list_builtin() {
            println!("{n}");
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--prefit-info") {
        match prefit::PrefitBundle::load_embedded() {
            Some(b) => {
                println!("schema_version: {}", b.schema_version);
                println!("training_pipeline_version: {}", b.training_pipeline_version);
                println!("fit_timestamp: {}", b.fit_timestamp);
                println!("fit_corpus_size: {}", b.fit_corpus_size);
                println!("domains: {}", b.domain_count());
                let mut keys: Vec<_> = b.domains.keys().collect();
                keys.sort();
                for k in keys {
                    if let Some(d) = b.domains.get(k) {
                        println!(
                            "  {:30} framework={:20} blocklist_additions={:3} shape={}",
                            k,
                            d.framework.as_deref().unwrap_or("-"),
                            d.blocklist_additions.len(),
                            d.shape_hint.as_deref().unwrap_or("-")
                        );
                    }
                }
                return Ok(());
            }
            None => {
                eprintln!("prefit: failed to load embedded bundle");
                std::process::exit(2);
            }
        }
    }
    if args.get(1).map(|s| s.as_str()) == Some("policy-check") {
        return policy_check_cmd(&args[2..]);
    }
    if let Some(i) = command_index(&args, "session") {
        return session_cmd(&args, i).await;
    }
    if let Some(i) = command_index(&args, "exec") {
        return session_exec_cmd(&args, i);
    }
    if let Some(i) = command_index(&args, "navigate") {
        return navigate_cmd(&args, i).await;
    }
    let profile_name = parse_profile_arg(&args);
    let profile = Profile::load(&profile_name)?;
    if args.iter().any(|a| a == "--mcp") {
        mcp_main(profile).await
    } else {
        rpc_main(profile).await
    }
}

fn is_help_flag(arg: &str) -> bool {
    arg == "-h" || arg == "--help"
}

fn command_index(args: &[String], command: &str) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == command {
            return Some(i);
        }
        if arg == "--profile" || arg == "--shims" || arg == "--mcp-profile" {
            i += 2;
        } else if arg.starts_with("--profile=")
            || arg.starts_with("--shims=")
            || arg.starts_with("--mcp-profile=")
        {
            i += 1;
        } else {
            i += 1;
        }
    }
    None
}

fn print_usage() {
    println!(
        r#"unbrowser {}

Usage:
  unbrowser [--profile <name>] [--policy=blocklist] [--shims stable|enhanced] [--mcp] [--mcp-profile minimal|full]
  unbrowser navigate <url> [--exec-scripts] [--json] [--events] [--shims stable|enhanced]
  unbrowser session start [--id <id>] [--profile <name>] [--policy=blocklist] [--shims stable|enhanced]
  unbrowser session exec [--pretty] <id|socket> <method> [params-json | shorthand args]
  unbrowser exec [--pretty] <id|socket> <method> [params-json | shorthand args]
  unbrowser session stop <id|socket>
  unbrowser session list
  unbrowser session prune
  unbrowser policy-check <url> [<url>...]
  unbrowser --list-profiles
  unbrowser --prefit-info
  unbrowser --version

Default with no subcommand starts JSON-RPC over stdin/stdout.
Use --mcp for Model Context Protocol server mode.
Use session mode for shell-friendly persistent state across commands.
"#,
        env!("CARGO_PKG_VERSION")
    );
}

fn print_navigate_usage() {
    println!(
        r#"Usage:
  unbrowser navigate <url> [--exec-scripts] [--json] [--events] [--shims stable|enhanced]

Fetch one URL, print the navigate result as JSON, then exit.

Options:
  --exec-scripts   Run page scripts before returning
  --json           Accepted for compatibility; JSON is always emitted
  --events         Keep NDJSON observability events on stderr
  --shims MODE     Runtime shim mode for A/B tests: stable (default) or enhanced
"#
    );
}

fn print_policy_check_usage() {
    println!(
        r#"Usage:
  unbrowser policy-check <url> [<url>...]
  unbrowser policy-check --info
"#
    );
}

fn print_session_usage() {
    println!(
        r#"Usage:
  unbrowser session start [--id <id>] [--profile <name>] [--policy=blocklist] [--shims stable|enhanced]
  unbrowser session exec [--pretty] <id|socket> <method> [params-json | shorthand args]
  unbrowser exec [--pretty] <id|socket> <method> [params-json | shorthand args]
  unbrowser session stop <id|socket>
  unbrowser session list
  unbrowser session prune

Examples:
  unbrowser session start
  unbrowser session exec ub_123 navigate https://news.ycombinator.com
  unbrowser exec ub_123 query '.titleline > a'
  unbrowser exec --pretty ub_123 blockmap
  unbrowser exec ub_123 eval 'document.title'
  unbrowser session stop ub_123

If the final argument after <method> is a JSON object, it is used as params.
Otherwise common methods accept shorthand args: navigate <url>, query <selector>,
query_debug <selector>, text [selector], click <ref>, type <ref> <text>,
submit <ref>, eval <code>, table_to_json [selector].
"#
    );
}

// `unbrowser policy-check <url> [<url>...]`
//
// Prints the policy decision for one or more URLs. Used to verify the
// blocklist against ad-hoc URLs and to drive scripts/policy_baseline.py
// without round-tripping through navigate. Pure stdlib + policy module —
// no JS engine, no HTTP.
fn policy_check_cmd(urls: &[String]) -> Result<()> {
    if urls.is_empty() {
        print_policy_check_usage();
        std::process::exit(2);
    }
    if urls.iter().any(|u| u == "--info") {
        println!("entries: {}", policy::entry_count());
        return Ok(());
    }
    for url in urls {
        let d = policy::decide(url);
        let host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "<unparsed>".to_string());
        if d.blocked {
            println!(
                "block\t{}\t{}\t{}\t{}",
                d.category.map(|c| c.as_str()).unwrap_or("?"),
                d.matched_pattern.unwrap_or("?"),
                host,
                url
            );
        } else {
            println!("allow\t-\t-\t{}\t{}", host, url);
        }
    }
    Ok(())
}

struct NavigateCliArgs {
    url: String,
    exec_scripts: bool,
    events: bool,
    include_ascii: bool,
    compact: bool,
}

async fn navigate_cmd(args: &[String], command_i: usize) -> Result<()> {
    let cli = parse_navigate_cli_args(args, command_i)?;
    let profile_name = parse_profile_arg(args);
    let profile = Profile::load(&profile_name)?;
    let policy_block = parse_policy_arg(args);
    let shim_mode = parse_shim_mode_arg(args)?;
    let previous_events = EVENTS_ENABLED.swap(cli.events, Ordering::Relaxed);
    let mut session = Session::new(&profile, policy_block, shim_mode)?;
    let result = session
        .navigate_opts(&cli.url, cli.exec_scripts, cli.include_ascii)
        .await;
    EVENTS_ENABLED.store(previous_events, Ordering::Relaxed);
    if cli.compact {
        println!("{}", serde_json::to_string(&result?)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&result?)?);
    }
    Ok(())
}

fn parse_navigate_cli_args(args: &[String], command_i: usize) -> Result<NavigateCliArgs> {
    let mut url = None;
    let mut exec_scripts = false;
    let mut events = false;
    let mut include_ascii = false;
    let mut compact = false;
    let mut i = 1;
    while i < args.len() {
        if i == command_i {
            i += 1;
            continue;
        }
        let arg = &args[i];
        if arg == "--profile" {
            i += 2;
            continue;
        }
        if arg == "--shims" {
            i += 2;
            continue;
        }
        if arg.starts_with("--profile=")
            || arg.starts_with("--shims=")
            || arg == "--policy=blocklist"
            || arg == "--policy=on"
        {
            i += 1;
            continue;
        }
        if arg == "--exec-scripts" {
            exec_scripts = true;
            i += 1;
            continue;
        }
        if arg == "--events" {
            events = true;
            i += 1;
            continue;
        }
        if arg == "--ascii" {
            include_ascii = true;
            i += 1;
            continue;
        }
        if arg == "--compact" || arg == "--json" {
            compact = true;
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            return Err(anyhow!("unknown navigate option: {arg}"));
        }
        if url.replace(arg.clone()).is_some() {
            return Err(anyhow!("unexpected extra navigate argument: {arg}"));
        }
        i += 1;
    }
    let url = url.ok_or_else(|| anyhow!("missing URL. Run `unbrowser navigate --help`."))?;
    Ok(NavigateCliArgs {
        url,
        exec_scripts,
        events,
        include_ascii,
        compact,
    })
}

fn session_dir() -> PathBuf {
    std::env::var("UNBROWSER_SESSION_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/unbrowser-sessions"))
}

fn generate_session_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("ub_{}_{}", std::process::id(), ms)
}

fn validate_session_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(anyhow!("session id cannot be empty"));
    }
    if id.len() > 64 {
        return Err(anyhow!("session id is too long (max 64 chars)"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(anyhow!(
            "session id may only contain ASCII letters, digits, '.', '_' or '-'"
        ));
    }
    Ok(())
}

fn session_socket_for(id_or_socket: &str) -> Result<PathBuf> {
    if id_or_socket.contains('/') {
        return Ok(PathBuf::from(id_or_socket));
    }
    validate_session_id(id_or_socket)?;
    Ok(session_dir().join(format!("{id_or_socket}.sock")))
}

fn ensure_private_session_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).context("create session dir")?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .context("set session dir permissions")?;
    Ok(())
}

fn parse_session_start_id(args: &[String], session_i: usize) -> Result<String> {
    let mut id = None;
    let mut i = session_i + 2;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--id requires a value"))?;
                id = Some(value.clone());
                i += 2;
            }
            a if a.starts_with("--id=") => {
                id = Some(a.trim_start_matches("--id=").to_string());
                i += 1;
            }
            "--profile" | "--shims" => i += 2,
            a if a.starts_with("--profile=")
                || a.starts_with("--shims=")
                || a == "--policy=blocklist"
                || a == "--policy=on" =>
            {
                i += 1;
            }
            other => return Err(anyhow!("unknown session start option: {other}")),
        }
    }
    let id = id.unwrap_or_else(generate_session_id);
    validate_session_id(&id)?;
    Ok(id)
}

fn session_start_cmd(args: &[String], session_i: usize) -> Result<()> {
    let id = parse_session_start_id(args, session_i)?;
    let dir = session_dir();
    ensure_private_session_dir(&dir)?;
    let socket = dir.join(format!("{id}.sock"));
    if socket.exists() {
        return Err(anyhow!(
            "session socket already exists: {} (stop it or choose --id)",
            socket.display()
        ));
    }

    let profile_name = parse_profile_arg(args);
    let shim_mode = parse_shim_mode_arg(args)?;
    let policy_block = parse_policy_arg(args);
    let exe = std::env::current_exe().context("resolve current executable")?;
    let mut cmd = Command::new(exe);
    cmd.arg("--session-daemon")
        .arg(&socket)
        .arg("--profile")
        .arg(&profile_name)
        .arg("--shims")
        .arg(shim_mode.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if policy_block {
        cmd.arg("--policy=blocklist");
    }
    let mut child = cmd.spawn().context("spawn session daemon")?;

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().context("check session daemon")? {
            return Err(anyhow!("session daemon exited early with status {status}"));
        }
        if session_rpc_call(&socket, "ping", json!({})).is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "session daemon did not become ready at {}",
                socket.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    println!(
        "{}",
        serde_json::to_string(&json!({
            "id": id,
            "socket": socket,
            "pid": child.id(),
            "profile": profile_name,
            "shim_mode": shim_mode.as_str(),
        }))?
    );
    Ok(())
}

fn parse_u32_flag(args: &[String], flag: &str, default: u32) -> Result<u32> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            let raw = args
                .get(i + 1)
                .ok_or_else(|| anyhow!("{flag} requires a value"))?;
            return raw
                .parse::<u32>()
                .map_err(|e| anyhow!("invalid {flag} value: {e}"));
        }
        if let Some(raw) = args[i].strip_prefix(&format!("{flag}=")) {
            return raw
                .parse::<u32>()
                .map_err(|e| anyhow!("invalid {flag} value: {e}"));
        }
        i += 1;
    }
    Ok(default)
}

fn first_shorthand_positional<'a>(args: &'a [String], value_flags: &[&str]) -> Option<&'a String> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if value_flags.iter().any(|flag| arg == flag) {
            i += 2;
            continue;
        }
        if value_flags
            .iter()
            .any(|flag| arg.starts_with(&format!("{flag}=")))
        {
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(arg);
    }
    None
}

fn parse_session_exec_params(method: &str, raw: &[String]) -> Result<Value> {
    if raw.is_empty() {
        return Ok(json!({}));
    }
    if raw.len() == 1 && raw[0].trim_start().starts_with('{') {
        let v: Value = serde_json::from_str(&raw[0]).context("parse params JSON")?;
        if !v.is_object() {
            return Err(anyhow!("params JSON must be an object"));
        }
        return Ok(v);
    }

    match method {
        "navigate" => {
            let url = raw
                .iter()
                .find(|a| !a.starts_with('-'))
                .ok_or_else(|| anyhow!("navigate shorthand requires <url>"))?;
            Ok(json!({
                "url": url,
                "exec_scripts": raw.iter().any(|a| a == "--exec-scripts")
            }))
        }
        "query" => Ok(json!({
            "selector": raw.first().ok_or_else(|| anyhow!("query shorthand requires <selector>"))?
        })),
        "query_debug" => Ok(json!({
            "selector": raw.first().ok_or_else(|| anyhow!("query_debug shorthand requires <selector>"))?,
            "limit": parse_u32_flag(raw, "--limit", 10)?
        })),
        "text" => Ok(raw
            .first()
            .map(|selector| json!({ "selector": selector }))
            .unwrap_or_else(|| json!({}))),
        "find_text" | "query_text" => Ok(json!({
            "text": raw.first().ok_or_else(|| anyhow!("{method} shorthand requires <text>"))?,
            "exact": raw.iter().any(|a| a == "--exact"),
            "limit": parse_u32_flag(raw, "--limit", 20)?
        })),
        "click" | "submit" => Ok(json!({
            "ref": raw.first().ok_or_else(|| anyhow!("{method} shorthand requires <ref>"))?
        })),
        "type" => {
            if raw.len() < 2 {
                return Err(anyhow!("type shorthand requires <ref> <text>"));
            }
            Ok(json!({ "ref": raw[0], "text": raw[1..].join(" ") }))
        }
        "eval" => Ok(json!({ "code": raw.join(" ") })),
        "extract" => Ok(raw
            .first()
            .map(|strategy| json!({ "strategy": strategy }))
            .unwrap_or_else(|| json!({}))),
        "extract_table" | "table_to_json" => Ok(raw
            .first()
            .map(|selector| json!({ "selector": selector }))
            .unwrap_or_else(|| json!({}))),
        "extract_cards" => Ok(json!({
            "selector": first_shorthand_positional(raw, &["--limit"]),
            "limit": parse_u32_flag(raw, "--limit", 50)?
        })),
        "blockmap" | "body" | "text_main" | "cookies_get" | "cookies_clear" | "ping" | "close" => {
            if raw.is_empty() {
                Ok(json!({}))
            } else {
                Err(anyhow!(
                    "{method} does not accept shorthand args; pass a JSON object instead"
                ))
            }
        }
        _ => Err(anyhow!(
            "unknown shorthand for method '{method}'; pass params as a JSON object"
        )),
    }
}

fn session_rpc_call(socket: &Path, method: &str, params: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("connect session socket {}", socket.display()))?;
    let req = json!({ "id": 1, "method": method, "params": params });
    writeln!(stream, "{}", serde_json::to_string(&req)?)?;
    stream.flush()?;

    let mut reader = StdBufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim().is_empty() {
        return Err(anyhow!("session daemon closed without a response"));
    }
    let resp: Value = serde_json::from_str(&line).context("parse session response")?;
    if let Some(err) = resp.get("error") {
        let msg = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("session command failed");
        return Err(anyhow!(msg.to_string()));
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

fn session_dispatch_error_code(message: &str) -> i32 {
    if message.starts_with("unknown tool:") {
        -32601
    } else {
        -32602
    }
}

fn session_exec_cmd(args: &[String], exec_i: usize) -> Result<()> {
    let mut i = exec_i + 1;
    let pretty = args.get(i).map(|s| s.as_str()) == Some("--pretty");
    if pretty {
        i += 1;
    }
    let id_or_socket = args
        .get(i)
        .ok_or_else(|| anyhow!("missing session id/socket. Run `unbrowser session start`."))?;
    let method = args
        .get(i + 1)
        .ok_or_else(|| anyhow!("missing method after session id"))?;
    let params = parse_session_exec_params(method, &args[(i + 2)..])?;
    let socket = session_socket_for(id_or_socket)?;
    let result = session_rpc_call(&socket, method, params)?;
    if pretty {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", serde_json::to_string(&result)?);
    }
    Ok(())
}

fn session_stop_cmd(args: &[String], stop_i: usize) -> Result<()> {
    let id_or_socket = args
        .get(stop_i + 1)
        .ok_or_else(|| anyhow!("missing session id/socket"))?;
    let socket = session_socket_for(id_or_socket)?;
    let _ = session_rpc_call(&socket, "close", json!({}))?;
    println!(
        "{}",
        serde_json::to_string(&json!({ "stopped": true, "socket": socket }))?
    );
    Ok(())
}

fn session_list_cmd() -> Result<()> {
    let dir = session_dir();
    let mut sessions = Vec::new();
    if dir.exists() {
        for entry in std::fs::read_dir(&dir).context("read session dir")? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) != Some("sock") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_string();
            let alive = session_rpc_call(&path, "ping", json!({})).is_ok();
            sessions.push(json!({ "id": id, "socket": path, "alive": alive }));
        }
    }
    println!("{}", serde_json::to_string(&sessions)?);
    Ok(())
}

fn session_prune_cmd() -> Result<()> {
    let dir = session_dir();
    let mut removed = Vec::new();
    let mut kept = 0usize;
    if dir.exists() {
        for entry in std::fs::read_dir(&dir).context("read session dir")? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|v| v.to_str()) != Some("sock") {
                continue;
            }
            if session_rpc_call(&path, "ping", json!({})).is_ok() {
                kept += 1;
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_string();
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            removed.push(json!({ "id": id, "socket": path }));
        }
    }
    println!(
        "{}",
        serde_json::to_string(&json!({
            "pruned": removed.len(),
            "kept": kept,
            "removed": removed,
        }))?
    );
    Ok(())
}

async fn session_cmd(args: &[String], session_i: usize) -> Result<()> {
    let sub = args
        .get(session_i + 1)
        .map(|s| s.as_str())
        .unwrap_or("help");
    match sub {
        "start" => session_start_cmd(args, session_i),
        "exec" => session_exec_cmd(args, session_i + 1),
        "stop" => session_stop_cmd(args, session_i + 1),
        "list" => session_list_cmd(),
        "prune" => session_prune_cmd(),
        "help" | "--help" | "-h" => {
            print_session_usage();
            Ok(())
        }
        other => Err(anyhow!("unknown session command: {other}")),
    }
}

// `--profile <name>` or `--profile=<name>`. Falls back to UNBROWSER_PROFILE
// env var, then the built-in default.
fn parse_profile_arg(args: &[String]) -> String {
    for (i, a) in args.iter().enumerate() {
        if a == "--profile" {
            if let Some(next) = args.get(i + 1) {
                return next.clone();
            }
        } else if let Some(rest) = a.strip_prefix("--profile=") {
            return rest.to_string();
        }
    }
    std::env::var("UNBROWSER_PROFILE").unwrap_or_else(|_| profile::DEFAULT_PROFILE.to_string())
}

fn parse_mcp_profile_arg(args: &[String]) -> String {
    for (i, a) in args.iter().enumerate() {
        if a == "--mcp-profile" {
            if let Some(next) = args.get(i + 1) {
                return next.to_ascii_lowercase();
            }
        } else if let Some(rest) = a.strip_prefix("--mcp-profile=") {
            return rest.to_ascii_lowercase();
        }
    }
    std::env::var("UNBROWSER_MCP_PROFILE")
        .map(|v| v.to_ascii_lowercase())
        .unwrap_or_else(|_| "full".to_string())
}

// `--policy=blocklist` enables Tier 1 deterministic blocking at the
// `__host_fetch_send` layer. Off by default — opt-in for v0 until the
// corpus measurement validates no extraction-quality regression. Env var
// UNBROWSER_POLICY=blocklist also flips it on for ad-hoc shell use.
fn parse_policy_arg(args: &[String]) -> bool {
    if args
        .iter()
        .any(|a| a == "--policy=blocklist" || a == "--policy=on")
    {
        return true;
    }
    std::env::var("UNBROWSER_POLICY")
        .map(|v| v == "blocklist" || v == "on")
        .unwrap_or(false)
}

// `--shims <mode>` or `--shims=<mode>`. Falls back to UNBROWSER_SHIMS.
// Stable preserves the current release behavior; enhanced is opt-in for A/B
// runs because it intentionally takes content-positive guesses about layout,
// viewport, and storage APIs that a no-rendering browser can't know exactly.
fn parse_shim_mode_arg(args: &[String]) -> Result<ShimMode> {
    for (i, a) in args.iter().enumerate() {
        if a == "--shims" {
            let next = args
                .get(i + 1)
                .ok_or_else(|| anyhow!("--shims requires a value: stable or enhanced"))?;
            return ShimMode::parse(next);
        } else if let Some(rest) = a.strip_prefix("--shims=") {
            return ShimMode::parse(rest);
        }
    }
    match std::env::var("UNBROWSER_SHIMS") {
        Ok(v) => ShimMode::parse(&v),
        Err(_) => Ok(ShimMode::Stable),
    }
}

// Per-RPC wall-clock budget for JS eval. Default 30s — fits the watchdog
// design rationale (script phase tightens to 5s, settle gets the remainder).
// Sites with legitimately slow SSR/hydration can set UNBROWSER_TIMEOUT_MS
// higher; clamped to [1000, 600_000] (1s..10min) to keep silly values from
// re-introducing the orphan-leak class of bug.
fn read_dispatch_budget_ms() -> u64 {
    std::env::var("UNBROWSER_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|v| v.clamp(1_000, 600_000))
        .unwrap_or(30_000)
}

// Per-navigation budget for static script evaluation before DOMContentLoaded.
// Heavy bundles can still be useful when SSR content exists, so the default is
// intentionally bounded and the result reports budget_exhausted/budget_skipped.
fn read_script_eval_budget_ms() -> u64 {
    std::env::var("UNBROWSER_SCRIPT_EVAL_BUDGET_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|v| v.clamp(100, 600_000))
        .unwrap_or(5_000)
}

async fn rpc_main(profile: Profile) -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let policy_block = parse_policy_arg(&args);
    let shim_mode = parse_shim_mode_arg(&args)?;
    let profile_name = profile.name.clone();
    let mut session = Session::new(&profile, policy_block, shim_mode)?;
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let dispatch_budget_ms = read_dispatch_budget_ms();
    emit_event(
        "ready",
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "dispatch_budget_ms": dispatch_budget_ms,
            "profile": profile_name,
            "shim_mode": shim_mode.as_str(),
        }),
    );

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = err_response(Value::Null, -32700, format!("parse error: {e}"));
                write_response(&mut out, &resp)?;
                continue;
            }
        };

        let id = req.id.clone();
        // Bound EVERY RPC call's JS work with a wall-clock deadline. The
        // watchdog interrupt handler installed in Session::new aborts any
        // running eval (script-phase, settle pump, microtask, query) once
        // the deadline passes. Without this, exec_scripts=true on hostile
        // SPAs left CPU-pegged orphan processes behind. Restore on the way
        // out so back-to-back calls each get a fresh budget. Default 30s,
        // tunable via UNBROWSER_TIMEOUT_MS for legit-but-slow sites.
        let prev_dispatch_deadline = session.set_eval_deadline_from_now(dispatch_budget_ms);
        let resp = match req.method.as_str() {
            "eval" => match eval_code_param(&req.params) {
                Ok(code) => match session.eval(code) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -1, e.to_string()),
                },
                Err(e) => err_response(id, -32602, e),
            },
            "navigate" => match req.params.get("url").and_then(|v| v.as_str()) {
                Some(u) => {
                    let exec = req
                        .params
                        .get("exec_scripts")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let include_ascii = req
                        .params
                        .get("include_ascii")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    match session.navigate_opts(u, exec, include_ascii).await {
                        Ok(v) => ok_response(id, v),
                        Err(e) => err_response(id, -2, e.to_string()),
                    }
                }
                None => err_response(id, -32602, "missing 'url' param"),
            },
            "body" => match session.last_body.lock().ok().and_then(|g| g.clone()) {
                Some(b) => ok_response(id, Value::String(b)),
                None => err_response(id, -3, "no body — call navigate first"),
            },
            "query" => match req.params.get("selector").and_then(|v| v.as_str()) {
                Some(s) => match session.query(s) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -4, e.to_string()),
                },
                None => err_response(id, -32602, "missing 'selector' param"),
            },
            "query_debug" => {
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10) as u32;
                match req.params.get("selector").and_then(|v| v.as_str()) {
                    Some(s) => match session.query_debug(s, limit) {
                        Ok(v) => ok_response(id, v),
                        Err(e) => err_response(id, -4, e.to_string()),
                    },
                    None => err_response(id, -32602, "missing 'selector' param"),
                }
            }
            "text" => {
                let s = req
                    .params
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("body");
                match session.text(s) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -5, e.to_string()),
                }
            }
            "text_main" => match session.text_main() {
                Ok(v) => ok_response(id, v),
                Err(e) => err_response(id, -5, e.to_string()),
            },
            "text_clean" => {
                let selector = req.params.get("selector").and_then(|v| v.as_str());
                let max_chars = req
                    .params
                    .get("max_chars")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                match session.text_clean(selector, max_chars) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -5, e.to_string()),
                }
            }
            "find_text" => {
                let text = req.params.get("text").and_then(|v| v.as_str());
                let selector = req.params.get("selector").and_then(|v| v.as_str());
                let exact = req
                    .params
                    .get("exact")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as u32;
                let context_chars = req
                    .params
                    .get("context_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(80) as u32;
                match text {
                    Some(t) => match session.find_text(t, selector, exact, limit, context_chars) {
                        Ok(v) => ok_response(id, v),
                        Err(e) => err_response(id, -5, e.to_string()),
                    },
                    None => err_response(id, -32602, "missing 'text' param"),
                }
            }
            "text_around" => {
                let ref_ = req.params.get("ref").and_then(|v| v.as_str());
                let text = req.params.get("text").and_then(|v| v.as_str());
                let selector = req.params.get("selector").and_then(|v| v.as_str());
                let context_chars = req
                    .params
                    .get("context_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(400) as u32;
                if ref_.is_none() && text.is_none() {
                    err_response(id, -32602, "missing 'ref' or 'text' param")
                } else {
                    match session.text_around(ref_, text, selector, context_chars) {
                        Ok(v) => ok_response(id, v),
                        Err(e) => err_response(id, -5, e.to_string()),
                    }
                }
            }
            "query_text" => {
                let text = req.params.get("text").and_then(|v| v.as_str());
                let selector = req.params.get("selector").and_then(|v| v.as_str());
                let exact = req
                    .params
                    .get("exact")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as u32;
                match text {
                    Some(t) => match session.query_text(t, selector, exact, limit) {
                        Ok(v) => ok_response(id, v),
                        Err(e) => err_response(id, -5, e.to_string()),
                    },
                    None => err_response(id, -32602, "missing 'text' param"),
                }
            }
            "blockmap" => match session.blockmap().map(|mut v| {
                // Strip ASCII on standalone blockmap calls — same as navigate default
                if let Some(obj) = v.as_object_mut() {
                    obj.remove("ascii");
                }
                v
            }) {
                Ok(v) => ok_response(id, v),
                Err(e) => err_response(id, -6, e.to_string()),
            },
            "page_model" => {
                let goal = req.params.get("goal").and_then(|v| v.as_str());
                let types = req.params.get("types");
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as u32;
                match session.page_model(goal, types, limit) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                }
            }
            "route_discover" => {
                let goal = req.params.get("goal").and_then(|v| v.as_str());
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30) as u32;
                match session.route_discover(goal, limit) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                }
            }
            "discover" => match parse_discover_options(&req.params) {
                Ok(opts) => match session.discover(opts).await {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                },
                Err(e) => err_response(id, -32602, e.to_string()),
            },
            "network_extract" => {
                let query = req
                    .params
                    .get("query")
                    .or_else(|| req.params.get("goal"))
                    .and_then(|v| v.as_str());
                let types = req.params.get("types");
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as usize;
                let host = req.params.get("host").and_then(|v| v.as_str());
                let nav_id = req.params.get("nav_id").and_then(|v| v.as_str());
                match session.network_extract(query, types, limit, host, nav_id) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                }
            }
            "extract" => {
                let strategy = req.params.get("strategy").and_then(|v| v.as_str());
                match session.extract(strategy) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                }
            }
            "extract_table" | "table_to_json" => {
                let selector = req
                    .params
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("table");
                match session.extract_table(selector) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                }
            }
            "extract_list" => {
                let item = req.params.get("item_selector").and_then(|v| v.as_str());
                let fields = req.params.get("fields");
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1000) as u32;
                match (item, fields) {
                    (Some(i), Some(f)) => match session.extract_list(i, f, limit) {
                        Ok(v) => ok_response(id, v),
                        Err(e) => err_response(id, -6, e.to_string()),
                    },
                    _ => err_response(id, -32602, "missing 'item_selector' or 'fields' param"),
                }
            }
            "extract_cards" => {
                let selector = req.params.get("selector").and_then(|v| v.as_str());
                let kind = req.params.get("kind").and_then(|v| v.as_str());
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as u32;
                match session.extract_cards(selector, limit, kind) {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                }
            }
            "settle" => {
                let max_ms = req
                    .params
                    .get("max_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2000);
                let max_iters = req
                    .params
                    .get("max_iters")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as u32;
                match session.settle(max_ms, max_iters).await {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -6, e.to_string()),
                }
            }
            "click" => match req.params.get("ref").and_then(|v| v.as_str()) {
                Some(r) => match session.click(r).await {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -7, e.to_string()),
                },
                None => err_response(id, -32602, "missing 'ref' param"),
            },
            "activate" => {
                let ref_ = req.params.get("ref").and_then(|v| v.as_str());
                let text = req.params.get("text").and_then(|v| v.as_str());
                match session.activate(ref_, text).await {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -7, e.to_string()),
                }
            }
            "type" => {
                let r = req.params.get("ref").and_then(|v| v.as_str());
                let t = req.params.get("text").and_then(|v| v.as_str());
                match (r, t) {
                    (Some(r), Some(t)) => match session.type_(r, t) {
                        Ok(v) => ok_response(id, v),
                        Err(e) => err_response(id, -8, e.to_string()),
                    },
                    _ => err_response(id, -32602, "missing 'ref' or 'text' param"),
                }
            }
            "submit" => match req.params.get("ref").and_then(|v| v.as_str()) {
                Some(r) => match session.submit(r).await {
                    Ok(v) => ok_response(id, v),
                    Err(e) => err_response(id, -9, e.to_string()),
                },
                None => err_response(id, -32602, "missing 'ref' param"),
            },
            "cookies_set" => {
                let cookies = req.params.get("cookies").and_then(|v| v.as_array());
                let default_url = req
                    .params
                    .get("url")
                    .and_then(|v| v.as_str())
                    .or(session.last_url.as_deref());
                match cookies {
                    Some(arr) => match session.jar.import(arr, default_url) {
                        Ok(n) => ok_response(id, json!({ "added": n })),
                        Err(e) => err_response(id, -10, e.to_string()),
                    },
                    None => err_response(id, -32602, "missing 'cookies' param"),
                }
            }
            "cookies_get" => ok_response(id, Value::Array(session.jar.export())),
            "cookies_clear" => {
                session.jar.clear();
                ok_response(id, json!({ "ok": true }))
            }
            "report_outcome" => {
                let nav_id = req
                    .params
                    .get("navigation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match validate_and_emit_outcome(&session, &req.params, &nav_id) {
                    Ok(()) => ok_response(id, json!({ "ok": true })),
                    Err(msg) => err_response(id, -32602, msg),
                }
            }
            "network_stores" => {
                let limit = req
                    .params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as usize;
                let host = req.params.get("host").and_then(|v| v.as_str());
                // nav_id default: most recent (i.e. current) navigation.
                // "all" → no nav filter. Explicit "nav_<n>" → that one only.
                // (PR #7 review medium: prevent stale page-A data.)
                let nav_param = req.params.get("nav_id").and_then(|v| v.as_str());
                let scope_id: Option<String> = match nav_param {
                    Some("all") => None,
                    Some(explicit) => Some(explicit.to_string()),
                    None => session
                        ._fetch
                        .current_nav_id
                        .lock()
                        .ok()
                        .and_then(|g| g.clone()),
                };
                let scope = match scope_id.as_deref() {
                    Some(id) => network_store::NavScope::Only(id),
                    None => network_store::NavScope::All,
                };
                let captures = session
                    ._fetch
                    .network_store
                    .lock()
                    .map(|s| s.ranked(limit, host, scope))
                    .unwrap_or_default();
                ok_response(id, serde_json::to_value(&captures).unwrap_or(Value::Null))
            }
            "network_stores_clear" => {
                if let Ok(mut s) = session._fetch.network_store.lock() {
                    s.clear();
                }
                ok_response(id, json!({ "ok": true }))
            }
            "close" => {
                write_response(&mut out, &ok_response(id, json!("bye")))?;
                return Ok(());
            }
            other => err_response(id, -32601, format!("unknown method: {other}")),
        };
        session.restore_eval_deadline(prev_dispatch_deadline);
        write_response(&mut out, &resp)?;
    }
    Ok(())
}

// =============================================================================
// MCP server mode (--mcp flag)
//
// Spec: https://modelcontextprotocol.io/  (JSON-RPC 2.0 over stdio)
// Methods we handle: initialize, notifications/initialized, tools/list, tools/call,
// resources/list, and prompts/list.
// Tool surface = our RPC methods (everything except `close`, which is implicit).
// =============================================================================

fn mcp_tools() -> Value {
    mcp_tools_for_profile("full")
}

fn mcp_tools_for_profile(profile: &str) -> Value {
    let minimal = profile == "minimal";
    let tools = json!([
        {
            "name": "navigate",
            "description": "Fetch a URL with Chrome-fingerprinted HTTP using the active profile. Parses HTML, seeds the JS DOM, returns BlockMap inline. With `exec_scripts: true`, extracts inline AND external <script> tags from the parsed HTML, fetches externals in parallel (8s per-fetch timeout), eval's them in document order in QuickJS (with shims for setTimeout/fetch/etc.), then settles the event loop and fires DOMContentLoaded + load. `<script async>` is honored: async scripts execute after the sync queue. When `--policy=blocklist` is set, tracker URLs are blocked at script-fetch time (see scripts.policy_blocked in the result). Returns a `scripts` summary with inline_count, external_count, async_count, policy_blocked, executed, errors.\n\nAuto-extract: when the page embeds JSON-bearing <script> tags (density.json_scripts > 0 — covers application/json, application/ld+json, text/x-magento-init, text/x-shopify-app, etc.), navigate auto-runs `extract()` and returns the result as the `extract` field. Saves a round trip on the common case where the data the JS would have rendered is already sitting in the HTML — JSON-LD article schemas on news sites, __NEXT_DATA__ page state on Next.js apps, json_in_script product blobs on Magento/Shopify, GitHub RSC payloads, etc. Capped at 16 KB inline; over that limit `extract` returns a stub with strategy/confidence/size_bytes/hint and the agent should call `extract()` explicitly to retrieve the full payload. Pages with no embedded JSON get extract:null and pay zero extra cost.\n\nTool advice: navigate also returns `tool_likelihoods` plus `tool_recommendations`, derived from concrete page signals (structure/headings, selector hints, density, embedded data, network captures, challenge state, and script pathology) so agents can pick the next tool without guessing.\n\nAuto-solve: Reddit's JS proof-of-work challenge (provider: reddit_js_challenge) is transparently solved — the challenge is detected, the GET solution URL is computed (solution = hex_value + hex_value), and the real page is returned in one navigate call. challenge:null on the result means the real page was served. Subsequent navigations in the same session carry the clearance cookie and skip the challenge entirely.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url":          { "type": "string", "description": "Absolute URL to fetch" },
                    "exec_scripts": { "type": "boolean", "description": "Run page <script> tags (inline + external src) after parse, settle the event loop, and fire DOMContentLoaded + load. Default false." },
                    "include_ascii": { "type": "boolean", "description": "Include human-readable ASCII blockmap grid (redundant with JSON fields). Default false." }
                },
                "required": ["url"]
            }
        },
        {
            "name": "query",
            "description": "Run a CSS selector against the current page's parsed DOM. Returns matching elements as [{ref, tag, attrs, text, text_chars, text_truncated}]. Element refs (e:NN) are stable handles for use with click/type/submit. Selector engine supports tag, id, class, attribute matchers (=, ^=, $=, *=, ~=), all four combinators (descendant, >, +, ~), pseudo-classes (:first/last/nth-child including An+B formulas, :first/last/nth-of-type, :only-child/of-type), :not(), and :has().",
            "inputSchema": {
                "type": "object",
                "properties": { "selector": { "type": "string", "description": "CSS selector" } },
                "required": ["selector"]
            }
        },
        {
            "name": "query_debug",
            "description": "Diagnose why a CSS selector did or did not match. Returns matched_count, sample matches, DOM summary counts, selector hints (top tags/classes/data attrs/ids), and actionable hints for selector_miss, thin_shell, or embedded_json. Use this when query() returns [] and you need to distinguish a bad selector from an empty/browser-rendered DOM.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector to test" },
                    "limit": { "type": "integer", "description": "Max sample matches to return (default 10, max 50)" }
                },
                "required": ["selector"]
            }
        },
        {
            "name": "text",
            "description": "Get the textContent of the FIRST element matching the selector (default: body). Note: on Wikipedia/MDN/news sites, the first <p> is often a hatnote or image caption, not the lead paragraph — prefer `text_main` for reading the page's primary content.",
            "inputSchema": {
                "type": "object",
                "properties": { "selector": { "type": "string", "description": "CSS selector (default: body)" } }
            }
        },
        {
            "name": "text_main",
            "description": "Get the textContent of the page's main content area, excluding chrome (header/nav/footer/aside). Tries <main>, then [role=main], then a single <article>, then falls back to the longest non-chrome subtree. Use this for reading article body / docs page / blog post content.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "text_clean",
            "description": "Return chrome-stripped, JSON-stripped, whitespace-collapsed text from a selector or the best content root. Drops script/style/noscript/svg and page chrome (nav/header/footer/aside) plus obvious hidden widgets and repeated boilerplate.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "Optional CSS selector to scope extraction. Default: best content root." },
                    "max_chars": { "type": "integer", "description": "Optional max characters to return." }
                }
            }
        },
        {
            "name": "find_text",
            "description": "Find localized text matches and return [{ref, tag, attrs, before, match, after, text}]. Ranks article/main/content matches above nav/header/footer boilerplate.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Substring to match (or exact string if exact=true)" },
                    "selector": { "type": "string", "description": "Optional CSS selector to limit search scope" },
                    "exact": { "type": "boolean", "description": "If true, exact cleaned-text match instead of substring (default false)" },
                    "limit": { "type": "integer", "description": "Max matches to return (default 20)" },
                    "context_chars": { "type": "integer", "description": "Characters before/after each match (default 80)" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "text_around",
            "description": "Return cleaned surrounding text around an element ref or the best ranked text match. Returns {ref, before, match, after, text}.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Optional element ref like e:142" },
                    "text": { "type": "string", "description": "Optional text to locate when ref is omitted" },
                    "selector": { "type": "string", "description": "Optional CSS selector to scope context" },
                    "context_chars": { "type": "integer", "description": "Characters before/after the target (default 400)" }
                }
            }
        },
        {
            "name": "query_text",
            "description": "Find elements by visible text content. Returns the smallest/deepest element whose textContent matches the needle, with chrome (header/nav/footer/aside) skipped. Anchor-promotion: a span/strong/etc. inside an <a> resolves to the anchor (so click() targets the actionable element). Right tool when CSS selectors are unstable (React-rendered pages with hashed class names) but the visible label is reliable — e.g. find a 'Sign in' button without knowing its class.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text":     { "type": "string", "description": "Substring to match (or exact string if exact=true)" },
                    "selector": { "type": "string", "description": "Optional CSS selector to limit search scope (default: whole document body)" },
                    "exact":    { "type": "boolean", "description": "If true, exact match instead of substring (default false)" },
                    "limit":    { "type": "integer", "description": "Max matches to return (default 20)" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "blockmap",
            "description": "Recompute the BlockMap for the current page. Use after eval'd JS or click/type modifies the DOM. Same shape as the inline blockmap from navigate.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "page_model",
            "description": "Render the current page into semantic, task-discoverable JSON objects. Reconstructs page structure as search_form, nav_link, article_card, course_card, model_card, product_card, table, answer_block, and limitation objects with actions, normalized fields, goal-based scoring, and provenance. Prefer this as the first planning tool after navigate when raw links/text are too wide.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "Optional task goal/query used to rank objects by relevance." },
                    "types": { "type": "array", "items": { "type": "string" }, "description": "Optional object kinds to return, e.g. search_form, article_card, model_card, course_card, card, table, answer_block." },
                    "limit": { "type": "integer", "description": "Max objects to return (default 50)." }
                }
            }
        },
        {
            "name": "route_discover",
            "description": "Find page-owned navigation/search routes for a goal. Returns ranked visible links, forms with controls/query_url previews, and inferred URLs derived from page-owned routes plus goal terms. Use before guessing URLs manually.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "Optional task goal/query used to rank routes and build GET query previews." },
                    "limit": { "type": "integer", "description": "Max routes/forms/inferred URLs per section (default 30)." }
                }
            }
        },
        {
            "name": "discover",
            "description": "High-level cheap-first information discovery. Optionally navigates to a URL, runs light JS, merges DOM routes, inferred form/query URLs, and network JSON routes into one ranked graph with provenance plus route-level escalation hints. Use this when the task is to find where information lives before extracting it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Optional absolute http(s) URL to navigate before discovery. If omitted, discovers on the current page." },
                    "goal": { "type": "string", "description": "Optional goal/query used to rank routes and build query URLs." },
                    "exec_scripts": { "type": "boolean", "description": "Run page scripts during navigation when url is provided. Default false; enable when static discovery is insufficient." },
                    "same_origin": { "type": "boolean", "description": "If true, only return page-owned routes." },
                    "include_network": { "type": "boolean", "description": "Include captured network JSON objects and API-like captures. Default true." },
                    "limit": { "type": "integer", "description": "Max routes to return after dedupe/ranking, 1-200 (default 50)." },
                    "debug": { "type": "boolean", "description": "If true, include full nested navigate, route_discover, and network_extract payloads. Default false returns compact summaries." }
                }
            }
        },
        {
            "name": "network_extract",
            "description": "Parse captured JSON/API/network responses into semantic objects with fields, scores, matched query terms, and capture/path provenance. Use after navigate or activate when network_stores shows JSON/GraphQL/NDJSON captures and raw body_preview is too noisy.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Optional task query/goal used to rank objects." },
                    "types": { "type": "array", "items": { "type": "string" }, "description": "Optional object kinds to keep, e.g. product_card, article_card, model_card, network_object, card." },
                    "limit": { "type": "integer", "description": "Max objects to return (default 50)." },
                    "host": { "type": "string", "description": "Optional substring filter on response host." },
                    "nav_id": { "type": "string", "description": "Defaults to the most recent navigation_id. Pass 'all' to inspect all captures." }
                }
            }
        },
        {
            "name": "extract",
            "description": "Auto-strategy structured-data extraction. Tries JSON-LD (schema.org) → __NEXT_DATA__ → Nuxt → JSON-in-script (Magento, Shopify, BigCommerce custom-typed scripts) → OpenGraph/meta → microdata → text_main fallback, returns the highest-confidence hit as {strategy, confidence, data, tried}. Use this as the one-shot 'give me the data, you figure out how' call when you don't want to plan the strategy yourself. Pass strategy='json_ld' (or any of the names above) to force a specific extractor.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "strategy": { "type": "string", "description": "Optional: force a specific extractor (json_ld, next_data, nuxt_data, json_in_script, og_meta, microdata, text_main)" }
                }
            }
        },
        {
            "name": "extract_table",
            "description": "Pull a <table> into {headers, rows, row_count}. Headers come from <thead><th>...</th></thead> if present, else the first <tr>'s <th> cells. Each subsequent <tr>'s <td> cells become a row dict keyed by header (or 'col_N' if no header for that column). Right tool for pricing tables, specs, finance/listings tables — saves writing the per-cell mapping eval.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector matching the <table> element" }
                },
                "required": ["selector"]
            }
        },
        {
            "name": "table_to_json",
            "description": "Alias for extract_table with a first-table default. Pulls a table into {headers, rows, row_count}; selector defaults to 'table'. Use this when an agent expects a table-to-JSON convenience tool.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "Optional CSS selector matching the <table> element (default: table)" }
                }
            }
        },
        {
            "name": "extract_list",
            "description": "Pull a repeated card pattern into [{...}, {...}]. Right tool for HN-style lists, search results, product grids — collapses per-site eval boilerplate. Field spec shapes: 'css selector' (text content), 'css selector @attr' (attribute), or ['css selector', '@attr'] (tuple form). If a sub-selector returns null, the field value is null.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "item_selector": { "type": "string", "description": "CSS selector matching each card/row" },
                    "fields": { "type": "object", "description": "{field_name: 'sub-selector' | 'sub-selector @attr' | ['sub-selector', '@attr']}" },
                    "limit": { "type": "integer", "description": "Max items to extract (default 1000)" }
                },
                "required": ["item_selector", "fields"]
            }
        },
        {
            "name": "extract_cards",
            "description": "Auto-detect repeated article/card/product/course/listing blocks and return normalized items [{title, price, condition, url, availability, snippet, meta, image_alt, score}]. Prefer this over extract_list when the page has semantically ambiguous recipe, course, product, or model cards and you do not already know field selectors. Optional selector scopes detection to known card nodes; kind can bias scoring (recipe, course, product, listing).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "Optional CSS selector matching each card/listing block" },
                    "limit": { "type": "integer", "description": "Max items to extract (default 50)" },
                    "kind": { "type": "string", "description": "Optional hint: recipe, course, product, listing, article" }
                }
            }
        },
        {
            "name": "settle",
            "description": "Drain the JS event loop: alternately runs queued microtasks (Promise resolutions) and fires expired setTimeout/setInterval callbacks, sleeping to the next deadline when only timers remain. Returns when the queue is empty OR max_ms elapses OR max_iters iterations complete. Defaults: max_ms=2000, max_iters=50. Use after seeding the DOM (or after eval'd code that schedules timers) to let pending callbacks run.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "max_ms":    { "type": "integer", "description": "Max wall-clock ms to spend (default 2000)" },
                    "max_iters": { "type": "integer", "description": "Max iterations of the drain loop (default 50)" }
                }
            }
        },
        {
            "name": "click",
            "description": "Dispatch a click event on the element at `ref` (e.g. e:142, returned from query). If the element is <a href> and the click was not preventDefault'd, auto-follows the href via navigate (returns the full navigation result with new BlockMap). Otherwise returns {ok, ref, tag, follow: null}.",
            "inputSchema": {
                "type": "object",
                "properties": { "ref": { "type": "string", "description": "Element ref like e:142" } },
                "required": ["ref"]
            }
        },
        {
            "name": "activate",
            "description": "Higher-level action probe. Clicks an element by ref or visible action text, settles, and returns before/after URL, BlockMap/page_model summaries, network counts, hashes, and classification: navigated, dom_changed, network_changed, no_effect, or unsupported.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Optional element ref like e:142." },
                    "text": { "type": "string", "description": "Optional visible action text to locate when ref is omitted." }
                }
            }
        },
        {
            "name": "type",
            "description": "Set the value of an input/textarea (referenced by `ref`) and dispatch input + change events. Use before submit on form fields.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Input element ref like e:142" },
                    "text": { "type": "string", "description": "Value to set" }
                },
                "required": ["ref", "text"]
            }
        },
        {
            "name": "submit",
            "description": "Submit a form by gathering input/textarea/select values and navigating to the resolved action URL. Supports GET and application/x-www-form-urlencoded POST. Checked checkbox/radio values are serialized; multipart upload forms are not supported.",
            "inputSchema": {
                "type": "object",
                "properties": { "ref": { "type": "string", "description": "Form element ref like e:142" } },
                "required": ["ref"]
            }
        },
        {
            "name": "body",
            "description": "Return the raw HTML body of the last navigation. Use as a fallback when the BlockMap or selectors aren't enough — but the response can be large (often 100KB+).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "eval",
            "description": "Run arbitrary JavaScript in the embedded QuickJS runtime against the current page's parsed DOM. Returns the JSON-stringified result. Power tool — prefer query/text/blockmap when the CSS selector engine can express what you need. Canonical param is code; raw JSON-RPC also accepts script or expression aliases and errors if no code-like param is present.",
            "inputSchema": {
                "type": "object",
                "properties": { "code": { "type": "string", "description": "JS code; the value of the last expression is returned" } },
                "required": ["code"]
            }
        },
        {
            "name": "cookies_set",
            "description": "Add cookies to the session jar. Each item is an object {name, value, domain, path?, secure?, http_only?, url?} or a raw Set-Cookie string. Continue using session state from a user-authorized browser, where permitted, by replaying a clearance cookie (e.g. PerimeterX _px3) acquired in real Chrome — requires explicit user confirmation and origin-scoped, ephemeral storage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cookies": { "type": "array", "description": "Array of cookie objects or Set-Cookie strings" },
                    "url": { "type": "string", "description": "Default URL for cookies that don't specify domain" }
                },
                "required": ["cookies"]
            }
        },
        {
            "name": "cookies_get",
            "description": "Return all cookies currently in the jar as [{name, value, domain, path, secure, http_only}]. Use this to export cookies to disk for a later session.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "cookies_clear",
            "description": "Drop all cookies from the jar.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "report_outcome",
            "description": "Bind a task outcome (success/failure/quality) to a previous navigation_id from a navigate() call. Used by the policy framework's outcome protocol — see docs/probabilistic-policy.md §4.5. v0 emits an outcome_reported NDJSON event for the navigation; no posterior updates yet. Drivers should call this once per agent task so future Bayesian phases (B/D-2) can attribute extraction success/failure to specific policy decisions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "navigation_id": { "type": "string", "description": "The id returned by navigate() — joins this outcome to the policy_trace event." },
                    "task_id":       { "type": "string", "description": "Optional opaque id chosen by the driver for cross-system correlation." },
                    "task_class":    { "type": "string", "enum": ["extract", "query", "click", "form", "visual"], "description": "What kind of task succeeded/failed. Lets future posteriors condition on task class." },
                    "success":       { "type": "boolean", "description": "Did the agent's task succeed?" },
                    "quality":       { "type": "number", "description": "Optional 0..1 quality score (e.g. fraction of expected fields extracted)." },
                    "error":         { "type": "string", "description": "Optional human-readable error/explanation when success=false." }
                },
                "required": ["navigation_id", "success"]
            }
        },
        {
            "name": "network_stores",
            "description": "Return content-bearing fetch/XHR responses captured during navigate, ranked by likely content value. SPAs often keep their data in API responses (JSON, GraphQL, NDJSON, Next/Nuxt route data) that are cleaner than the rendered DOM — this tool surfaces them directly. Each entry has capture_id, URL, status, content-type, body_preview (truncated to 256 KB), body_bytes (full size), body_truncated flag, navigation_id, and a heuristic score. Bodies for trackers/ads/CSS/HTML/media are NOT captured. The navigate result already contains a top-5 summary scoped to that navigation; use this tool to get more entries, filter by host, or pull captures from a different navigation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit":  { "type": "integer", "description": "Max entries to return (default 20).", "minimum": 1, "maximum": 100 },
                    "host":   { "type": "string", "description": "Optional substring filter on response host." },
                    "nav_id": { "type": "string", "description": "Defaults to the most recent navigation_id (page B never sees page A captures). Pass an explicit navigation_id from a prior navigate result to query that navigation specifically. Pass 'all' to disable nav filtering and return captures from every navigation." }
                }
            }
        },
        {
            "name": "network_stores_clear",
            "description": "Drop all captured network responses from the session's network store. Use this between unrelated navigations if you don't want earlier captures showing up in later network_stores calls.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "help",
            "description": "Discover full tooling from the minimal 3. Returns grouped catalog of all 32 tools with when to use and example. Minimal profile shows only navigate (as open), extract, help; help unlocks the rest. Use help(topic) to filter e.g. 'query', 'extraction', 'discovery'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "description": "Optional topic filter: query, reading, discovery, extraction, interaction, session, or a tool name" }
                }
            }
        }
    ]);
    if minimal {
        // Minimal profile: keep navigate (as open), extract, help — plus query so agents can probe selectors before escalating.
        let allowed = ["navigate", "extract", "help", "query"];
        if let Some(arr) = tools.as_array() {
            let filtered: Vec<Value> = arr
                .iter()
                .filter(|t| allowed.contains(&t.get("name").and_then(|v| v.as_str()).unwrap_or("")))
                .cloned()
                .collect();
            return json!(filtered);
        }
    }
    tools
}

async fn dispatch_tool(session: &mut Session, name: &str, args: &Value) -> Result<Value> {
    let str_arg = |k: &str| args.get(k).and_then(|v| v.as_str());
    match name {
        "navigate" => {
            let url = str_arg("url").ok_or_else(|| anyhow!("missing 'url'"))?;
            let exec = args
                .get("exec_scripts")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let include_ascii = args
                .get("include_ascii")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            session.navigate_opts(url, exec, include_ascii).await
        }
        "query" => {
            let sel = str_arg("selector").ok_or_else(|| anyhow!("missing 'selector'"))?;
            session.query(sel)
        }
        "query_debug" => {
            let sel = str_arg("selector").ok_or_else(|| anyhow!("missing 'selector'"))?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as u32;
            session.query_debug(sel, limit)
        }
        "text" => {
            let sel = str_arg("selector").unwrap_or("body");
            session.text(sel)
        }
        "text_main" => session.text_main(),
        "text_clean" => {
            let selector = str_arg("selector");
            let max_chars = args
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            session.text_clean(selector, max_chars)
        }
        "find_text" => {
            let text = str_arg("text").ok_or_else(|| anyhow!("missing 'text'"))?;
            let selector = str_arg("selector");
            let exact = args.get("exact").and_then(|v| v.as_bool()).unwrap_or(false);
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as u32;
            let context_chars = args
                .get("context_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(80) as u32;
            session.find_text(text, selector, exact, limit, context_chars)
        }
        "text_around" => {
            let ref_ = str_arg("ref");
            let text = str_arg("text");
            if ref_.is_none() && text.is_none() {
                return Err(anyhow!("missing 'ref' or 'text'"));
            }
            let selector = str_arg("selector");
            let context_chars = args
                .get("context_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(400) as u32;
            session.text_around(ref_, text, selector, context_chars)
        }
        "query_text" => {
            let text = str_arg("text").ok_or_else(|| anyhow!("missing 'text'"))?;
            let selector = str_arg("selector");
            let exact = args.get("exact").and_then(|v| v.as_bool()).unwrap_or(false);
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as u32;
            session.query_text(text, selector, exact, limit)
        }
        "blockmap" => session.blockmap().map(|mut v| {
            if let Some(obj) = v.as_object_mut() {
                obj.remove("ascii");
            }
            v
        }),
        "page_model" => {
            let goal = str_arg("goal");
            let types = args.get("types");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
            session.page_model(goal, types, limit)
        }
        "route_discover" => {
            let goal = str_arg("goal");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
            session.route_discover(goal, limit)
        }
        "discover" => {
            let opts = parse_discover_options(args)?;
            session.discover(opts).await
        }
        "network_extract" => {
            let query = str_arg("query").or_else(|| str_arg("goal"));
            let types = args.get("types");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let host = str_arg("host");
            let nav_id = str_arg("nav_id");
            session.network_extract(query, types, limit, host, nav_id)
        }
        "extract" => {
            let strategy = str_arg("strategy");
            session.extract(strategy)
        }
        "extract_table" => {
            let sel = str_arg("selector").ok_or_else(|| anyhow!("missing 'selector'"))?;
            session.extract_table(sel)
        }
        "table_to_json" => {
            let sel = str_arg("selector").unwrap_or("table");
            session.extract_table(sel)
        }
        "extract_list" => {
            let item =
                str_arg("item_selector").ok_or_else(|| anyhow!("missing 'item_selector'"))?;
            let fields = args
                .get("fields")
                .ok_or_else(|| anyhow!("missing 'fields'"))?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(1000) as u32;
            session.extract_list(item, fields, limit)
        }
        "extract_cards" => {
            let selector = str_arg("selector");
            let kind = str_arg("kind");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
            session.extract_cards(selector, limit, kind)
        }
        "settle" => {
            let max_ms = args.get("max_ms").and_then(|v| v.as_u64()).unwrap_or(2000);
            let max_iters = args.get("max_iters").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
            session.settle(max_ms, max_iters).await
        }
        "click" => {
            let r = str_arg("ref").ok_or_else(|| anyhow!("missing 'ref'"))?;
            session.click(r).await
        }
        "activate" => {
            let ref_ = str_arg("ref");
            let text = str_arg("text");
            session.activate(ref_, text).await
        }
        "type" => {
            let r = str_arg("ref").ok_or_else(|| anyhow!("missing 'ref'"))?;
            let t = str_arg("text").ok_or_else(|| anyhow!("missing 'text'"))?;
            session.type_(r, t)
        }
        "submit" => {
            let r = str_arg("ref").ok_or_else(|| anyhow!("missing 'ref'"))?;
            session.submit(r).await
        }
        "body" => match session.last_body.lock().ok().and_then(|g| g.clone()) {
            Some(b) => Ok(Value::String(b)),
            None => Err(anyhow!("no body — call navigate first")),
        },
        "eval" => {
            let code = eval_code_param(args).map_err(|e| anyhow!(e))?;
            session.eval(code)
        }
        "cookies_set" => {
            let cookies = args
                .get("cookies")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("missing 'cookies'"))?;
            let default_url = str_arg("url").or(session.last_url.as_deref());
            let added = session.jar.import(cookies, default_url)?;
            Ok(json!({ "added": added }))
        }
        "cookies_get" => Ok(Value::Array(session.jar.export())),
        "cookies_clear" => {
            session.jar.clear();
            Ok(json!({ "ok": true }))
        }
        "report_outcome" => {
            let nav_id =
                str_arg("navigation_id").ok_or_else(|| anyhow!("missing 'navigation_id'"))?;
            validate_and_emit_outcome(session, args, nav_id).map_err(|e| anyhow!(e))?;
            Ok(json!({ "ok": true }))
        }
        "network_stores" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
            let host = str_arg("host");
            let nav_param = str_arg("nav_id");
            let scope_id: Option<String> = match nav_param {
                Some("all") => None,
                Some(explicit) => Some(explicit.to_string()),
                None => session
                    ._fetch
                    .current_nav_id
                    .lock()
                    .ok()
                    .and_then(|g| g.clone()),
            };
            let scope = match scope_id.as_deref() {
                Some(id) => network_store::NavScope::Only(id),
                None => network_store::NavScope::All,
            };
            let captures = session
                ._fetch
                .network_store
                .lock()
                .map(|s| s.ranked(limit, host, scope))
                .unwrap_or_default();
            Ok(serde_json::to_value(&captures).unwrap_or(Value::Null))
        }
        "network_stores_clear" => {
            if let Ok(mut s) = session._fetch.network_store.lock() {
                s.clear();
            }
            Ok(json!({ "ok": true }))
        }
        "help" => {
            let topic = args.get("topic").and_then(|v| v.as_str()).unwrap_or("");
            let catalog = mcp_tools();
            let tools = catalog.as_array().cloned().unwrap_or_default();
            if topic.is_empty() {
                // grouped catalog
                let groups = json!({
                    "core": ["navigate", "extract", "help"],
                    "query_text": ["query", "query_debug", "query_text", "find_text", "text_around"],
                    "reading": ["text", "text_main", "text_clean", "blockmap", "body"],
                    "discovery": ["page_model", "route_discover", "discover", "network_extract", "network_stores"],
                    "extraction": ["extract", "extract_cards", "extract_list", "extract_table", "table_to_json"],
                    "interaction": ["click", "activate", "type", "submit", "settle", "eval"],
                    "session": ["cookies_set", "cookies_get", "cookies_clear", "report_outcome", "network_stores_clear"]
                });
                Ok(
                    json!({ "catalog": groups, "tools": tools, "note": "Minimal profile shows navigate/extract/help/query; help unlocks the rest. Use help(topic) to filter." }),
                )
            } else {
                let t = topic.to_ascii_lowercase();
                let filtered: Vec<Value> = tools
                    .into_iter()
                    .filter(|v| {
                        let name = v
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        let desc = v
                            .get("description")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        name.contains(&t) || desc.contains(&t) || t == name
                    })
                    .collect();
                Ok(json!({ "topic": topic, "tools": filtered }))
            }
        }
        _ => Err(anyhow!("unknown tool: {name}")),
    }
}

async fn handle_session_request(
    session: &mut Session,
    req: Request,
    dispatch_budget_ms: u64,
) -> (Response, bool) {
    let id = req.id.clone();
    if req.method == "close" {
        return (ok_response(id, json!("bye")), true);
    }
    if req.method == "ping" {
        return (ok_response(id, json!({ "ok": true })), false);
    }
    let prev = session.set_eval_deadline_from_now(dispatch_budget_ms);
    let outcome = dispatch_tool(session, &req.method, &req.params).await;
    session.restore_eval_deadline(prev);
    match outcome {
        Ok(value) => (ok_response(id, value), false),
        Err(e) => {
            let message = e.to_string();
            (
                err_response(id, session_dispatch_error_code(&message), message),
                false,
            )
        }
    }
}

async fn handle_session_stream(
    session: &mut Session,
    stream: UnixStream,
    dispatch_budget_ms: u64,
) -> Result<bool> {
    let mut reader = StdBufReader::new(stream.try_clone().context("clone session stream")?);
    let mut writer = stream;
    let mut stop = false;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .context("read session request")?;
        if n == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let resp = err_response(Value::Null, -32700, format!("parse error: {e}"));
                write_response(&mut writer, &resp)?;
                continue;
            }
        };
        let (resp, should_stop) = handle_session_request(session, req, dispatch_budget_ms).await;
        write_response(&mut writer, &resp)?;
        if should_stop {
            stop = true;
            break;
        }
    }
    Ok(stop)
}

async fn session_daemon_cmd(args: &[String], daemon_i: usize) -> Result<()> {
    let socket = args
        .get(daemon_i + 1)
        .ok_or_else(|| anyhow!("--session-daemon requires a socket path"))?;
    let socket = PathBuf::from(socket);
    if let Some(parent) = socket.parent() {
        ensure_private_session_dir(parent)?;
    }
    if socket.exists() {
        std::fs::remove_file(&socket).context("remove stale session socket")?;
    }

    let profile_name = parse_profile_arg(args);
    let profile = Profile::load(&profile_name)?;
    let policy_block = parse_policy_arg(args);
    let shim_mode = parse_shim_mode_arg(args)?;
    let mut session = Session::new(&profile, policy_block, shim_mode)?;
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("bind session socket {}", socket.display()))?;
    let dispatch_budget_ms = read_dispatch_budget_ms();

    for stream in listener.incoming() {
        let stream = stream.context("accept session connection")?;
        if handle_session_stream(&mut session, stream, dispatch_budget_ms).await? {
            break;
        }
    }

    let _ = std::fs::remove_file(&socket);
    Ok(())
}

async fn mcp_main(profile: Profile) -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let policy_block = parse_policy_arg(&args);
    let shim_mode = parse_shim_mode_arg(&args)?;
    let mcp_profile = parse_mcp_profile_arg(&args);
    let mut session = Session::new(&profile, policy_block, shim_mode)?;
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let dispatch_budget_ms = read_dispatch_budget_ms();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                writeln!(out, "{}", serde_json::to_string(&resp)?)?;
                out.flush()?;
                continue;
            }
        };

        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = req.get("id").cloned();
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let is_notification = id.is_none();

        // Notifications never get a response.
        if method == "notifications/initialized" || method == "notifications/cancelled" {
            continue;
        }

        let result: Result<Value> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
                "serverInfo": {
                    "name": "unbrowser",
                    "version": env!("CARGO_PKG_VERSION"),
                    "shim_mode": shim_mode.as_str()
                }
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": mcp_tools_for_profile(&mcp_profile) })),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                // Same watchdog budget as the bare-RPC dispatcher.
                let prev = session.set_eval_deadline_from_now(dispatch_budget_ms);
                let outcome = dispatch_tool(&mut session, name, &arguments).await;
                session.restore_eval_deadline(prev);
                match outcome {
                    Ok(value) => {
                        let text = serde_json::to_string_pretty(&value)?;
                        Ok(json!({
                            "content": [{ "type": "text", "text": text }],
                            "isError": false
                        }))
                    }
                    Err(e) => Ok(json!({
                        "content": [{ "type": "text", "text": format!("Error: {e}") }],
                        "isError": true
                    })),
                }
            }
            _ => Err(anyhow!("method not found: {method}")),
        };

        if is_notification {
            continue;
        }

        let resp = match result {
            Ok(value) => json!({
                "jsonrpc": "2.0",
                "id": id.unwrap_or(Value::Null),
                "result": value
            }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id.unwrap_or(Value::Null),
                "error": { "code": -32601, "message": e.to_string() }
            }),
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::{
        ShimMode, apply_status_to_blockmap, command_index, eval_code_param,
        parse_navigate_cli_args, parse_session_exec_params, parse_shim_mode_arg,
        session_dispatch_error_code, validate_session_id,
    };
    use serde_json::json;

    fn args(parts: &[&str]) -> Vec<String> {
        std::iter::once("unbrowser".to_string())
            .chain(parts.iter().map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn navigate_args_accept_global_flags_before_command() {
        let args = args(&[
            "--profile",
            "chrome_134",
            "--shims",
            "enhanced",
            "navigate",
            "https://example.com",
            "--exec-scripts",
            "--events",
            "--json",
        ]);
        let command_i = command_index(&args, "navigate").expect("navigate command");
        let parsed = parse_navigate_cli_args(&args, command_i).expect("parsed navigate args");
        assert_eq!(parsed.url, "https://example.com");
        assert!(parsed.exec_scripts);
        assert!(parsed.events);
        assert_eq!(parse_shim_mode_arg(&args).unwrap(), ShimMode::Enhanced);
    }

    #[test]
    fn navigate_args_reject_extra_positional() {
        let args = args(&["navigate", "https://example.com", "https://example.org"]);
        let command_i = command_index(&args, "navigate").expect("navigate command");
        assert!(parse_navigate_cli_args(&args, command_i).is_err());
    }

    #[test]
    fn eval_code_param_accepts_aliases_but_rejects_ambiguity() {
        assert_eq!(eval_code_param(&json!({"code": "1+1"})).unwrap(), "1+1");
        assert_eq!(
            eval_code_param(&json!({"expression": "document.title"})).unwrap(),
            "document.title"
        );
        assert!(eval_code_param(&json!({})).is_err());
        assert!(eval_code_param(&json!({"code": "1", "script": "2"})).is_err());
        assert!(eval_code_param(&json!({"code": 42})).is_err());
    }

    #[test]
    fn http_error_suppresses_shell_density_signals() {
        let mut blockmap = json!({
            "density": {
                "tables": null,
                "td": null,
                "li": null,
                "json_scripts": 0,
                "script_heavy_shell": true,
                "thin_shell": true,
                "likely_js_filled": true
            }
        });
        apply_status_to_blockmap(&mut blockmap, 404);
        let density = blockmap.get("density").unwrap();
        assert_eq!(
            density.get("thin_shell").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            density.get("likely_js_filled").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            density.get("http_error_status").and_then(|v| v.as_u64()),
            Some(404)
        );
    }

    #[test]
    fn session_exec_params_support_common_shorthands() {
        assert_eq!(
            parse_session_exec_params("navigate", &["https://example.com".into()]).unwrap(),
            json!({"url": "https://example.com", "exec_scripts": false})
        );
        assert_eq!(
            parse_session_exec_params(
                "query_debug",
                &[".card".into(), "--limit".into(), "3".into()]
            )
            .unwrap(),
            json!({"selector": ".card", "limit": 3})
        );
        assert_eq!(
            parse_session_exec_params("type", &["e:1".into(), "hello".into(), "world".into()])
                .unwrap(),
            json!({"ref": "e:1", "text": "hello world"})
        );
        assert_eq!(
            parse_session_exec_params("eval", &["document.title".into()]).unwrap(),
            json!({"code": "document.title"})
        );
        assert_eq!(
            parse_session_exec_params("extract_cards", &["--limit".into(), "10".into()]).unwrap(),
            json!({"selector": null, "limit": 10})
        );
        assert_eq!(
            parse_session_exec_params(
                "extract_cards",
                &[".product-card".into(), "--limit".into(), "3".into()]
            )
            .unwrap(),
            json!({"selector": ".product-card", "limit": 3})
        );
        assert!(parse_session_exec_params("cookies_set", &["not-json".into()]).is_err());
    }

    #[test]
    fn session_dispatch_error_codes_distinguish_unknown_from_bad_params() {
        assert_eq!(session_dispatch_error_code("unknown tool: nope"), -32601);
        assert_eq!(
            session_dispatch_error_code("missing 'selector' param"),
            -32602
        );
    }

    #[test]
    fn session_id_validation_rejects_paths() {
        assert!(validate_session_id("ub_123-ok").is_ok());
        assert!(validate_session_id("../bad").is_err());
        assert!(validate_session_id("").is_err());
    }
}

#[cfg(test)]
mod shim_mode_tests {
    use super::{Profile, Session, ShimMode, profile};

    #[test]
    fn enhanced_shims_enable_layout_and_media_guesses() {
        let profile = Profile::load(profile::DEFAULT_PROFILE).expect("default profile");
        let stable = Session::new(&profile, false, ShimMode::Stable).expect("stable session");
        assert_eq!(
            stable
                .eval("document.createElement('div').getBoundingClientRect().width")
                .expect("stable eval")
                .as_u64(),
            Some(0)
        );

        let enhanced = Session::new(&profile, false, ShimMode::Enhanced).expect("enhanced session");
        let v = enhanced
            .eval(
                r#"(() => {
                  const el = document.createElement('div');
                  el.textContent = 'content that should occupy a synthetic box';
                  const first = document.createElement('div');
                  const second = document.createElement('div');
                  document.body.appendChild(first);
                  document.body.appendChild(second);
                  return {
                    mode: __unbrowser_shims.mode,
                    width: el.getBoundingClientRect().width,
                    firstTop: first.getBoundingClientRect().top,
                    secondTop: second.getBoundingClientRect().top,
                    media: matchMedia('(min-width: 1000px)').matches,
                    unsupportedMedia: matchMedia('(prefers-reduced-transparency: reduce)').matches,
                    hidden: document.hidden,
                    idbOpen: !!indexedDB.open('shim-test'),
                  };
                })()"#,
            )
            .expect("enhanced eval");
        assert_eq!(v.get("mode").and_then(|v| v.as_str()), Some("enhanced"));
        assert!(v.get("width").and_then(|v| v.as_u64()).unwrap_or(0) > 0);
        assert!(
            v.get("secondTop").and_then(|v| v.as_i64()).unwrap_or(0)
                > v.get("firstTop").and_then(|v| v.as_i64()).unwrap_or(0)
        );
        assert_eq!(v.get("media").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            v.get("unsupportedMedia").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(v.get("hidden").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(v.get("idbOpen").and_then(|v| v.as_bool()), Some(true));

        let custom = enhanced
            .eval(
                r#"(() => {
                  class XProbe extends HTMLElement {
                    connectedCallback() { this.textContent = 'connected'; }
                  }
                  customElements.define('x-probe', XProbe);
                  const el = document.createElement('x-probe');
                  document.body.appendChild(el);
                  const t = document.createElement('template');
                  t.innerHTML = '<p id="inside">templated</p>';
                  const frag = document.adoptNode(t.content);
                  return {
                    defined: !!customElements.get('x-probe'),
                    connected: el.textContent,
                    templateText: frag.querySelector('#inside').textContent,
                    moduleLike: __looksLikeModuleSource('const x = import.meta.url; export { x };'),
                  };
                })()"#,
            )
            .expect("custom elements eval");
        assert_eq!(custom.get("defined").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            custom.get("connected").and_then(|v| v.as_str()),
            Some("connected")
        );
        assert_eq!(
            custom.get("templateText").and_then(|v| v.as_str()),
            Some("templated")
        );
        assert_eq!(
            custom.get("moduleLike").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn extract_cards_commerce_avoids_broad_text_false_positives() {
        let profile = Profile::load(profile::DEFAULT_PROFILE).expect("default profile");
        let session = Session::new(&profile, false, ShimMode::Stable).expect("session");
        let rows = session
            .eval(
                r#"(() => {
                  document.body.innerHTML = `
                    <main>
                      <div class="product-card" data-price="$19.99" data-availability="InStock" data-condition="NewCondition">
                        <a href="/p/1"><h2>Root Attr Product</h2></a>
                      </div>
                      <div class="product-card">
                        <a href="/plans/2024"><h2>Pricing Guide</h2></a>
                        <a class="price-link" href="/plans/2024">See pricing</a>
                      </div>
                    </main>`;
                  return __extractCards('.product-card', 10, 'product');
                })()"#,
            )
            .expect("extract cards");
        let rows = rows.as_array().expect("rows array");
        let root = rows
            .iter()
            .find(|row| row.get("title").and_then(|v| v.as_str()) == Some("Root Attr Product"))
            .expect("root attr product");
        assert_eq!(root.get("price").and_then(|v| v.as_str()), Some("$19.99"));
        assert_eq!(
            root.get("availability").and_then(|v| v.as_str()),
            Some("in_stock")
        );
        assert_eq!(root.get("condition").and_then(|v| v.as_str()), Some("new"));

        let guide = rows
            .iter()
            .find(|row| row.get("title").and_then(|v| v.as_str()) == Some("Pricing Guide"))
            .expect("pricing guide");
        assert!(guide.get("price").is_none_or(|v| v.is_null()));
    }

    #[test]
    fn page_model_does_not_promote_new_research_to_product() {
        let profile = Profile::load(profile::DEFAULT_PROFILE).expect("default profile");
        let session = Session::new(&profile, false, ShimMode::Stable).expect("session");
        let model = session
            .eval(
                r#"(() => {
                  document.body.innerHTML = `
                    <main>
                      <div class="card">
                        <a href="/research/new-york"><h2>New York research grants</h2></a>
                        <p>Available documentation covers new research methods.</p>
                      </div>
                    </main>`;
                  return __pageModel({types: ['card', 'product_card'], limit: 10});
                })()"#,
            )
            .expect("page model");
        let objects = model
            .get("objects")
            .and_then(|v| v.as_array())
            .expect("objects array");
        let card = objects
            .iter()
            .find(|obj| {
                obj.get("title").and_then(|v| v.as_str()) == Some("New York research grants")
            })
            .expect("research card");
        assert_eq!(card.get("kind").and_then(|v| v.as_str()), Some("card"));
        let fields = card.get("fields").expect("fields");
        assert!(fields.get("condition").is_none());
        assert!(fields.get("availability").is_none());
    }

    #[test]
    fn blockmap_script_heavy_shell_requires_sparse_visible_content() {
        let profile = Profile::load(profile::DEFAULT_PROFILE).expect("default profile");
        let session = Session::new(&profile, false, ShimMode::Stable).expect("session");
        let density = session
            .eval(
                r#"(() => {
                  var scripts = '';
                  for (var i = 0; i < 25; i++) scripts += '<script src="/chunk-' + i + '.js"></script>';
                  document.body.innerHTML = '<main><article><h1>Useful Report</h1><p>' + 'Readable SSR content. '.repeat(300) + '</p></article></main>' + scripts;
                  return __blockmap().density;
                })()"#,
            )
            .expect("blockmap");
        assert_eq!(
            density.get("script_heavy_shell").and_then(|v| v.as_bool()),
            Some(false)
        );

        let density = session
            .eval(
                r#"(() => {
                  var scripts = '';
                  for (var i = 0; i < 25; i++) scripts += '<script src="/app-' + i + '.js"></script>';
                  document.body.innerHTML = '<div id="root">Loading...</div>' + scripts;
                  return __blockmap().density;
                })()"#,
            )
            .expect("blockmap shell");
        assert_eq!(
            density.get("script_heavy_shell").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn fragment_target_check_uses_the_seeded_dom_not_script_text() {
        let profile = Profile::load(profile::DEFAULT_PROFILE).expect("default profile");
        let session = Session::new(&profile, false, ShimMode::Stable).expect("session");
        session
            .eval(
                r#"document.body.innerHTML = '<script>var example = "id=\\"details\\"";</script><section id="real-target"></section><a name="legacy"></a>'"#,
            )
            .expect("seed fragment targets");

        assert_eq!(
            session.fragment_target_present("https://example.com/#details"),
            Some(false),
            "an id-looking string inside a script is not a DOM target"
        );
        assert_eq!(
            session.fragment_target_present("https://example.com/#real-target"),
            Some(true)
        );
        assert_eq!(
            session.fragment_target_present("https://example.com/#legacy"),
            Some(true)
        );
    }

    #[test]
    fn fragment_target_check_accepts_decoded_existing_target() {
        let profile = Profile::load(profile::DEFAULT_PROFILE).expect("default profile");
        let session = Session::new(&profile, false, ShimMode::Stable).expect("session");
        session
            .eval(r#"document.body.innerHTML = '<section id="section one"></section>'"#)
            .expect("seed encoded fragment target");
        assert_eq!(
            session.fragment_target_present("https://example.com/#section%20one"),
            Some(true)
        );
        assert_eq!(
            session.fragment_target_present("https://example.com/#:~:text=section%20one"),
            None,
            "text fragments are excluded from this detector"
        );
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::{
        discovery_page_owned, discovery_route_key, parse_discover_options,
        raw_network_route_candidates, static_dom_route_keys_from_body, upsert_discovery_route,
    };
    use crate::network_store::{ContentKind, NetworkCapture};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn discovery_route_key_normalizes_common_duplicates() {
        assert_eq!(
            discovery_route_key("https://example.com/docs/?utm_source=x&b=2&a=1#top"),
            "https://example.com/docs?a=1&b=2"
        );
    }

    #[test]
    fn discovery_page_owned_rejects_sibling_suffix_domains() {
        assert!(discovery_page_owned(
            "https://example.com/start",
            "https://docs.example.com/path"
        ));
        assert!(discovery_page_owned(
            "https://example.com/start",
            "https://example.com/path"
        ));
        assert!(!discovery_page_owned(
            "https://example.com/start",
            "https://evil-example.com/path"
        ));
        assert!(!discovery_page_owned(
            "https://app.example.com/start",
            "https://example.com/path"
        ));
    }

    #[test]
    fn static_dom_route_keys_parse_initial_html_without_navigation() {
        let keys = static_dom_route_keys_from_body(
            r#"<a href="/docs/">Docs</a><form action="/search"></form>"#,
            "https://example.com/base/page",
        );
        assert!(keys.contains("https://example.com/docs"));
        assert!(keys.contains("https://example.com/search"));
    }

    #[test]
    fn upsert_discovery_route_keeps_unique_sources() {
        let mut routes = Vec::new();
        let mut index = HashMap::new();
        let item = json!({"url": "https://example.com/docs", "score": 0.4});

        upsert_discovery_route(
            &mut routes,
            &mut index,
            &item,
            "static_dom",
            "document_route",
            0.7,
            "https://example.com/",
        );
        upsert_discovery_route(
            &mut routes,
            &mut index,
            &item,
            "static_dom",
            "document_route",
            0.7,
            "https://example.com/",
        );
        upsert_discovery_route(
            &mut routes,
            &mut index,
            &json!({"url": "https://example.com/docs/", "score": 0.8}),
            "network_json",
            "network_route",
            0.6,
            "https://example.com/",
        );

        assert_eq!(routes.len(), 1);
        assert_eq!(
            routes[0].get("source").and_then(|v| v.as_str()),
            Some("static_dom")
        );
        let sources = routes[0]
            .get("sources")
            .and_then(|v| v.as_array())
            .expect("sources array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>();
        assert_eq!(sources, vec!["static_dom", "network_json"]);
        assert_eq!(routes[0].get("score").and_then(|v| v.as_f64()), Some(0.8));
    }

    #[test]
    fn parse_discover_options_rejects_invalid_surface_params() {
        assert!(parse_discover_options(&json!({"url": "not a url"})).is_err());
        assert!(parse_discover_options(&json!({"url": 42})).is_err());
        assert!(parse_discover_options(&json!({"url": "ftp://example.com"})).is_err());
        assert!(parse_discover_options(&json!({"limit": 0})).is_err());
        assert!(parse_discover_options(&json!({"limit": "many"})).is_err());
        assert!(parse_discover_options(&json!({"limit": 201})).is_err());
        assert!(
            parse_discover_options(&json!({"url": "https://example.com", "limit": 200})).is_ok()
        );
    }

    #[test]
    fn raw_network_route_candidates_respects_depth_limit() {
        let mut nested = json!("/too-deep");
        for _ in 0..40 {
            nested = json!({"url": nested});
        }
        let capture = NetworkCapture {
            capture_id: 1,
            url: "https://example.com/api/discovery".to_string(),
            method: "GET".to_string(),
            status: 200,
            content_type: "application/json".to_string(),
            kind: ContentKind::Json,
            body_bytes: 10,
            body_preview: serde_json::to_string(&nested).expect("nested json"),
            body_truncated: false,
            captured_at_ms: 0,
            score: 100,
            navigation_id: Some("nav_1".to_string()),
        };
        assert!(raw_network_route_candidates(&capture, 10).is_empty());
    }
}

#[cfg(test)]
mod outcome_tests {
    //! Tests for `derive_outcome` (synthetic outcome derivation).
    //!
    //! Each test feeds a hand-built navigate-result fragment and asserts
    //! the derived (success, reasons) verdict. The function is pure so
    //! these don't need a session, runtime, or QuickJS.

    use super::{
        BLOCKMAP_MIN_STRUCTURE, EXTRACT_OBJECT_MIN_KEYS, TIEBREAKER_MIN_TITLE_LEN, derive_outcome,
        derive_tool_likelihoods,
    };
    use serde_json::{Value, json};

    fn empty_blockmap() -> Value {
        json!({
            "title": "",
            "structure": [],
            "headings": [],
            "interactives": { "links": 0, "buttons": 0, "inputs": [], "forms": [] },
        })
    }

    #[test]
    fn derive_outcome_extracts_pass() {
        // JSON-LD strategy returned 4-key object — strong success.
        let extract = json!({
            "strategy": "json_ld",
            "confidence": 0.95,
            "data": {
                "@context": "https://schema.org",
                "@type": "NewsArticle",
                "headline": "Markets close mixed",
                "datePublished": "2026-05-02",
            },
        });
        let (success, reasons, signals) = derive_outcome(
            200,
            true,
            &Value::Null,
            &empty_blockmap(),
            &extract,
            &Value::Null,
            &Value::Null,
        );
        assert!(success, "extract with 4 keys should pass");
        assert!(
            reasons.iter().any(|r| r.starts_with("extract:")),
            "reasons should include extract:..., got {reasons:?}"
        );
        assert_eq!(
            signals.get("extract_present").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            signals.get("extract_strategy").and_then(|v| v.as_str()),
            Some("json_ld")
        );
        // Sanity check: the threshold the test pivots on hasn't drifted.
        const { assert!(EXTRACT_OBJECT_MIN_KEYS <= 4) };
    }

    #[test]
    fn derive_outcome_blocked_by_challenge() {
        let challenge = json!({
            "blocked": true,
            "provider": "perimeterx_block",
            "confidence": 0.95,
            "status": 403,
        });
        // Status is also non-2xx but challenge takes precedence in reasons.
        let (success, reasons, signals) = derive_outcome(
            403,
            true,
            &challenge,
            &empty_blockmap(),
            &Value::Null,
            &Value::Null,
            &Value::Null,
        );
        assert!(!success, "challenge should fail the navigate");
        assert!(
            reasons.iter().any(|r| r.starts_with("challenge:")),
            "reasons should include challenge:..., got {reasons:?}"
        );
        assert_eq!(
            signals.get("challenge").and_then(|v| v.as_str()),
            Some("perimeterx_block")
        );
    }

    #[test]
    fn derive_outcome_all_scripts_failed() {
        // 5 scripts attempted (3 inline + 2 external), 0 executed → fail.
        // No challenge, status 200 — only the script-pathology check fires.
        let scripts = json!({
            "inline_count": 3,
            "external_count": 2,
            "executed": 0,
            "interrupted": 0,
        });
        let (success, reasons, signals) = derive_outcome(
            200,
            true,
            &Value::Null,
            &empty_blockmap(),
            &Value::Null,
            &Value::Null,
            &scripts,
        );
        assert!(
            !success,
            "0 executed of 5 attempted should fail the navigate"
        );
        assert!(
            reasons.iter().any(|r| r.starts_with("scripts_all_failed:")),
            "reasons should include scripts_all_failed:..., got {reasons:?}"
        );
        assert_eq!(
            signals.get("scripts_total").and_then(|v| v.as_u64()),
            Some(5)
        );
    }

    #[test]
    fn derive_outcome_thin_shell_with_title() {
        // No extract, no structure, no network captures, status OK,
        // exec_scripts off — but blockmap has a title. Tie-breaker fires
        // and we count this as a (weak) success.
        let blockmap = json!({
            "title": "Sign in to Example",
            "structure": [],
            "headings": [],
            "interactives": { "links": 0, "buttons": 0, "inputs": [], "forms": [] },
        });
        let (success, reasons, _signals) = derive_outcome(
            200,
            false,
            &Value::Null,
            &blockmap,
            &Value::Null,
            &Value::Null,
            &Value::Null,
        );
        assert!(success, "title-only page should weak-pass via tie-breaker");
        assert!(
            reasons.iter().any(|r| r.starts_with("title_only:")),
            "reasons should include title_only:..., got {reasons:?}"
        );
        // Threshold sanity-check.
        assert!("Sign in to Example".len() >= TIEBREAKER_MIN_TITLE_LEN);
    }

    // === Additional coverage beyond the four required tests ===

    #[test]
    fn derive_outcome_blockmap_with_interactives_passes() {
        // 3 structure entries, total interactives > 0, no extract / network.
        let blockmap = json!({
            "title": "Hacker News",
            "structure": [
                {"role": "header", "ref": "e:1", "counts": {"links": 5, "buttons": 0, "inputs": 1}},
                {"role": "main", "ref": "e:2", "counts": {"links": 100, "buttons": 0, "inputs": 0}},
                {"role": "footer", "ref": "e:3", "counts": {"links": 8, "buttons": 0, "inputs": 0}},
            ],
            "headings": [],
        });
        let (success, reasons, signals) = derive_outcome(
            200,
            true,
            &Value::Null,
            &blockmap,
            &Value::Null,
            &Value::Null,
            &Value::Null,
        );
        assert!(success);
        assert!(reasons.iter().any(|r| r.starts_with("blockmap:")));
        assert_eq!(
            signals
                .get("blockmap_structure_count")
                .and_then(|v| v.as_u64()),
            Some(3)
        );
        const { assert!(BLOCKMAP_MIN_STRUCTURE <= 3) };
    }

    #[test]
    fn derive_outcome_network_capture_passes() {
        let network = json!({"count": 2, "total_bytes": 12345, "top": []});
        let (success, reasons, signals) = derive_outcome(
            200,
            true,
            &Value::Null,
            &empty_blockmap(),
            &Value::Null,
            &network,
            &Value::Null,
        );
        assert!(success);
        assert!(reasons.iter().any(|r| r.starts_with("network_stores:")));
        assert_eq!(
            signals
                .get("network_capture_count")
                .and_then(|v| v.as_u64()),
            Some(2)
        );
    }

    #[test]
    fn derive_outcome_404_fails() {
        let (success, reasons, _) = derive_outcome(
            404,
            true,
            &Value::Null,
            &empty_blockmap(),
            &Value::Null,
            &Value::Null,
            &Value::Null,
        );
        assert!(!success);
        assert!(reasons.iter().any(|r| r.starts_with("status:404")));
    }

    #[test]
    fn derive_outcome_no_signal_fails() {
        // 200, no extract, no structure, no captures, no headings, no title.
        let (success, reasons, _) = derive_outcome(
            200,
            false,
            &Value::Null,
            &empty_blockmap(),
            &Value::Null,
            &Value::Null,
            &Value::Null,
        );
        assert!(!success);
        assert!(reasons.iter().any(|r| r == "no_signal"));
    }

    #[test]
    fn derive_outcome_truncated_extract_passes() {
        // Extract result was so big it got the truncated stub. Strategy
        // fired — that's a success even though data is null.
        let extract = json!({
            "strategy": "next_data",
            "confidence": 0.9,
            "data": null,
            "truncated": true,
            "size_bytes": 800000,
        });
        let (success, reasons, _) = derive_outcome(
            200,
            true,
            &Value::Null,
            &empty_blockmap(),
            &extract,
            &Value::Null,
            &Value::Null,
        );
        assert!(success);
        assert!(reasons.iter().any(|r| r.starts_with("extract:next_data")));
    }

    #[test]
    fn derive_outcome_majority_interrupted_fails() {
        // 6 of 10 scripts hit the watchdog → pathological page.
        let scripts = json!({
            "inline_count": 5,
            "external_count": 5,
            "executed": 10,
            "interrupted": 6,
        });
        // Even with a usable blockmap, the script-pathology check fires
        // first as a strong-failure signal.
        let blockmap = json!({
            "title": "Some site",
            "structure": [
                {"role": "header", "ref": "e:1", "counts": {"links": 5, "buttons": 0, "inputs": 0}},
                {"role": "main", "ref": "e:2", "counts": {"links": 10, "buttons": 0, "inputs": 0}},
                {"role": "footer", "ref": "e:3", "counts": {"links": 3, "buttons": 0, "inputs": 0}},
            ],
            "headings": [],
        });
        let (success, reasons, _) = derive_outcome(
            200,
            true,
            &Value::Null,
            &blockmap,
            &Value::Null,
            &Value::Null,
            &scripts,
        );
        assert!(!success);
        assert!(
            reasons
                .iter()
                .any(|r| r.starts_with("scripts_pathological:"))
        );
    }

    #[test]
    fn tool_likelihoods_selector_rich_page_prefers_query_text() {
        let blockmap = json!({
            "title": "BBC News",
            "structure": [
                {"role": "header", "ref": "e:1", "counts": {"links": 20, "buttons": 2, "inputs": 1}},
                {"role": "main", "ref": "e:2", "counts": {"links": 58, "buttons": 4, "inputs": 2}},
                {"role": "section", "ref": "e:3", "counts": {"links": 12, "buttons": 0, "inputs": 0}},
                {"role": "footer", "ref": "e:4", "counts": {"links": 8, "buttons": 0, "inputs": 0}},
            ],
            "headings": [
                {"level": 1, "ref": "e:10", "text": "News"},
                {"level": 2, "ref": "e:11", "text": "Top story"},
                {"level": 2, "ref": "e:12", "text": "Also in news"}
            ],
            "main_headings": [
                {"level": 1, "ref": "e:10", "text": "News"},
                {"level": 2, "ref": "e:11", "text": "Top story"},
                {"level": 2, "ref": "e:12", "text": "Also in news"}
            ],
            "selectors": {"data_testid": 120, "aria_label": 40, "role": 5},
            "interactives": {"links": 98, "buttons": 6, "inputs": [], "forms": []},
            "density": {"tables": null, "td": null, "li": null, "json_scripts": 1, "thin_shell": false, "likely_js_filled": false},
        });
        let extract = json!({
            "strategy": "next_data",
            "confidence": 0.97,
            "data": {"headline": "Top story"},
        });
        let network = json!({"count": 0, "total_bytes": 0, "top": []});
        let scripts =
            json!({"inline_count": 3, "external_count": 2, "executed": 5, "interrupted": 0});

        let advice = derive_tool_likelihoods(
            200,
            false,
            &blockmap,
            &extract,
            &network,
            &Value::Null,
            &scripts,
        );

        let recs = advice
            .get("tool_recommendations")
            .and_then(|v| v.as_array())
            .expect("tool_recommendations array");
        assert_eq!(recs[0].as_str(), Some("query_text"));
        assert!(
            advice
                .get("tool_likelihoods")
                .and_then(|v| v.get("query_text"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                > advice
                    .get("tool_likelihoods")
                    .and_then(|v| v.get("query"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
        );
    }

    #[test]
    fn tool_likelihoods_data_page_prefers_extract() {
        let blockmap = json!({
            "title": "Home - Financial Times",
            "structure": [{"role": "main", "ref": "e:2", "counts": {"links": 6, "buttons": 1, "inputs": 0}}],
            "headings": [],
            "main_headings": [],
            "selectors": {"data_testid": 0, "aria_label": 0, "role": 0},
            "interactives": {"links": 6, "buttons": 1, "inputs": [], "forms": []},
            "density": {"tables": null, "td": null, "li": null, "json_scripts": 6, "thin_shell": false, "likely_js_filled": false},
        });
        let extract = json!({
            "strategy": "json_ld",
            "confidence": 0.95,
            "data": {"@context": "https://schema.org", "@type": "WebSite", "name": "FT"},
        });
        let network = json!({"count": 2, "total_bytes": 18244, "top": []});
        let scripts =
            json!({"inline_count": 0, "external_count": 0, "executed": 0, "interrupted": 0});

        let advice = derive_tool_likelihoods(
            200,
            false,
            &blockmap,
            &extract,
            &network,
            &Value::Null,
            &scripts,
        );

        let recs = advice
            .get("tool_recommendations")
            .and_then(|v| v.as_array())
            .expect("tool_recommendations array");
        assert_eq!(recs[0].as_str(), Some("extract"));
    }

    #[test]
    fn tool_likelihoods_thin_shell_prefers_chrome() {
        let blockmap = json!({
            "title": "Loading...",
            "structure": [],
            "headings": [],
            "main_headings": [],
            "selectors": {"data_testid": 0, "aria_label": 0, "role": 0},
            "interactives": {"links": 0, "buttons": 0, "inputs": [], "forms": []},
            "density": {"tables": null, "td": null, "li": null, "json_scripts": 0, "thin_shell": true, "likely_js_filled": true},
        });
        let scripts =
            json!({"inline_count": 2, "external_count": 4, "executed": 1, "interrupted": 5});

        let advice = derive_tool_likelihoods(
            200,
            true,
            &blockmap,
            &Value::Null,
            &Value::Null,
            &Value::Null,
            &scripts,
        );

        let recs = advice
            .get("tool_recommendations")
            .and_then(|v| v.as_array())
            .expect("tool_recommendations array");
        assert_eq!(recs[0].as_str(), Some("chrome_escalation"));
    }

    #[test]
    fn tool_likelihoods_script_heavy_shell_prefers_chrome() {
        let blockmap = json!({
            "title": "Pinterest",
            "structure": [],
            "headings": [],
            "main_headings": [],
            "selectors": {"data_testid": 0, "aria_label": 2, "role": 1},
            "interactives": {"links": 4, "buttons": 0, "inputs": [], "forms": []},
            "density": {"tables": null, "td": null, "li": null, "json_scripts": 0, "script_tags": 42, "script_heavy_shell": true, "thin_shell": false, "likely_js_filled": true},
        });
        let advice = derive_tool_likelihoods(
            200,
            false,
            &blockmap,
            &Value::Null,
            &Value::Null,
            &Value::Null,
            &Value::Null,
        );
        let recs = advice
            .get("tool_recommendations")
            .and_then(|v| v.as_array())
            .expect("tool_recommendations array");
        assert_eq!(recs[0].as_str(), Some("chrome_escalation"));
    }

    #[test]
    fn tool_likelihoods_all_zero_signals_remain_finite() {
        let advice = derive_tool_likelihoods(
            200,
            false,
            &empty_blockmap(),
            &Value::Null,
            &Value::Null,
            &Value::Null,
            &Value::Null,
        );

        let probs = advice
            .get("tool_likelihoods")
            .and_then(|v| v.as_object())
            .expect("tool_likelihoods object");
        let sum: f64 = probs
            .iter()
            .filter(|(k, _)| k.as_str() != "confidence" && k.as_str() != "margin")
            .map(|(_, v)| v.as_f64().unwrap_or(f64::NAN))
            .sum();
        assert!(sum.is_finite());
        assert!((sum - 1.0).abs() < 1e-9);
        for value in probs.values() {
            assert!(value.as_f64().unwrap_or(f64::NAN).is_finite());
        }
        assert!(
            advice
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(f64::NAN)
                .is_finite()
        );
        assert!(
            advice
                .get("margin")
                .and_then(|v| v.as_f64())
                .unwrap_or(f64::NAN)
                .is_finite()
        );
        assert_eq!(
            advice
                .get("tool_recommendations")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty()),
            Some(true)
        );
    }

    #[test]
    fn tool_likelihoods_conflicting_signals_still_downweight_shells() {
        let blockmap = json!({
            "title": "Hybrid Page",
            "structure": [
                {"role": "main", "ref": "e:2", "counts": {"links": 50, "buttons": 3, "inputs": 2}}
            ],
            "headings": [{"level": 1, "ref": "e:10", "text": "Loading"}],
            "main_headings": [{"level": 1, "ref": "e:10", "text": "Loading"}],
            "selectors": {"data_testid": 90, "aria_label": 18, "role": 4},
            "interactives": {"links": 50, "buttons": 3, "inputs": [], "forms": []},
            "density": {"tables": null, "td": null, "li": null, "json_scripts": 0, "thin_shell": true, "likely_js_filled": true},
        });
        let advice = derive_tool_likelihoods(
            200,
            true,
            &blockmap,
            &Value::Null,
            &Value::Null,
            &Value::Null,
            &json!({"inline_count": 2, "external_count": 1, "executed": 1, "interrupted": 2}),
        );

        let recs = advice
            .get("tool_recommendations")
            .and_then(|v| v.as_array())
            .expect("tool_recommendations array");
        assert_eq!(recs[0].as_str(), Some("chrome_escalation"));
    }
}

#[cfg(test)]
mod network_extract_tests {
    use super::{extract_network_objects_from_capture, network_store, network_terms};

    fn capture(body: &str) -> network_store::NetworkCapture {
        network_store::NetworkCapture {
            capture_id: 7,
            url: "https://api.example.com/v1/items".to_string(),
            method: "GET".to_string(),
            status: 200,
            content_type: "application/json".to_string(),
            body_bytes: body.len(),
            body_truncated: false,
            body_preview: body.to_string(),
            captured_at_ms: 0,
            score: 55,
            kind: network_store::ContentKind::Json,
            navigation_id: Some("nav_1".to_string()),
        }
    }

    #[test]
    fn extracts_named_array_items() {
        let cap = capture(
            r#"{
                "items": [
                    {"id": 1, "name": "Alpha Jacket", "price": "$19", "url": "/p/alpha"},
                    {"id": 2, "name": "Beta Jacket", "price": "$29", "url": "/p/beta"}
                ]
            }"#,
        );
        let terms = network_terms("alpha jacket");
        let objects = extract_network_objects_from_capture(&cap, &terms, 20).unwrap();
        let alpha = objects
            .iter()
            .find(|o| o.title.as_deref() == Some("Alpha Jacket"))
            .expect("alpha object");
        assert_eq!(alpha.kind, "product_card");
        assert_eq!(
            alpha.url.as_deref(),
            Some("https://api.example.com/p/alpha")
        );
        assert!(alpha.matched_terms.contains(&"alpha".to_string()));
    }

    #[test]
    fn redacts_sensitive_fields() {
        let cap = capture(r#"{"name":"Viewer","token":"secret-token","id":"u1"}"#);
        let objects = extract_network_objects_from_capture(&cap, &[], 10).unwrap();
        assert_eq!(objects[0].fields.get("token").unwrap(), "[REDACTED]");
    }
}

#[cfg(test)]
mod decision_record_tests {
    use super::DecisionRecord;

    #[test]
    fn skip_action_yields_block_key() {
        let d = DecisionRecord {
            action: "skip",
            host: "zephr-templates.cnbc.com".to_string(),
        };
        assert_eq!(
            d.decision_key().as_deref(),
            Some("block:zephr-templates.cnbc.com")
        );
    }

    #[test]
    fn queued_action_yields_allow_key() {
        let d = DecisionRecord {
            action: "queued",
            host: "i.cnbc.com".to_string(),
        };
        assert_eq!(d.decision_key().as_deref(), Some("allow:i.cnbc.com"));
    }

    #[test]
    fn empty_host_drops_key() {
        let d = DecisionRecord {
            action: "skip",
            host: String::new(),
        };
        assert!(d.decision_key().is_none());
    }

    #[test]
    fn unknown_action_drops_key() {
        // fetch_failed and any other action should not produce a key —
        // they're not policy choices T2 should bucket on.
        let d = DecisionRecord {
            action: "fetch_failed",
            host: "cdn.example.com".to_string(),
        };
        assert!(d.decision_key().is_none());
    }
}
