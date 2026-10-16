//! Embedded HTTP proxy for the browser pane.
//!
//! The pane renders an `<iframe>` pointed at `http://127.0.0.1:<port>/proxy?url=<target>`.
//! This module:
//!   1. fetches the target URL with `reqwest` (any HTTP method, forwarded body)
//!   2. removes the headers that prevent framing (`X-Frame-Options`, CSP
//!      `frame-ancestors`, ...) and adds permissive CORS headers
//!   3. rewrites every sub-resource URL in the returned HTML/CSS through the
//!      proxy, so fonts, stylesheets, images and scripts all load same-origin
//!      (no CORS errors — the original bug)
//!   4. injects a `<base href>` (real target origin) so relative `<a>` links
//!      resolve correctly for the bridge, plus the bridge script itself
//!
//! Security note: the entire CSP header is stripped from responses so the
//! injected inline bridge can run on sites with strict `script-src`. This is an
//! acceptable tradeoff for a local dev-tool webview — the same page is one tab
//! away in a normal browser with the site's full protections.
//!
//! Known limitations: sessions don't survive across proxied requests (cookies
//! land on the 127.0.0.1:PORT origin and the proxy's `reqwest` client has no
//! cookie jar), and `EventSource`/WebSockets are not intercepted by the bridge.
//! Hash-based routing (`/#/route`) doesn't work — the fragment is encoded
//! inside the `url=` param, so `location.hash` is empty. And because the bridge
//! (with its fetch/XHR proxy hooks) now runs in the `<head>`, a page that
//! later *reassigns* `window.fetch` could bypass the proxy (rare — most
//! frameworks wrap the existing reference).
//!
//! `html_unescape` runs on the incoming `url` param, so a URL whose query
//! legitimately contains a percent-encoded `&amp;` sequence (e.g. `?q=1%26amp%3B2`)
//! would be decoded to `&` — such URLs are rare and usually botched escapes
//! anyway, so this is accepted.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::Query;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::Response;
use axum::Extension;
use regex::Regex;
use serde::Deserialize;
use url::Url;

const BRIDGE_JS: &str = include_str!("bridge.js");

/// Headers removed from proxied responses — everything that blocks embedding,
/// would leak our injected base/bridge, or describes the upstream transfer
/// (we decompress and re-encode, so those would corrupt the response).
const STRIPPED_HEADERS: [&str; 7] = [
    "x-frame-options",
    "content-security-policy",
    "content-security-policy-report-only",
    "cross-origin-opener-policy",
    "content-encoding",
    "content-length",
    "transfer-encoding",
];

#[derive(Deserialize)]
pub struct ProxyParams {
    /// The absolute target URL to fetch.
    url: String,
    /// Optional scroll position (pixels) to restore after load.
    scroll: Option<u32>,
}

#[derive(Clone)]
pub struct ProxyClient {
    client: reqwest::Client,
    port: std::sync::Arc<std::sync::atomic::AtomicU16>,
}

impl ProxyClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(10))
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            port: std::sync::Arc::new(std::sync::atomic::AtomicU16::new(0)),
        }
    }

    /// The localhost port the proxy listener bound at startup (0 until bound).
    pub fn port(&self) -> u16 {
        self.port.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Build the axum router serving the proxy. The listener must bind on
/// `127.0.0.1:0` (ephemeral port) and record the port via [`ProxyClient::set_port`]
/// so the client can discover it through the `get_proxy_info` query.
pub fn router(client: Arc<ProxyClient>) -> axum::Router {
    axum::Router::new()
        .route("/proxy", axum::routing::any(proxy_handler))
        // Catch-all for requests to the proxy origin that aren't `/proxy`:
        // JS-injected elements resolve against `location.origin` (e.g. Next.js
        // `<link rel="preload">` fonts), bypassing both HTML rewriting and the
        // bridge's fetch/XHR wrapper. axum prefers the static `/proxy` route.
        .route("/{*path}", axum::routing::any(proxy_any_path))
        .layer(Extension(client))
}

/// Extract the target URL from a `/proxy?url=<encoded>` reference (the
/// Referer of any sub-resource loaded by the proxied page). Returns `None` for
/// anything that isn't a proxy URL.
fn proxy_target_from_referer(referer: &str) -> Option<String> {
    let url = Url::parse(referer).ok()?;
    if url.path() != "/proxy" {
        return None;
    }
    url.query_pairs()
        .find(|(k, _)| k == "url")
        .map(|(_, v)| v.into_owned())
}

/// Serve any non-`/proxy` path on the proxy origin by resolving it against the
/// current page's target (from the Referer's `url` param) and delegating to
/// the main proxy handler.
async fn proxy_any_path(
    uri: Uri,
    method: Method,
    headers: HeaderMap,
    Extension(client): Extension<Arc<ProxyClient>>,
) -> Response {
    let target = headers
        .get("referer")
        .and_then(|v| v.to_str().ok())
        .and_then(proxy_target_from_referer)
        .and_then(|t| Url::parse(&t).ok())
        .map(|u| u.origin().ascii_serialization())
        .filter(|o| !o.is_empty());

    let Some(origin) = target else {
        return error_response(
            StatusCode::BAD_GATEWAY,
            "cannot resolve proxied sub-resource (no target from Referer)",
        );
    };

    let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let full = format!("{origin}{path}");
    proxy_handler(
        method,
        Query(ProxyParams { url: full, scroll: None }),
        headers,
        Extension(client),
        Bytes::new(),
    )
    .await
}

/// Serve a proxied request — any method (GET/POST/PUT/DELETE/...), body
/// forwarded so the page's fetch/XHR calls work through the proxy.
pub async fn proxy_handler(
    method: Method,
    Query(params): Query<ProxyParams>,
    headers: HeaderMap,
    Extension(client): Extension<Arc<ProxyClient>>,
    body: Bytes,
) -> Response {
    // Normalize the target: strip HTML entities pages leave in URLs (e.g.
    // `&amp;` in attribute values) and require an absolute http(s) URL — the
    // 400 the user saw came from a literal `&amp;` reaching Next.js's image
    // optimizer.
    let target = html_unescape(params.url.trim());
    let parsed = match Url::parse(&target) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => u,
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid url"),
    };

    let mut req = client.client.request(method.clone(), &target);
    // Forward a safe subset of request headers (no cookies — the proxy is a
    // different origin and has no session for the target site anyway).
    for name in ["content-type", "accept", "user-agent", "accept-language"] {
        if let Some(v) = headers.get(name) {
            req = req.header(name, v.clone());
        }
    }
    // Present the request as same-site: servers that validate Origin/Referer
    // (e.g. the Next.js dev server) reject cross-origin sub-resource loads
    // with 403. The page's own origin is the proxy, so rewrite these to the
    // target origin instead of forwarding the page's.
    let origin = parsed.origin().ascii_serialization();
    if !origin.is_empty() {
        req = req.header("origin", origin);
    }
    req = req.header("referer", parsed.as_str());
    if !matches!(method, Method::GET | Method::HEAD) && !body.is_empty() {
        req = req.body(body);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("Failed to fetch {target}: {e}"),
            );
        }
    };

    let status = resp.status();
    let final_url = resp.url().clone();
    let mut out_headers: HeaderMap = resp.headers().clone();
    for name in STRIPPED_HEADERS {
        out_headers.remove(name);
    }
    out_headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    out_headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"),
    );
    out_headers.insert("access-control-allow-headers", HeaderValue::from_static("*"));

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ctype_lower = content_type.to_lowercase();

    let proxy_prefix = format!("http://127.0.0.1:{}/proxy?url=", client.port());

    if ctype_lower.contains("html") {
        let body = match resp.text().await {
            Ok(t) => t,
            Err(e) => return error_response(StatusCode::BAD_GATEWAY, &format!("read body: {e}")),
        };
        let rewritten = rewrite_html(&body, final_url.as_str(), &proxy_prefix, params.scroll.unwrap_or(0));
        out_headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        build_response(status, out_headers, rewritten)
    } else if ctype_lower.contains("text/css") || ctype_lower.contains("stylesheet") {
        let body = match resp.text().await {
            Ok(t) => t,
            Err(e) => return error_response(StatusCode::BAD_GATEWAY, &format!("read body: {e}")),
        };
        let rewritten = rewrite_css(&body, final_url.as_str(), &proxy_prefix);
        build_response(status, out_headers, rewritten)
    } else {
        let body = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return error_response(StatusCode::BAD_GATEWAY, &format!("read body: {e}")),
        };
        build_response(status, out_headers, body)
    }
}

fn build_response(status: StatusCode, headers: HeaderMap, body: impl Into<axum::body::Body>) -> Response {
    let mut builder = Response::builder().status(status);
    if let Some(hm) = builder.headers_mut() {
        *hm = headers;
    }
    builder
        .body(body.into())
        .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "build"))
}

// ─── URL rewriting ───────────────────────────────────────────────────────────

/// Decode the HTML character references that commonly appear in attribute
/// values (e.g. `&amp;` in a query string). Without this, `&amp;` is encoded as
/// `%26amp%3B` and the upstream server (Next.js image optimizer, …) rejects it.
fn html_unescape(v: &str) -> String {
    v.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#34;", "\"")
}

/// Build the proxy URL that fetches `abs` on behalf of the page.
fn proxy_url(prefix: &str, abs: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(abs.as_bytes()).collect();
    format!("{prefix}{encoded}")
}

/// Whether a URL should be routed through the proxy at all.
fn should_rewrite(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    for skip in [
        "data:", "javascript:", "mailto:", "tel:", "blob:", "about:", "ws:", "wss:",
    ] {
        if lower.starts_with(skip) {
            return false;
        }
    }
    if lower.starts_with('#') {
        return false;
    }
    // Already-proxied URLs (the page itself may reference our proxy origin).
    if t.contains("/proxy?url=") {
        return false;
    }
    true
}

/// Resolve an attribute/`url()` value against `base_url` and proxy it.
fn rewrite_url_value(raw: &str, base_url: &str, prefix: &str) -> String {
    let t = html_unescape(raw.trim());
    if !should_rewrite(&t) {
        return raw.to_string();
    }
    let abs = match Url::parse(&t) {
        Ok(u) => u.to_string(),
        Err(_) => match Url::parse(base_url) {
            Ok(base) => base
                .join(&t)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| t.clone()),
            Err(_) => t.clone(),
        },
    };
    proxy_url(prefix, &abs)
}

/// Rewrite `url(...)` and `@import` inside CSS text.
fn rewrite_css(css: &str, base_url: &str, prefix: &str) -> String {
    let url_re = Regex::new(r#"(?i)url\(\s*['"]?([^'")]+)['"]?\s*\)"#).unwrap();
    let step1 = url_re.replace_all(css, |caps: &regex::Captures| {
        let inner = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let newv = rewrite_url_value(inner, base_url, prefix);
        format!("url(\"{newv}\")")
    });
    let imp_re = Regex::new(r#"(?i)@import\s+(?:url\(\s*['"]?([^'")]+)['"]?\s*\)|['"]([^'"]+)['"])"#).unwrap();
    let step2 = imp_re.replace_all(&step1, |caps: &regex::Captures| {
        let inner = caps
            .get(1)
            .map(|m| m.as_str())
            .or_else(|| caps.get(2).map(|m| m.as_str()))
            .unwrap_or("");
        let newv = rewrite_url_value(inner, base_url, prefix);
        format!("@import url(\"{newv}\")")
    });
    step2.into_owned()
}

/// Rewrite `srcset` candidates ("url 1x, url 2x"). The HTML spec splits
/// srcset candidates on commas (URLs containing raw commas must be encoded), so
/// a naive comma split is spec-correct; only data: URIs — which always contain
/// commas — need a bail-out.
fn rewrite_srcset(value: &str, base_url: &str, prefix: &str) -> String {
    // data: URIs contain commas — bail out rather than mangle them.
    if value.to_ascii_lowercase().contains("data:") {
        return value.to_string();
    }
    value
        .split(',')
        .map(|part| {
            let p = part.trim();
            let mut it = p.splitn(2, char::is_whitespace);
            let url = it.next().unwrap_or("");
            let desc = it.next().unwrap_or("");
            let newu = rewrite_url_value(url, base_url, prefix);
            if desc.is_empty() {
                newu
            } else {
                format!("{newu} {desc}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Rewrite the `url=` inside a `<meta http-equiv="refresh">` content value.
fn rewrite_meta_refresh(value: &str, base_url: &str, prefix: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if let Some(idx) = lower.find("url=") {
        let url_part = &value[idx + 4..];
        let newu = rewrite_url_value(url_part.trim(), base_url, prefix);
        format!("{}url={newu}", &value[..idx])
    } else {
        value.to_string()
    }
}

/// Decide whether an attribute on `tag` should be proxied.
fn should_rewrite_attr(tag: &str, name: &str, attrs_lower: &str) -> bool {
    match (tag, name) {
        ("script" | "img" | "iframe" | "video" | "audio" | "source" | "track"
        | "embed" | "object" | "input", "src") => true,
        ("img" | "source", "srcset") => true,
        ("link" | "script" | "iframe" | "video" | "audio" | "embed" | "object", "href") => true,
        ("video", "poster") => true,
        (_, "data-src") | (_, "data-original") | (_, "data-lazy-src") => true,
        (_, "data-lazy-srcset") => true,
        (_, "style") => true,
        ("meta", "content") if attrs_lower.contains("refresh") => true,
        _ => false,
    }
}

/// Rewrite attributes on a single tag. Keeps `<a>`/`<form>` href/action as real
/// URLs so the bridge's click/submit interception can navigate them. Handles
/// double- and single-quoted attributes with separate passes (no backrefs).
fn rewrite_attrs(tag: &str, attrs: &str, base_url: &str, prefix: &str) -> String {
    let dbl_re = Regex::new(r#"(?is)([a-zA-Z-]+)(\s*=\s*)"([^"]*)""#).unwrap();
    let sgl_re = Regex::new(r#"(?is)([a-zA-Z-]+)(\s*=\s*)'([^']*)'"#).unwrap();
    let lower_attrs = attrs.to_ascii_lowercase();

    // Rewrite double-quoted attributes first, then single-quoted on the result
    // (the rewritten values are percent-encoded, so they never contain quotes).
    let step1 = dbl_re.replace_all(attrs, |caps: &regex::Captures| {
        rewrite_attr_match(caps, tag, &lower_attrs, base_url, prefix, "\"")
    });
    sgl_re
        .replace_all(&step1, |caps: &regex::Captures| {
            rewrite_attr_match(caps, tag, &lower_attrs, base_url, prefix, "'")
        })
        .into_owned()
}

/// Rewrite one captured `name=value` attribute pair (group 1/2/3) when the tag
/// + attribute combo should be proxied; otherwise echo it back unchanged.
fn rewrite_attr_match(
    caps: &regex::Captures<'_>,
    tag: &str,
    lower_attrs: &str,
    base_url: &str,
    prefix: &str,
    quote: &str,
) -> String {
    let name = caps[1].to_ascii_lowercase();
    let eq = &caps[2];
    let value = caps.get(3).map(|m| m.as_str()).unwrap_or("");

    if !should_rewrite_attr(tag, &name, lower_attrs) {
        return format!("{}{}{}{}{}", &caps[1], eq, quote, value, quote);
    }

    let newv = match name.as_str() {
        "style" => rewrite_css(value, base_url, prefix),
        "srcset" | "data-lazy-srcset" => rewrite_srcset(value, base_url, prefix),
        "content" => rewrite_meta_refresh(value, base_url, prefix),
        _ => rewrite_url_value(value, base_url, prefix),
    };
    format!("{}{}{}{}{}", &caps[1], eq, quote, newv, quote)
}

/// Rewrite every sub-resource URL in a fetched HTML document through the proxy.
fn rewrite_html(html: &str, base_url: &str, prefix: &str, scroll_y: u32) -> String {
    // 1. Rewrite `<style>` block contents.
    let style_re = Regex::new(r#"(?is)(<style\b[^>]*>)(.*?)(</style>)"#).unwrap();
    let step1 = style_re.replace_all(html, |caps: &regex::Captures| {
        let inner = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        format!("{}{}{}", &caps[1], rewrite_css(inner, base_url, prefix), &caps[3])
    });

    // 2. Walk tags and rewrite their attributes.
    let tag_re = Regex::new(r#"(?is)<([a-z][a-z0-9-]*)([^>]*)>"#).unwrap();
    let step2 = tag_re.replace_all(&step1, |caps: &regex::Captures| {
        let tag = caps[1].to_ascii_lowercase();
        let attrs = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let new_attrs = rewrite_attrs(&tag, attrs, base_url, prefix);
        format!("<{}{}>", &caps[1], new_attrs)
    });

    // 3. Inject `<base href>` (real target origin) + the bridge script, both
    //    right after <head>. The bridge must run BEFORE the page's own scripts
    //    so its history/pushState shim is installed before SPA routers
    //    (Next.js hydration) call history APIs with cross-origin URLs — those
    //    calls throw SecurityError if unshimmed.
    let base_tag = format!(r#"<base href="{base_url}">"#);
    let bridge = format!(
        r#"<script>window.__zemTarget={target};window.__zemScrollY={scroll};</script><script>{bridge}</script>"#,
        target = serde_json::to_string(base_url).unwrap_or_default(),
        scroll = scroll_y,
        bridge = BRIDGE_JS,
    );

    let mut out = String::with_capacity(step2.len() + base_tag.len() + bridge.len());
    let head_end = find_after_head(&step2);
    out.push_str(&step2[..head_end]);
    out.push_str(&base_tag);
    out.push_str(&bridge);
    out.push_str(&step2[head_end..]);
    out
}

/// Index just past `<head...>`, or 0 when no head tag exists yet.
fn find_after_head(html: &str) -> usize {
    let lower = html.to_ascii_lowercase();
    if let Some(start) = lower.find("<head") {
        if let Some(gt) = lower[start..].find('>') {
            return start + gt + 1;
        }
    }
    0
}

/// Minimal interaction→focus signal for pages the full bridge isn't injected
/// into (the proxy's own error page). Mirrors the bridge's behavior so the
/// host can still focus the pane when the user clicks the error page.
const FOCUS_SIGNAL_JS: &str = "['pointerdown','mousedown','keydown','touchstart'].forEach(function(evt){window.addEventListener(evt,function(){try{window.parent.postMessage({__zem:true,type:'focus'},'*')}catch(e){}},{capture:true})});";

fn error_response(status: StatusCode, message: &str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Zem proxy error</title>\
         <body style='font-family:system-ui;background:#111;color:#eee;padding:2rem'>\
         <h2>Zem browser pane</h2><p>{}</p>\
         <p style='opacity:.6;font-size:.85rem'>Check the URL and try again.</p>\
         <script>{}</script></body>",
        message, FOCUS_SIGNAL_JS
    );
    // Must be explicit `text/html` — a bare `(status, String)` response defaults
    // to `text/plain`, which renders the error as raw source inside the iframe.
    Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| Response::builder().status(status).body(axum::body::Body::empty()).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "http://127.0.0.1:9999/proxy?url=";

    #[test]
    fn rewrites_html_asset_urls_through_proxy() {
        let html = r#"
<!doctype html><html><head>
<link rel="stylesheet" href="/static/app.css">
<script src="https://cdn.example.com/lib.js"></script>
</head><body>
<img src="logo.png" srcset="a.png 1x, https://cdn.example.com/b.png 2x">
<a href="/page">link</a>
<style>.x { background: url('../img/bg.png'); }</style>
<form action="/submit" method="post"></form>
</body></html>
"#;
        let out = rewrite_html(html, "https://duckduckgo.com/", PREFIX, 0);

        // Stylesheet + script rewritten through the proxy.
        assert!(out.contains(&format!("href=\"{}https%3A%2F%2Fduckduckgo.com%2Fstatic%2Fapp.css\"", PREFIX)), "{}:", out);
        assert!(out.contains(&format!("src=\"{}https%3A%2F%2Fcdn.example.com%2Flib.js\"", PREFIX)), "{}:", out);

        // Relative image + srcset candidates rewritten.
        assert!(out.contains(&format!("src=\"{}https%3A%2F%2Fduckduckgo.com%2Flogo.png\"", PREFIX)), "{}:", out);
        assert!(out.contains(&format!("a.png 1x, {}https%3A%2F%2Fcdn.example.com%2Fb.png 2x", PREFIX)), "{}:", out);

        // <a href> stays a real URL so the bridge can intercept navigation.
        assert!(out.contains("<a href=\"/page\">"), "{}:", out);

        // Inline <style> url() rewritten, CSS relative to the document.
        assert!(out.contains(&format!("url(\"{}https%3A%2F%2Fduckduckgo.com%2Fimg%2Fbg.png\")", PREFIX)), "{}:", out);

        // Form action untouched.
        assert!(out.contains("<form action=\"/submit\""), "{}:", out);

        // Base + bridge injected.
        assert!(out.contains(r#"<base href="https://duckduckgo.com/">"#));
        assert!(out.contains("window.__zemTarget"));
    }

    #[test]
    fn rewrites_css_urls_relative_to_css_location() {
        let css = r#"
@import "theme.css";
@font-face {
  font-family: 'Duck';
  src: url('../fonts/Duck.woff2') format('woff2');
}
.icon { background: url(https://cdn.example.com/i.png); }
"#;
        let out = rewrite_css(css, "https://duckduckgo.com/static/app.css", PREFIX);

        assert!(out.contains(&format!("@import url(\"{}https%3A%2F%2Fduckduckgo.com%2Fstatic%2Ftheme.css\")", PREFIX)), "{}:", out);
        assert!(out.contains(&format!("url(\"{}https%3A%2F%2Fduckduckgo.com%2Ffonts%2FDuck.woff2\")", PREFIX)), "{}:", out);
        assert!(out.contains(&format!("url(\"{}https%3A%2F%2Fcdn.example.com%2Fi.png\")", PREFIX)), "{}:", out);
    }

    #[test]
    fn skips_data_and_same_origin_and_already_proxied() {
        assert_eq!(rewrite_url_value("data:image/png;base64,AAAA", "https://x.com/", PREFIX), "data:image/png;base64,AAAA");
        assert_eq!(rewrite_url_value("#frag", "https://x.com/", PREFIX), "#frag");
        assert!(rewrite_url_value("https://x.com/a.png", "https://x.com/", PREFIX).starts_with(PREFIX));

        let already = format!("{PREFIX}https%3A%2F%2Fx.com%2Fa.png");
        assert_eq!(rewrite_url_value(&already, "https://x.com/", PREFIX), already);
    }

    #[test]
    fn decodes_html_entities_in_attribute_urls() {
        // `&amp;` must become `&` before encoding — otherwise the upstream
        // server (Next.js image optimizer) gets a literal `&amp;` and 400s.
        let html = r#"<img src="/_next/image?url=%2Fhero.jpeg&amp;w=3840&amp;q=75">"#;
        let out = rewrite_html(html, "http://localhost:3000/", PREFIX, 0);
        assert!(
            out.contains(&format!("{PREFIX}http%3A%2F%2Flocalhost%3A3000%2F_next%2Fimage%3Furl%3D%252Fhero.jpeg%26w%3D3840%26q%3D75")),
            "{}:", out
        );
    }

    #[test]
    fn extracts_target_from_proxy_referer() {
        let referer = format!("http://127.0.0.1:55173/proxy?url={}", url::form_urlencoded::byte_serialize("http://localhost:3000/docs/page".as_bytes()).collect::<String>());
        let target = proxy_target_from_referer(&referer).unwrap();
        assert_eq!(target, "http://localhost:3000/docs/page");
        assert!(proxy_target_from_referer("http://localhost:3000/other").is_none());
        assert!(proxy_target_from_referer("").is_none());
    }

    #[test]
    fn keeps_data_srcsets_untouched() {
        let srcset = "data:image/png;base64,AAA 1x, data:image/png;base64,BBB 2x";
        assert_eq!(rewrite_srcset(srcset, "https://x.com/", PREFIX), srcset);
    }
}
