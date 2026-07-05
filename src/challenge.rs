// Anti-bot challenge detection and auto-solving.
//
// Two-phase design:
//   1. `detect(status, body)` — classify the page against known vendor signatures.
//      Returns a `Detection` on any match (or None on the happy path).
//   2. `solve_url(detection, body, current_url)` — dispatch to a vendor-specific
//      solver.  Returns Some(solution_url) when the challenge can be resolved
//      without real Chrome; None means escalation is required.
//
// Adding a new solver:
//   a. Add a match arm in `solve_url` pointing to a new `solve_<vendor>` fn.
//   b. The fn signature is always (body: &str, current_url: &str) -> Option<String>.
//   c. Add a unit test in the tests module below.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Value, json};

const HINT_ESCALATE: &str = "Solve once in real Chrome (or via unchainedsky CLI), copy the clearance \
     cookie via DevTools, paste with cookies_set, then retry navigate. \
     Cookie typically lasts 30 min \u{2013} 24 h.";

const HINT_BODY: &str = "Inspect `body` to identify the vendor, escalate to real Chrome to confirm \
     the page renders, or skip this URL.";

// `missing_primary_action` is intentionally weak: it is useful as a detector
// signal for JS-only shells, but too noisy to force Chrome unless confidence is
// higher than ordinary browser-route reasons. Keep these paired so detector and
// routing advice drift is obvious in review.
pub const MISSING_PRIMARY_ACTION_DETECTION_CONFIDENCE: f64 = 0.70;
pub const MISSING_PRIMARY_ACTION_ESCALATION_THRESHOLD: f64 = 0.85;

// ── Detection result ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Detection {
    pub blocked: bool,
    pub provider: &'static str,
    pub confidence: f64,
    pub status: u16,
    pub matched: Vec<&'static str>,
    pub clearance_cookie: Option<&'static str>,
    pub reason: String,
    pub hint: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RateLimit {
    pub limited: bool,
    pub status: u16,
    pub retry_after: Option<String>,
    pub retry_after_seconds: Option<u64>,
    pub reason: String,
    pub hint: &'static str,
}

// ── Detection ────────────────────────────────────────────────────────────────

/// Classify the response against known vendor signatures.
///
/// Returns the best match, or `None` on the happy path.
///
/// Vendor-specific signatures outrank generic interstitial copy even when the
/// generic copy has a higher standalone confidence. Real block pages often mix
/// both, and the vendor result carries the actionable clearance-cookie hint.
/// Aligned with private-core's `challenge_detection.py` — same vendor names
/// and confidence ladder.
pub fn detect(status: u16, body: &str) -> Option<Detection> {
    // Cheap early-out: large 2xx bodies are almost certainly real content.
    // Real challenge pages are typically under 50 KB; 80 KB buys headroom
    // while still catching eBay's ~13 KB "Pardon Our Interruption" page.
    if (200..300).contains(&status) && body.len() > 80_000 {
        return None;
    }
    let lower = body.to_lowercase();

    // (vendor, confidence, patterns, clearance_cookie_hint)
    // Patterns are matched case-insensitively (body is lowercased once above).
    // Substring match — no regex crate needed.
    type Group = (&'static str, f64, &'static [&'static str], &'static str);
    let groups: &[Group] = &[
        ("arkose_labs", 0.98, &["arkoselabs", "funcaptcha"], ""),
        (
            "interstitial",
            0.99,
            &[
                "pardon our interruption",
                "are you a robot",
                "are you a human",
                "automated access has been blocked",
                "your browser has been flagged",
                "as you were browsing",
            ],
            "",
        ),
        (
            "cloudflare_turnstile",
            0.97,
            &[
                "just a moment",
                "checking your browser",
                "verifying you are human",
                "needs to review the security of your connection",
                "performance & security by cloudflare",
                "cf-challenge",
                "cf_challenge",
                "turnstile",
                "__cf_chl_",
                "cf-mitigated",
            ],
            "cf_clearance",
        ),
        (
            "aws_waf",
            0.96,
            &[
                "awswafcookiedomainlist",
                "gokuprops",
                "aws-waf-token",
                "aws-waf-client",
                "/awswaf/",
            ],
            "aws-waf-token",
        ),
        // Reddit's network-level block — TLS fingerprint or header-based.
        // Body is a large CSS-heavy page saying "You've been blocked by
        // network security." with a link to file a support ticket. Not
        // solvable without real Chrome; confidence 0.90 so it beats
        // generic_human_verification (0.76) and outranks unknown_block.
        (
            "reddit_network_block",
            0.90,
            &["you've been blocked by network security"],
            "",
        ),
        // Reddit's JS proof-of-work — solvable without real Chrome.
        // Confidence is 0.95 so it beats generic_human_verification (0.76)
        // when both patterns fire on the same page.
        ("reddit_js_challenge", 0.95, &["await(async e=>e+e)(\""], ""),
        (
            "recaptcha",
            0.95,
            &[
                "g-recaptcha",
                "google recaptcha",
                "recaptcha/api2",
                "i'm not a robot",
                "im not a robot",
            ],
            "",
        ),
        (
            "perimeterx_block",
            0.94,
            &[
                "px-captcha",
                "_pxappid",
                "/_px",
                "robot or human",
                "/blocked?url=",
            ],
            "_px3",
        ),
        (
            "datadome",
            0.93,
            &["datadome", "captcha-delivery"],
            "datadome",
        ),
        (
            "press_hold",
            0.92,
            &[
                "press & hold",
                "press and hold",
                "press&hold",
                "hold to confirm",
            ],
            "",
        ),
        (
            "yahoo_sad_panda",
            0.90,
            &[
                "sad-panda",
                "sorry, the page you requested cannot be found",
                "yahoo.*nytransit",
            ],
            "",
        ),
        (
            "akamai_bmp",
            0.88,
            &["_abck=", "bm_sz=", "akamai bot manager"],
            "_abck",
        ),
        (
            "imperva",
            0.85,
            &["_incapsula", "incident_id"],
            "incap_ses_*",
        ),
        (
            "generic_human_verification",
            0.76,
            &[
                "verify you are human",
                "verify that you are human",
                "verify that you're human",
                "please wait for verification",
                "please wait while we verify",
                "unusual traffic",
                "access to this page has been denied",
                "access denied",
                "automated requests",
                "sorry, you have been blocked",
            ],
            "",
        ),
    ];

    let mut best: Option<(&'static str, f64, &'static str, Vec<&'static str>)> = None;
    for &(vendor, confidence, patterns, cookie) in groups {
        let matched: Vec<&'static str> = patterns
            .iter()
            .copied()
            .filter(|&p| lower.contains(p))
            .collect();
        if !matched.is_empty()
            && best.as_ref().is_none_or(|(best_vendor, c, _, _)| {
                let rank = provider_specificity(vendor);
                let best_rank = provider_specificity(best_vendor);
                rank > best_rank || (rank == best_rank && confidence > *c)
            })
        {
            best = Some((vendor, confidence, cookie, matched));
        }
    }

    if let Some((vendor, confidence, cookie, matched)) = best {
        return Some(Detection {
            blocked: true,
            provider: vendor,
            confidence,
            status,
            matched,
            clearance_cookie: if cookie.is_empty() {
                None
            } else {
                Some(cookie)
            },
            reason: format!("Matched {vendor} challenge signatures."),
            hint: HINT_ESCALATE,
        });
    }

    // Fallback: tiny body on anomalous status with no vendor match. Rate limits
    // are reported separately by detect_rate_limit(), not as unknown bot walls.
    if is_rate_limited(status, &lower, None) {
        return None;
    }
    let anomalous = !matches!(status, 200 | 301 | 302 | 304 | 404 | 410)
        && (status >= 400 || status == 202 || status == 401 || status == 403);
    if anomalous && body.len() < 5_000 {
        return Some(Detection {
            blocked: true,
            provider: "unknown_block",
            confidence: 0.55,
            status,
            matched: vec![],
            clearance_cookie: None,
            reason: format!(
                "Tiny body ({} bytes) on anomalous status {} with no known vendor \
                 signature \u{2014} likely a soft block.",
                body.len(),
                status
            ),
            hint: HINT_BODY,
        });
    }

    None
}

pub fn detect_rate_limit(
    status: u16,
    body: &str,
    headers: &HashMap<String, String>,
) -> Option<RateLimit> {
    let lower = body.to_lowercase();
    let retry_after = headers.get("retry-after").cloned();
    if !is_rate_limited(status, &lower, retry_after.as_deref()) {
        return None;
    }
    let retry_after_seconds = retry_after.as_deref().and_then(parse_retry_after_seconds);
    let reason = if status == 429 {
        "HTTP 429 Too Many Requests".to_string()
    } else if retry_after.is_some() {
        format!("HTTP {status} with Retry-After header")
    } else {
        format!("HTTP {status} retry-later response")
    };
    Some(RateLimit {
        limited: true,
        status,
        retry_after,
        retry_after_seconds,
        reason,
        hint: "Back off this URL/domain, honor Retry-After when present, and retry later instead of escalating as a bot challenge.",
    })
}

pub fn detect_browser_route(status: u16, body: &str, blockmap: &Value) -> Option<Value> {
    if !(200..400).contains(&status) {
        return None;
    }
    let lower = body.to_lowercase();
    let title = blockmap
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let density = blockmap.get("density").unwrap_or(&Value::Null);
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
    let interactive_count = links + buttons + inputs + forms;
    let structure_count = blockmap
        .get("structure")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let main_heading_count = blockmap
        .get("main_headings")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let raw_route_surface = has_raw_route_surface(&lower);
    let script_heavy_has_usable_content =
        script_heavy_shell && (main_heading_count > 0 || structure_count > 1 || links >= 20);

    let mut evidence: Vec<&'static str> = Vec::new();
    let (reason, confidence) = if contains_any(
        &lower,
        &[
            "/httpservice/retry/enablejs",
            "trouble accessing search",
            "sg_rel",
            "enablejs",
        ],
    ) {
        evidence.push("google_enablejs_retry");
        ("enable_js_interstitial", 0.90)
    } else if contains_any(
        &lower,
        &[
            "enable javascript",
            "enable js",
            "please turn javascript on",
            "javascript is disabled",
            "requires javascript to be enabled",
            "to continue, enable javascript",
        ],
    ) || title.contains("enable javascript")
    {
        evidence.push("enable_js_text");
        ("enable_js_interstitial", 0.94)
    } else if contains_any(
        &lower,
        &[
            "mapboxgl",
            "leaflet",
            "google.maps",
            "maps.googleapis.com",
            "<canvas",
            "webgl",
        ],
    ) {
        evidence.push("canvas_or_map_signature");
        ("canvas_or_map_ui", 0.86)
    } else if thin_shell {
        evidence.push("blockmap.density.thin_shell");
        ("thin_shell", 0.88)
    } else if script_heavy_shell && !script_heavy_has_usable_content {
        evidence.push("blockmap.density.script_heavy_shell");
        ("script_heavy_shell", 0.86)
    } else if likely_js_filled && !script_heavy_has_usable_content {
        evidence.push("blockmap.density.likely_js_filled");
        ("rendered_result_required", 0.78)
    } else if structure_count == 0 && interactive_count == 0 && body.len() < 20_000 {
        evidence.push("no_structure_or_interactives");
        ("no_interactives", 0.72)
    } else if contains_any(&lower, &["search", "sign in", "checkout", "continue"])
        && interactive_count == 0
        && structure_count <= 1
        && body.len() < 20_000
        && !raw_route_surface
    {
        evidence.push("primary_action_text_without_interactives");
        (
            "missing_primary_action",
            MISSING_PRIMARY_ACTION_DETECTION_CONFIDENCE,
        )
    } else {
        return None;
    };

    Some(json!({
        "needed": true,
        "reason": reason,
        "confidence": confidence,
        "evidence": evidence,
        "hint": "Route this page to real browser automation; unbrowser should not keep retrying the same response.",
    }))
}

fn has_raw_route_surface(lower_body: &str) -> bool {
    contains_any(lower_body, &["<a", "<area", "<form", "href", "action"])
}

fn is_rate_limited(status: u16, lower_body: &str, retry_after: Option<&str>) -> bool {
    status == 429
        || (status == 503
            && (retry_after.is_some()
                || contains_any(
                    lower_body,
                    &[
                        "rate limit",
                        "too many requests",
                        "retry later",
                        "try again later",
                        "slow down",
                    ],
                )))
}

fn parse_retry_after_seconds(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn provider_specificity(provider: &str) -> u8 {
    match provider {
        "interstitial" | "generic_human_verification" => 0,
        _ => 1,
    }
}

// ── Solver dispatch ──────────────────────────────────────────────────────────

/// Try to auto-solve a challenge without real Chrome.
///
/// Returns `Some(solution_url)` when the challenge is deterministically
/// solvable; the caller should navigate to that URL.
/// Returns `None` when escalation to a real browser is required.
pub fn solve_url(detection: &Detection, body: &str, current_url: &str) -> Option<String> {
    match detection.provider {
        "reddit_js_challenge" => solve_reddit_js(body, current_url),
        // Future solvers: one arm per provider.
        _ => None,
    }
}

// ── Reddit JS proof-of-work solver ───────────────────────────────────────────
//
// Reddit serves a small challenge page with one inline <script>:
//
//   await(async e=>e+e)("HEXVALUE")  →  solution = HEXVALUE + HEXVALUE
//
// The solution is submitted as a GET back to the original URL:
//   ?solution=<doubled>&js_challenge=1&token=<per-request hash>&jsc_orig_r=
//
// This is deterministic and requires no real browser.

fn solve_reddit_js(body: &str, current_url: &str) -> Option<String> {
    const SCRIPT_MARKER: &str = "await(async e=>e+e)(\"";
    let script_pos = body.find(SCRIPT_MARKER)?;
    let after_quote = &body[script_pos + SCRIPT_MARKER.len()..];
    let val_end = after_quote.find('"')?;
    let challenge_value = &after_quote[..val_end];

    if challenge_value.is_empty()
        || challenge_value.len() > 64
        || !challenge_value.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    let solution = format!("{0}{0}", challenge_value);

    // Extract form action from the first <form> tag.
    let form_start = body.find("<form ")?;
    let form_tag_end = body[form_start..].find('>')?;
    let form_tag = &body[form_start..form_start + form_tag_end];
    let action = extract_attr(form_tag, "action")?;

    let base = url::Url::parse(current_url).ok()?;
    let mut target = base.join(&action).ok()?;
    {
        let mut qp = target.query_pairs_mut();
        qp.append_pair("solution", &solution);
        qp.append_pair("js_challenge", "1");
        if let Some(token) = extract_hidden_input(body, "token") {
            qp.append_pair("token", &token);
        }
        let orig_r = extract_hidden_input(body, "jsc_orig_r").unwrap_or_default();
        qp.append_pair("jsc_orig_r", &orig_r);
    }
    Some(target.to_string())
}

// ── HTML parsing helpers ─────────────────────────────────────────────────────

/// Extract an attribute value from a tag string (the text between `<tag` and `>`).
/// Handles both double- and single-quoted attribute values.
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let dq = format!(r#"{}=""#, name);
    let sq = format!("{}='", name);
    if let Some(pos) = tag.find(&dq) {
        let rest = &tag[pos + dq.len()..];
        return Some(rest[..rest.find('"')?].to_string());
    }
    if let Some(pos) = tag.find(&sq) {
        let rest = &tag[pos + sq.len()..];
        return Some(rest[..rest.find('\'')?].to_string());
    }
    None
}

/// Find `<input name="NAME" ...>` in `body` and return its `value` attribute.
fn extract_hidden_input(body: &str, name: &str) -> Option<String> {
    let needle = format!(r#"name="{}""#, name);
    let pos = body.find(&needle)?;
    let tag_start = body[..pos].rfind("<input")?;
    let tag_end = body[pos..].find('>')?;
    extract_attr(&body[tag_start..pos + tag_end], "value")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const REDDIT_CHALLENGE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head><title>Reddit - Please wait for verification</title>
    <script nonce="test-nonce">
      document.addEventListener("DOMContentLoaded",async function(){var e=document.forms[0],n=(e.onsubmit=function(t){return!0},await(async e=>e+e)("a5be06c2a2c9c99d"));e.elements.namedItem("solution").value=n,e.requestSubmit()},{once:!0});
    </script>
  </head>
  <body>
    <form hidden method="GET" action="/r/programming/">
      <input type="hidden" name="solution" />
      <input type="hidden" name="js_challenge" value="1"/>
      <input type="hidden" name="token" value="deadbeef1234"/>
      <input type="hidden" name="jsc_orig_r" value=""/>
    </form>
  </body>
</html>"#;

    // detect() ----------------------------------------------------------------

    #[test]
    fn detect_reddit_network_block() {
        let body = "<html><body>You've been blocked by network security. If you think you've been blocked by mistake, file a ticket below.</body></html>";
        let d = detect(403, body).expect("should detect reddit network block");
        assert_eq!(d.provider, "reddit_network_block");
        assert!(d.confidence >= 0.9);
        assert!(d.blocked);
        assert_eq!(d.clearance_cookie, None);
    }

    #[test]
    fn detect_reddit_network_block_beats_generic() {
        let body = "<html><body><h1>Access Denied</h1><p>You've been blocked by network security.</p></body></html>";
        let d = detect(403, body).expect("should detect");
        assert_eq!(
            d.provider, "reddit_network_block",
            "reddit_network_block should beat generic_human_verification"
        );
    }

    #[test]
    fn detect_reddit_js_challenge() {
        let d = detect(200, REDDIT_CHALLENGE_HTML).expect("should detect");
        assert_eq!(d.provider, "reddit_js_challenge");
        assert!(d.confidence > 0.9);
        assert!(d.blocked);
        assert_eq!(d.clearance_cookie, None);
    }

    #[test]
    fn detect_happy_path_returns_none() {
        assert!(detect(200, "<html><body><h1>Hello</h1></body></html>").is_none());
    }

    #[test]
    fn detect_large_body_skipped() {
        let big = "x".repeat(90_000);
        assert!(detect(200, &big).is_none());
    }

    #[test]
    fn detect_cloudflare_turnstile() {
        let body = "<html>just a moment while we check your browser</html>";
        let d = detect(200, body).expect("should detect cloudflare");
        assert_eq!(d.provider, "cloudflare_turnstile");
        assert_eq!(d.clearance_cookie, Some("cf_clearance"));
    }

    #[test]
    fn detect_perimeterx_beats_generic_interstitial_copy() {
        let body = r#"<!doctype html><html><head>
            <script>window._pxAppId = "PXfake";</script>
            <script src="/_px/PXfake/init.js"></script>
        </head><body>
            <div id="px-captcha"></div>
            <h1>Are you a human?</h1>
            <p>Pardon our interruption.</p>
        </body></html>"#;
        let d = detect(403, body).expect("should detect perimeterx");
        assert_eq!(d.provider, "perimeterx_block");
        assert_eq!(d.clearance_cookie, Some("_px3"));
        assert!(d.matched.contains(&"px-captcha"));
    }

    #[test]
    fn detect_generic_interstitial_without_vendor_markers() {
        let body =
            "<html><body><h1>Are you a human?</h1><p>Pardon our interruption.</p></body></html>";
        let d = detect(403, body).expect("should detect interstitial");
        assert_eq!(d.provider, "interstitial");
        assert_eq!(d.clearance_cookie, None);
    }

    #[test]
    fn detect_generic_interstitial_with_generic_challenge_js_stays_generic() {
        let body = r#"<html><head><script src="/assets/challenge.js"></script></head>
            <body><h1>Are you a human?</h1><p>Pardon our interruption.</p></body></html>"#;
        let d = detect(403, body).expect("should detect interstitial");
        assert_eq!(d.provider, "interstitial");
        assert_eq!(d.clearance_cookie, None);
    }

    #[test]
    fn detect_unknown_block_fallback() {
        let d = detect(403, "tiny").expect("should detect unknown");
        assert_eq!(d.provider, "unknown_block");
    }

    #[test]
    fn detect_unknown_block_skipped_for_normal_404() {
        // 404 is in the allow-list so the fallback should NOT fire.
        assert!(detect(404, "not found").is_none());
    }

    #[test]
    fn detect_aws_waf_stays_challenge() {
        let body = r#"<html><script src="/awswaf/challenge.js"></script><body>aws-waf-token</body></html>"#;
        let d = detect(202, body).expect("should detect aws waf");
        assert_eq!(d.provider, "aws_waf");
        assert_eq!(d.clearance_cookie, Some("aws-waf-token"));
    }

    #[test]
    fn detect_aws_waf_client_beats_generic_interstitial_copy() {
        let body = r#"<html><head><script src="/aws-waf-client.js"></script></head>
            <body><h1>Are you a human?</h1><p>Pardon our interruption.</p></body></html>"#;
        let d = detect(202, body).expect("should detect aws waf");
        assert_eq!(d.provider, "aws_waf");
        assert_eq!(d.clearance_cookie, Some("aws-waf-token"));
        assert!(d.matched.contains(&"aws-waf-client"));
    }

    #[test]
    fn rate_limit_429_is_not_unknown_challenge() {
        let mut headers = HashMap::new();
        headers.insert("retry-after".into(), "120".into());
        let body = "you are being rate limited";
        let rl = detect_rate_limit(429, body, &headers).expect("rate limit");
        assert!(rl.limited);
        assert_eq!(rl.retry_after.as_deref(), Some("120"));
        assert_eq!(rl.retry_after_seconds, Some(120));
        assert!(detect(429, body).is_none());
    }

    #[test]
    fn browser_route_enable_js_interstitial() {
        let blockmap = json!({
            "title": "Enable JavaScript",
            "structure": [],
            "interactives": {"links": 0, "buttons": 0, "inputs": [], "forms": []},
            "density": {"thin_shell": false, "likely_js_filled": false}
        });
        let route = detect_browser_route(
            200,
            "<html><title>Enable JavaScript</title>Please enable JavaScript to continue.</html>",
            &blockmap,
        )
        .expect("browser route");
        assert_eq!(
            route.get("reason").and_then(|v| v.as_str()),
            Some("enable_js_interstitial")
        );
    }

    #[test]
    fn browser_route_google_retry_enablejs_shell() {
        let blockmap = json!({
            "title": "Google Search",
            "structure": [{"role": "main"}],
            "interactives": {"links": 2, "buttons": 0, "inputs": [], "forms": []},
            "density": {"thin_shell": false, "likely_js_filled": false}
        });
        let route = detect_browser_route(
            200,
            r#"<html><body><a href="/httpservice/retry/enablejs">retry</a><div id="SG_REL">Having trouble accessing Search?</div></body></html>"#,
            &blockmap,
        )
        .expect("browser route");
        assert_eq!(
            route.get("reason").and_then(|v| v.as_str()),
            Some("enable_js_interstitial")
        );
        assert_eq!(
            route
                .get("evidence")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str()),
            Some("google_enablejs_retry")
        );
    }

    #[test]
    fn browser_route_app_shell_no_interactives() {
        let blockmap = json!({
            "title": "Loading",
            "structure": [],
            "interactives": {"links": 0, "buttons": 0, "inputs": [], "forms": []},
            "density": {"thin_shell": true, "likely_js_filled": true}
        });
        let route = detect_browser_route(
            200,
            r#"<html><body><div id="root"></div><script src="/app.js"></script></body></html>"#,
            &blockmap,
        )
        .expect("browser route");
        assert_eq!(
            route.get("reason").and_then(|v| v.as_str()),
            Some("thin_shell")
        );
    }

    #[test]
    fn browser_route_script_heavy_shell_has_specific_reason() {
        let blockmap = json!({
            "title": "Target",
            "structure": [],
            "interactives": {"links": 6, "buttons": 0, "inputs": [], "forms": []},
            "density": {"thin_shell": false, "likely_js_filled": true, "script_heavy_shell": true, "script_tags": 38}
        });
        let route = detect_browser_route(
            200,
            r#"<html><body><div id="root">Loading...</div><script src="/app.js"></script></body></html>"#,
            &blockmap,
        )
        .expect("browser route");
        assert_eq!(
            route.get("reason").and_then(|v| v.as_str()),
            Some("script_heavy_shell")
        );
        assert_eq!(
            route
                .get("evidence")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str()),
            Some("blockmap.density.script_heavy_shell")
        );
    }

    #[test]
    fn browser_route_does_not_flag_static_route_surface() {
        let blockmap = json!({
            "title": "Usable News",
            "structure": [{"role": "main"}],
            "interactives": {"links": 0, "buttons": 0, "inputs": [], "forms": []},
            "density": {"thin_shell": false, "likely_js_filled": false}
        });
        let route = detect_browser_route(
            200,
            r#"<html><body><p>Search our archive or continue reading.</p><a href="/news/climate">Climate guide</a></body></html>"#,
            &blockmap,
        );
        assert!(
            route.is_none(),
            "static links should remain cheap-path discoverable"
        );
    }

    #[test]
    fn browser_route_script_heavy_with_main_content_stays_cheap_path() {
        let blockmap = json!({
            "title": "Usable Article",
            "structure": [{"role": "main"}, {"role": "article"}],
            "headings": [{"level": 1, "text": "Useful Report"}],
            "main_headings": [{"level": 1, "text": "Useful Report"}],
            "interactives": {"links": 3, "buttons": 0, "inputs": [], "forms": []},
            "density": {"thin_shell": false, "likely_js_filled": true, "script_heavy_shell": true, "script_tags": 30}
        });
        let route = detect_browser_route(
            200,
            r#"<html><body><main><article><h1>Useful Report</h1><p>Readable SSR content.</p></article></main></body></html>"#,
            &blockmap,
        );
        assert!(
            route.is_none(),
            "usable SSR content should not force Chrome"
        );
    }

    // solve_url() -------------------------------------------------------------

    #[test]
    fn solve_reddit_js_basic() {
        let d = detect(200, REDDIT_CHALLENGE_HTML).unwrap();
        let url = solve_url(
            &d,
            REDDIT_CHALLENGE_HTML,
            "https://www.reddit.com/r/programming/",
        )
        .expect("should solve");
        assert!(
            url.contains("solution=a5be06c2a2c9c99da5be06c2a2c9c99d"),
            "{url}"
        );
        assert!(url.contains("js_challenge=1"), "{url}");
        assert!(url.contains("token=deadbeef1234"), "{url}");
        assert!(
            url.starts_with("https://www.reddit.com/r/programming/"),
            "{url}"
        );
    }

    #[test]
    fn solve_returns_none_for_unsolvable() {
        let d = Detection {
            blocked: true,
            provider: "cloudflare_turnstile",
            confidence: 0.97,
            status: 200,
            matched: vec!["just a moment"],
            clearance_cookie: Some("cf_clearance"),
            reason: "test".into(),
            hint: HINT_ESCALATE,
        };
        assert!(solve_url(&d, "", "https://example.com/").is_none());
    }

    // HTML helpers ------------------------------------------------------------

    #[test]
    fn extract_attr_double_quoted() {
        let tag = r#"<form hidden method="GET" action="/r/foo/">"#;
        assert_eq!(extract_attr(tag, "action"), Some("/r/foo/".into()));
        assert_eq!(extract_attr(tag, "method"), Some("GET".into()));
    }

    #[test]
    fn extract_attr_missing() {
        assert!(extract_attr(r#"<form method="GET">"#, "action").is_none());
    }

    #[test]
    fn extract_hidden_input_present() {
        let body = r#"<form><input type="hidden" name="token" value="abc123"/></form>"#;
        assert_eq!(extract_hidden_input(body, "token"), Some("abc123".into()));
    }

    #[test]
    fn extract_hidden_input_missing() {
        assert!(extract_hidden_input(r#"<input name="x" value="y"/>"#, "z").is_none());
    }
}
