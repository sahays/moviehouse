//! Security response headers.
//!
//! These were set by the nginx vhost (`security_headers.inc` plus a tuned CSP)
//! back when `MovieHouse` ran behind a reverse proxy. The local-only build serves
//! axum directly to the LAN, so anything the proxy used to add has to come from
//! the app or it is simply absent — which is what security-audit finding **L4**
//! (no CSP / security headers) describes.
//!
//! Deliberately **no HSTS**: the LAN listener is plain HTTP, and an HSTS header
//! would pin the browser to `https://` for that host and port, locking the user
//! out of their own server.

use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

/// Content-Security-Policy for the embedded Vite/React SPA. Carried over from
/// the vhost this replaces; each relaxation is load-bearing:
///
/// * `img-src ... https:` — TMDB posters are fetched from `image.tmdb.org`.
/// * `style-src 'unsafe-inline'` — Tailwind emits inline style attributes.
/// * `media-src 'self' blob:` — HLS playback hands the video element blob URLs.
/// * `connect-src ... ws:` — the progress WebSocket is same-origin but plain
///   `ws:` on a LAN, not `wss:`.
const CSP: &str = "default-src 'self'; img-src 'self' data: https:; media-src 'self' blob:; \
                   style-src 'self' 'unsafe-inline'; script-src 'self'; \
                   connect-src 'self' ws: wss:; font-src 'self'; frame-ancestors 'none'; \
                   base-uri 'self'";

/// Attach the security headers to every response.
pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}
