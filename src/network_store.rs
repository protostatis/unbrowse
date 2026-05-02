//! Network response capture for content extraction.
//!
//! Many SPAs (Zillow listings, npm package metadata, Reddit JSON, GraphQL-
//! backed apps) keep their content in API responses and only assemble it
//! into the DOM via hydration. For an LLM agent doing extraction, the
//! API response is often *cleaner* than the rendered DOM — less template
//! noise, structured shape, no need to wait for hydration to complete.
//!
//! This module captures every fetch/XHR response that looks content-bearing
//! (JSON / GraphQL / NDJSON / Next/Nuxt route data), ranks them by likely
//! content value, and surfaces them via the navigate result and the
//! `network_stores` RPC method.
//!
//! Storage is bounded: a sliding window of the most recent captures, capped
//! by both entry count and total bytes. Per-capture body is truncated.
//!
//! Hooks: `run_fetch` in main.rs calls `maybe_capture` after every fetch
//! completes (worker thread). The store is shared via Arc<Mutex<>> with the
//! Session.

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_ENTRIES: usize = 100;
const DEFAULT_MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024; // 4 MB
const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024; // 64 KB per capture

#[derive(Debug, Clone, Serialize)]
pub struct NetworkCapture {
    pub url: String,
    pub method: String,
    pub status: u16,
    pub content_type: String,
    pub body_bytes: usize,
    pub body_truncated: bool,
    pub body: String,
    pub captured_at_ms: u64,
    pub score: u32,
    pub kind: ContentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Json,
    GraphQl,
    Ndjson,
    JsonLd,
    NextRouteData,
    NuxtRouteData,
    /// Capture-eligible by URL/body shape but not a recognized format.
    JsonLikely,
}

pub struct NetworkStore {
    captures: VecDeque<NetworkCapture>,
    current_bytes: usize,
    pub max_entries: usize,
    pub max_total_bytes: usize,
    pub max_body_bytes: usize,
}

impl Default for NetworkStore {
    fn default() -> Self {
        Self {
            captures: VecDeque::new(),
            current_bytes: 0,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

impl NetworkStore {
    /// Inspect a completed fetch response and capture if content-bearing.
    /// Returns true if captured.
    pub fn maybe_capture(
        &mut self,
        url: &str,
        method: &str,
        status: u16,
        headers: &HashMap<String, String>,
        body: &str,
    ) -> bool {
        // Only successful responses; non-2xx is typically auth/redirect
        // noise, not content.
        if !(200..300).contains(&status) {
            return false;
        }
        if body.is_empty() {
            return false;
        }
        let content_type = headers
            .get("content-type")
            .cloned()
            .unwrap_or_default()
            .to_lowercase();

        let (score, kind) = classify(&content_type, url, body);
        if score == 0 {
            return false;
        }

        let body_bytes = body.len();
        let body_truncated = body_bytes > self.max_body_bytes;
        let stored_body = if body_truncated {
            // Truncate at a character boundary to keep the JSON parseable
            // when possible (best-effort — caller must check `body_truncated`).
            let mut end = self.max_body_bytes;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            body[..end].to_string()
        } else {
            body.to_string()
        };

        // Evict oldest until we fit.
        while self.captures.len() >= self.max_entries
            || self.current_bytes + stored_body.len() > self.max_total_bytes
        {
            if let Some(old) = self.captures.pop_front() {
                self.current_bytes = self.current_bytes.saturating_sub(old.body.len());
            } else {
                break;
            }
        }

        self.current_bytes += stored_body.len();
        self.captures.push_back(NetworkCapture {
            url: url.to_string(),
            method: method.to_string(),
            status,
            content_type,
            body_bytes,
            body_truncated,
            body: stored_body,
            captured_at_ms: now_ms(),
            score,
            kind,
        });
        true
    }

    /// Top N captures by score, optionally filtered by host substring.
    /// Returned in score-descending order. `body` field is preserved.
    pub fn ranked(&self, limit: usize, host_filter: Option<&str>) -> Vec<NetworkCapture> {
        let mut v: Vec<NetworkCapture> = self
            .captures
            .iter()
            .filter(|c| match host_filter {
                None => true,
                Some(h) => host_of(&c.url).contains(h),
            })
            .cloned()
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.score));
        v.truncate(limit);
        v
    }

    /// Quick summary for embedding in navigate result without dumping bodies.
    pub fn summary(&self, top_k: usize) -> NetworkStoreSummary {
        let mut tops: Vec<&NetworkCapture> = self.captures.iter().collect();
        tops.sort_by_key(|c| std::cmp::Reverse(c.score));
        tops.truncate(top_k);
        NetworkStoreSummary {
            count: self.captures.len(),
            total_bytes: self.current_bytes,
            top: tops
                .iter()
                .map(|c| NetworkCaptureMeta {
                    url: c.url.clone(),
                    status: c.status,
                    content_type: c.content_type.clone(),
                    body_bytes: c.body_bytes,
                    body_truncated: c.body_truncated,
                    score: c.score,
                    kind: c.kind,
                })
                .collect(),
        }
    }

    pub fn clear(&mut self) {
        self.captures.clear();
        self.current_bytes = 0;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.captures.len()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkStoreSummary {
    pub count: usize,
    pub total_bytes: usize,
    pub top: Vec<NetworkCaptureMeta>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkCaptureMeta {
    pub url: String,
    pub status: u16,
    pub content_type: String,
    pub body_bytes: usize,
    pub body_truncated: bool,
    pub score: u32,
    pub kind: ContentKind,
}

/// Heuristic ranking. Score 0 means "skip this response."
/// Combined Content-Type + URL pattern + body shape signals.
fn classify(content_type: &str, url: &str, body: &str) -> (u32, ContentKind) {
    let ct = content_type;

    // Hard skip: media, fonts, css, html, plain js. These never carry the
    // structured data we want to surface; capturing them just wastes the
    // store budget.
    if ct.starts_with("image/")
        || ct.starts_with("font/")
        || ct.starts_with("video/")
        || ct.starts_with("audio/")
    {
        return (0, ContentKind::JsonLikely);
    }
    if ct.contains("text/css")
        || ct.contains("text/html")
        || (ct.contains("javascript") && !ct.contains("json"))
    {
        return (0, ContentKind::JsonLikely);
    }

    let url_lower = url.to_lowercase();
    let mut score: u32 = 0;
    let mut kind = ContentKind::JsonLikely;

    // ---- Content-Type signals (strongest) ----
    if ct.contains("application/graphql") || ct.contains("graphql+json") {
        score += 40;
        kind = ContentKind::GraphQl;
    } else if ct.contains("application/ld+json") {
        score += 25;
        kind = ContentKind::JsonLd;
    } else if ct.contains("application/x-ndjson") || ct.contains("application/jsonl") {
        score += 25;
        kind = ContentKind::Ndjson;
    } else if ct.contains("application/json") || ct.contains("+json") {
        score += 30;
        kind = ContentKind::Json;
    }

    // ---- URL pattern signals ----
    if url_lower.contains("/graphql") || url_lower.contains("/gql") {
        score += 25;
        if matches!(kind, ContentKind::JsonLikely | ContentKind::Json) {
            kind = ContentKind::GraphQl;
        }
    }
    if url_lower.contains("/_next/data/") || url_lower.contains("__nextjs__") {
        score += 30;
        kind = ContentKind::NextRouteData;
    }
    if url_lower.contains("/__nuxt") || url_lower.contains("/nuxt/") {
        score += 25;
        kind = ContentKind::NuxtRouteData;
    }
    if url_lower.contains("/api/")
        || url_lower.contains("/v1/")
        || url_lower.contains("/v2/")
        || url_lower.contains("/v3/")
    {
        score += 15;
    }

    // ---- Body shape signals (always cheap to try) ----
    let trimmed = body.trim_start();
    let looks_jsonish = trimmed.starts_with('{') || trimmed.starts_with('[');
    if looks_jsonish {
        // Cheap parse check on first 4 KB — if it parses, it's structured
        // data. Don't parse the whole thing (could be MB).
        let probe = if body.len() > 4096 {
            // Find a char boundary near 4096 for the slice
            let mut end = 4096;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            &body[..end]
        } else {
            body
        };
        // Probe parses to a `Value` if it's complete enough. For truncated
        // probes we can't fully parse, but a starts-with-`{` body that
        // *would* parse if complete is still strong evidence.
        if serde_json::from_str::<serde_json::Value>(probe).is_ok() {
            score += 15;
        } else if body.len() > 4096 && serde_json::from_str::<serde_json::Value>(body).is_ok() {
            // Whole-body parse for medium bodies (parser stops at end of value;
            // bounded by O(body_size)).
            score += 15;
        } else {
            // Looks JSONy but didn't parse — still a weak positive signal,
            // small bonus for the open brace/bracket.
            score += 5;
        }

        // Size bonus — bigger structured payloads are more likely to be
        // the real data.
        if body.len() > 2_000 {
            score += 5;
        }
        if body.len() > 20_000 {
            score += 5;
        }
    }

    // Threshold — must look at least somewhat content-bearing to capture.
    if score < 25 {
        return (0, kind);
    }
    (score, kind)
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_lowercase()))
        .unwrap_or_default()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(ct: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("content-type".to_string(), ct.to_string());
        m
    }

    #[test]
    fn captures_application_json() {
        let mut s = NetworkStore::default();
        let body = r#"{"data": {"items": [1,2,3], "total": 42}}"#;
        assert!(s.maybe_capture(
            "https://api.example.com/v1/items",
            "GET",
            200,
            &h("application/json"),
            body
        ));
        assert_eq!(s.len(), 1);
        let r = &s.ranked(10, None)[0];
        assert_eq!(r.kind, ContentKind::Json);
        assert!(r.score >= 30);
    }

    #[test]
    fn captures_graphql() {
        let mut s = NetworkStore::default();
        let body = r#"{"data":{"viewer":{"id":"x"}}}"#;
        assert!(s.maybe_capture(
            "https://api.example.com/graphql",
            "POST",
            200,
            &h("application/graphql+json"),
            body
        ));
        let r = &s.ranked(10, None)[0];
        assert_eq!(r.kind, ContentKind::GraphQl);
    }

    #[test]
    fn captures_next_route_data() {
        let mut s = NetworkStore::default();
        let body = r#"{"pageProps":{"data":[1,2,3]}}"#;
        assert!(s.maybe_capture(
            "https://example.com/_next/data/abc/page.json",
            "GET",
            200,
            &h("application/json"),
            body
        ));
        let r = &s.ranked(10, None)[0];
        assert_eq!(r.kind, ContentKind::NextRouteData);
    }

    #[test]
    fn skips_html() {
        let mut s = NetworkStore::default();
        assert!(!s.maybe_capture(
            "https://example.com/",
            "GET",
            200,
            &h("text/html"),
            "<html>x</html>"
        ));
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn skips_image_css_js() {
        let mut s = NetworkStore::default();
        assert!(!s.maybe_capture(
            "https://example.com/x.png",
            "GET",
            200,
            &h("image/png"),
            "binary"
        ));
        assert!(!s.maybe_capture(
            "https://example.com/x.css",
            "GET",
            200,
            &h("text/css"),
            "body{}"
        ));
        assert!(!s.maybe_capture(
            "https://example.com/x.js",
            "GET",
            200,
            &h("application/javascript"),
            "var x=1"
        ));
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn skips_non_2xx() {
        let mut s = NetworkStore::default();
        assert!(!s.maybe_capture(
            "https://api.example.com/v1/x",
            "GET",
            401,
            &h("application/json"),
            r#"{"error":"unauth"}"#
        ));
        assert!(!s.maybe_capture(
            "https://api.example.com/v1/x",
            "GET",
            500,
            &h("application/json"),
            r#"{"error":"oops"}"#
        ));
        assert!(!s.maybe_capture(
            "https://api.example.com/v1/x",
            "GET",
            302,
            &h("application/json"),
            r#"{}"#
        ));
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn skips_empty_body() {
        let mut s = NetworkStore::default();
        assert!(!s.maybe_capture(
            "https://api.example.com/v1/x",
            "GET",
            200,
            &h("application/json"),
            ""
        ));
    }

    #[test]
    fn truncates_large_body() {
        let mut s = NetworkStore::default();
        let big = "{".to_string() + &"\"k\":\"v\",".repeat(20_000) + "\"end\":1}";
        assert!(s.maybe_capture(
            "https://api.example.com/v1/x",
            "GET",
            200,
            &h("application/json"),
            &big
        ));
        let r = &s.ranked(10, None)[0];
        assert!(r.body_truncated);
        assert_eq!(r.body_bytes, big.len());
        assert!(r.body.len() <= s.max_body_bytes);
    }

    #[test]
    fn ranks_by_score() {
        let mut s = NetworkStore::default();
        // Plain JSON, no path bonus
        s.maybe_capture(
            "https://example.com/data.json",
            "GET",
            200,
            &h("application/json"),
            r#"{"a":1}"#,
        );
        // GraphQL — should outscore plain JSON
        s.maybe_capture(
            "https://example.com/graphql",
            "POST",
            200,
            &h("application/graphql+json"),
            r#"{"data":{"x":[1,2,3]}}"#,
        );
        let r = s.ranked(10, None);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].kind, ContentKind::GraphQl);
    }

    #[test]
    fn evicts_when_over_capacity() {
        let mut s = NetworkStore {
            max_entries: 3,
            ..NetworkStore::default()
        };
        for i in 0..5 {
            let url = format!("https://api.example.com/v1/item/{i}");
            s.maybe_capture(
                &url,
                "GET",
                200,
                &h("application/json"),
                &format!(r#"{{"id":{i}}}"#),
            );
        }
        assert_eq!(s.len(), 3);
        // Oldest two should be evicted
        let urls: Vec<_> = s.ranked(10, None).into_iter().map(|c| c.url).collect();
        assert!(urls.iter().any(|u| u.ends_with("/2")));
        assert!(urls.iter().any(|u| u.ends_with("/4")));
        assert!(!urls.iter().any(|u| u.ends_with("/0")));
    }

    #[test]
    fn host_filter_works() {
        let mut s = NetworkStore::default();
        s.maybe_capture(
            "https://api.first.com/v1/x",
            "GET",
            200,
            &h("application/json"),
            r#"{"a":1}"#,
        );
        s.maybe_capture(
            "https://api.second.com/v1/x",
            "GET",
            200,
            &h("application/json"),
            r#"{"b":2}"#,
        );
        let r = s.ranked(10, Some("first"));
        assert_eq!(r.len(), 1);
        assert!(r[0].url.contains("first.com"));
    }
}
