//! Access-code auth: stateless HMAC-signed session cookie.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Session cookie name.
pub const COOKIE_NAME: &str = "mh_session";
/// Session lifetime: 30 days (chosen for convenience on a personal app).
/// Tradeoff: the cookie is stateless, so a captured token stays valid for this
/// full window and cannot be revoked individually — rotating
/// `MOVIEHOUSE_ACCESS_CODE` invalidates every session at once (the HMAC key
/// changes). `HttpOnly` + HTTPS make capture hard; shorten this if you need a
/// tighter replay window.
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Current unix time in seconds.
pub fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Wall-clock seconds since epoch won't approach i64::MAX for billions of
    // years, so this cast never actually wraps.
    #[allow(clippy::cast_possible_wrap)]
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// HMAC-SHA256(key = `access_code`, msg) → lowercase hex.
/// `new_from_slice` only errors for key-length limits HMAC does not impose, so
/// the error branch is unreachable; return `""` there to stay `unwrap`/`expect`
/// free (the repo denies `unwrap_used`/`expect_used`). An empty signature never
/// verifies, so this is fail-closed.
fn sign(access_code: &str, msg: &str) -> String {
    let Ok(mut mac) = HmacSha256::new_from_slice(access_code.as_bytes()) else {
        return String::new();
    };
    mac.update(msg.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Mint a token `"{exp}.{hex(hmac)}"` where exp = now + TTL.
pub fn mint_token(access_code: &str, now_unix: i64) -> String {
    let exp = now_unix + SESSION_TTL_SECS;
    let exp_str = exp.to_string();
    let sig = sign(access_code, &exp_str);
    format!("{exp_str}.{sig}")
}

/// Verify signature (constant-time) and that the token has not expired.
pub fn verify_token(token: &str, access_code: &str, now_unix: i64) -> bool {
    let Some((exp_str, sig)) = token.split_once('.') else {
        return false;
    };
    if exp_str.is_empty() || sig.is_empty() {
        return false;
    }
    let expected = sign(access_code, exp_str);
    // Constant-time compare of hex signatures.
    if expected.as_bytes().ct_eq(sig.as_bytes()).unwrap_u8() != 1 {
        return false;
    }
    match exp_str.parse::<i64>() {
        Ok(exp) => exp > now_unix,
        Err(_) => false,
    }
}

/// Playback-token lifetime: 12 hours. Long enough to start a film in the evening,
/// pause it, and finish it — short enough that a leaked URL stops working the
/// same day.
pub const PLAYBACK_TTL_SECS: i64 = 12 * 60 * 60;

/// Mint a playback token for one media entry: `"{exp}.{hex(hmac)}"`.
///
/// AirPlay/Chromecast receivers fetch the media URL themselves, as separate HTTP
/// clients with no access to the browser's cookie jar — an Apple TV asking for
/// `/stream` sends no `mh_session` and gets a 401, which it renders as a blocked
/// badge instead of a play button. A capability in the query string is the only
/// credential that survives the handoff.
///
/// The signed message is `"playback:{media_id}:{exp}"`, so a token is useless for
/// any other entry and — because a session token signs the bare `exp` — the two
/// token types can never be replayed as each other.
pub fn mint_playback_token(access_code: &str, media_id: &str, now_unix: i64) -> String {
    let exp = now_unix + PLAYBACK_TTL_SECS;
    let sig = sign(access_code, &playback_msg(media_id, exp));
    format!("{exp}.{sig}")
}

/// Verify a playback token against the media entry it is scoped to.
pub fn verify_playback_token(
    token: &str,
    access_code: &str,
    media_id: &str,
    now_unix: i64,
) -> bool {
    let Some((exp_str, sig)) = token.split_once('.') else {
        return false;
    };
    if exp_str.is_empty() || sig.is_empty() {
        return false;
    }
    let Ok(exp) = exp_str.parse::<i64>() else {
        return false;
    };
    let expected = sign(access_code, &playback_msg(media_id, exp));
    if expected.as_bytes().ct_eq(sig.as_bytes()).unwrap_u8() != 1 {
        return false;
    }
    exp > now_unix
}

fn playback_msg(media_id: &str, exp: i64) -> String {
    format!("playback:{media_id}:{exp}")
}

/// Constant-time comparison of a submitted code against the configured code.
pub fn code_matches(submitted: &str, configured: &str) -> bool {
    submitted
        .as_bytes()
        .ct_eq(configured.as_bytes())
        .unwrap_u8()
        == 1
}

/// `Set-Cookie` value for a fresh session. `secure` adds the Secure attribute
/// (only when the request came over HTTPS, so LAN http still works).
pub fn session_cookie_header(token: &str, secure: bool) -> String {
    let mut h = format!(
        "{COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_TTL_SECS}"
    );
    if secure {
        h.push_str("; Secure");
    }
    h
}

/// `Set-Cookie` value that clears the session cookie immediately.
pub fn clear_cookie_header() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

/// Extract the `mh_session` value from a `Cookie` request header.
pub fn token_from_cookie_header(header: &str) -> Option<&str> {
    header.split(';').find_map(|pair| {
        let pair = pair.trim();
        pair.strip_prefix(COOKIE_NAME)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

/// Minimal state for auth handlers/middleware — just the access code.
/// Decoupled from the heavy `AppState` so it is unit-testable without sled.
#[derive(Clone)]
pub struct AuthState {
    pub access_code: Arc<str>,
}

#[derive(Deserialize)]
struct LoginBody {
    code: String,
}

/// True when the request reached us over HTTPS (nginx sets X-Forwarded-Proto).
fn request_is_secure(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"))
}

/// True when a valid, unexpired session cookie is present.
fn has_valid_session(headers: &HeaderMap, access_code: &str) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(token_from_cookie_header)
        .is_some_and(|tok| verify_token(tok, access_code, now_unix()))
}

/// Best-effort client IP for audit logging (nginx sets X-Forwarded-For).
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map_or_else(|| "unknown".into(), |s| s.trim().to_string())
}

async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    let ip = client_ip(&headers);
    if !code_matches(&body.code, &state.access_code) {
        tracing::warn!(ip = %ip, "access-code login failed");
        // Small fixed delay to blunt online brute-force. Defense in depth behind
        // nginx's login rate-limit and the required high-entropy access code.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"authenticated": false})),
        )
            .into_response();
    }
    let token = mint_token(&state.access_code, now_unix());
    let cookie = session_cookie_header(&token, request_is_secure(&headers));
    tracing::info!(ip = %ip, "access-code login succeeded");
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({"authenticated": true})),
    )
        .into_response()
}

async fn status(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let ok = has_valid_session(&headers, &state.access_code);
    Json(serde_json::json!({ "authenticated": ok })).into_response()
}

async fn logout() -> Response {
    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_cookie_header())],
    )
        .into_response()
}

async fn health() -> &'static str {
    "ok"
}

/// Guard: pass through when a valid session cookie is present, else 401.
pub async fn require_auth(
    State(state): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    if has_valid_session(request.headers(), &state.access_code) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// The media id from `/api/v1/media/{id}/...`, if the path has that shape.
fn media_id_from_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/v1/media/")?;
    let id = rest.split('/').next()?;
    (!id.is_empty()).then_some(id)
}

/// The `token` query parameter, if present.
fn token_from_query(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| {
        pair.strip_prefix("token")
            .and_then(|rest| rest.strip_prefix('='))
            .filter(|v| !v.is_empty())
    })
}

/// The `token` query parameter, when it has the shape a playback token can have
/// (`{digits}.{hex}`). The charset check keeps an arbitrary caller-supplied value
/// from being reflected into a response body (the HLS playlist embeds the token
/// in the segment URLs it generates).
pub fn playback_token_query(query: Option<&str>) -> Option<&str> {
    let token = token_from_query(query?)?;
    let (exp, sig) = token.split_once('.')?;
    let well_formed = !exp.is_empty()
        && !sig.is_empty()
        && exp.bytes().all(|b| b.is_ascii_digit())
        && sig.bytes().all(|b| b.is_ascii_hexdigit());
    well_formed.then_some(token)
}

/// Guard for media byte-serving routes: a valid session cookie **or** a valid
/// `?token=` scoped to the media id in the path.
///
/// Only the endpoints an external player has to fetch by URL are wired to this —
/// stream, HLS segments, subtitle text. Everything that mutates state stays on
/// the cookie-only [`require_auth`], so a shared playback URL can read one title
/// and nothing else.
pub async fn require_media_auth(
    State(state): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    if has_valid_session(request.headers(), &state.access_code) {
        return next.run(request).await;
    }

    let uri = request.uri();
    let authorized = match (media_id_from_path(uri.path()), uri.query()) {
        (Some(media_id), Some(query)) => token_from_query(query).is_some_and(|token| {
            verify_playback_token(token, &state.access_code, media_id, now_unix())
        }),
        _ => false,
    };

    if authorized {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Public routes: login/status/logout + unguarded health probe.
pub fn auth_router(state: AuthState) -> Router {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/status", get(status))
        .route("/api/v1/auth/logout", post(logout))
        .route("/health", get(health))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "correct-horse-battery-staple";
    const NOW: i64 = 1_700_000_000;

    #[test]
    fn mint_then_verify_roundtrips() {
        let t = mint_token(CODE, NOW);
        assert!(verify_token(&t, CODE, NOW + 10));
    }

    #[test]
    fn expired_token_is_rejected() {
        let t = mint_token(CODE, NOW);
        assert!(!verify_token(&t, CODE, NOW + SESSION_TTL_SECS + 1));
    }

    #[test]
    fn wrong_code_rejected() {
        let t = mint_token(CODE, NOW);
        assert!(!verify_token(&t, "wrong-code", NOW + 10));
    }

    #[test]
    fn tampered_token_rejected() {
        let t = mint_token(CODE, NOW);
        // Flip the final hex char to a GUARANTEED-different one. (pop+push the
        // same literal char is a no-op when the signature already ends in it.)
        let mut chars: Vec<char> = t.chars().collect();
        if let Some(last) = chars.last_mut() {
            *last = if *last == '0' { '1' } else { '0' };
        }
        let tampered: String = chars.into_iter().collect();
        assert!(!verify_token(&tampered, CODE, NOW + 10));
    }

    #[test]
    fn malformed_token_rejected() {
        assert!(!verify_token("garbage", CODE, NOW));
        assert!(!verify_token("", CODE, NOW));
        assert!(!verify_token("123.", CODE, NOW));
    }

    #[test]
    fn code_matches_is_correct() {
        assert!(code_matches("abc", "abc"));
        assert!(!code_matches("abc", "abd"));
        assert!(!code_matches("abc", "abcd"));
        assert!(!code_matches("", "abc"));
    }

    #[test]
    fn cookie_header_has_expected_attributes() {
        let h = session_cookie_header("tok", true);
        assert!(h.starts_with("mh_session=tok"));
        assert!(h.contains("HttpOnly"));
        assert!(h.contains("SameSite=Lax"));
        assert!(h.contains("Path=/"));
        assert!(h.contains("Max-Age=2592000"));
        assert!(h.contains("Secure"));
        // Insecure (LAN http) must NOT set Secure.
        assert!(!session_cookie_header("tok", false).contains("Secure"));
    }

    #[test]
    fn clear_cookie_expires_immediately() {
        assert!(clear_cookie_header().contains("Max-Age=0"));
    }

    const MEDIA: &str = "badab8c6-7c54-4305-8e80-04a5c2ea2a02";

    #[test]
    fn playback_token_roundtrips() {
        let t = mint_playback_token(CODE, MEDIA, NOW);
        assert!(verify_playback_token(&t, CODE, MEDIA, NOW + 10));
    }

    #[test]
    fn playback_token_is_scoped_to_one_entry() {
        let t = mint_playback_token(CODE, MEDIA, NOW);
        let other = "11111111-2222-3333-4444-555555555555";
        assert!(!verify_playback_token(&t, CODE, other, NOW + 10));
    }

    #[test]
    fn playback_token_expires() {
        let t = mint_playback_token(CODE, MEDIA, NOW);
        assert!(!verify_playback_token(
            &t,
            CODE,
            MEDIA,
            NOW + PLAYBACK_TTL_SECS + 1
        ));
    }

    #[test]
    fn playback_token_needs_the_right_code() {
        let t = mint_playback_token(CODE, MEDIA, NOW);
        assert!(!verify_playback_token(&t, "wrong-code", MEDIA, NOW + 10));
    }

    #[test]
    fn playback_and_session_tokens_are_not_interchangeable() {
        // Domain separation: neither token type verifies as the other, so a
        // read-only playback URL can never be escalated into a session.
        let session = mint_token(CODE, NOW);
        assert!(!verify_playback_token(&session, CODE, MEDIA, NOW + 10));

        let playback = mint_playback_token(CODE, MEDIA, NOW);
        assert!(!verify_token(&playback, CODE, NOW + 10));
    }

    #[test]
    fn malformed_playback_token_rejected() {
        assert!(!verify_playback_token("garbage", CODE, MEDIA, NOW));
        assert!(!verify_playback_token("", CODE, MEDIA, NOW));
        assert!(!verify_playback_token("123.", CODE, MEDIA, NOW));
        assert!(!verify_playback_token(".abc", CODE, MEDIA, NOW));
        assert!(!verify_playback_token("notanumber.abc", CODE, MEDIA, NOW));
    }

    #[test]
    fn media_id_parsed_from_path() {
        assert_eq!(media_id_from_path("/api/v1/media/abc/stream"), Some("abc"));
        assert_eq!(
            media_id_from_path("/api/v1/media/abc/segment/seg0.ts"),
            Some("abc")
        );
        assert_eq!(
            media_id_from_path("/api/v1/media/abc/subtitles/0"),
            Some("abc")
        );
        assert_eq!(media_id_from_path("/api/v1/library/abc"), None);
        assert_eq!(media_id_from_path("/api/v1/media/"), None);
    }

    #[test]
    fn token_extracted_from_query() {
        assert_eq!(token_from_query("token=abc"), Some("abc"));
        assert_eq!(token_from_query("x=1&token=abc&y=2"), Some("abc"));
        assert_eq!(token_from_query("x=1"), None);
        assert_eq!(token_from_query("token="), None);
    }

    #[test]
    fn playback_token_query_rejects_odd_charsets() {
        // Only `{digits}.{hex}` may be echoed back into an HLS playlist.
        let good = mint_playback_token(CODE, MEDIA, NOW);
        assert_eq!(
            playback_token_query(Some(&format!("token={good}"))),
            Some(good.as_str())
        );
        assert_eq!(playback_token_query(Some("token=123.zz")), None);
        assert_eq!(playback_token_query(Some("token=abc.def")), None);
        assert_eq!(playback_token_query(Some("token=123.abc\"onload")), None);
        assert_eq!(playback_token_query(Some("token=nodot")), None);
        assert_eq!(playback_token_query(None), None);
    }

    #[test]
    fn extract_token_from_cookie_header() {
        assert_eq!(token_from_cookie_header("mh_session=abc"), Some("abc"));
        assert_eq!(
            token_from_cookie_header("foo=1; mh_session=abc; bar=2"),
            Some("abc")
        );
        assert_eq!(token_from_cookie_header("foo=1; bar=2"), None);
        assert_eq!(token_from_cookie_header(""), None);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod router_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // oneshot

    fn state() -> AuthState {
        AuthState {
            access_code: "secret-code".into(),
        }
    }

    #[tokio::test]
    async fn health_is_public_200() {
        let app = auth_router(state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn status_false_without_cookie() {
        let app = auth_router(state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"authenticated\":false"));
    }

    #[tokio::test]
    async fn login_wrong_code_401() {
        let app = auth_router(state());
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"code":"nope"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn login_right_code_sets_cookie() {
        let app = auth_router(state());
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"code":"secret-code"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let set_cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(set_cookie.starts_with("mh_session="));
    }

    use axum::middleware;
    use axum::routing::get as get_route;

    // A tiny protected router mirroring how server.rs wires the guard.
    fn protected_app() -> Router {
        async fn dummy() -> &'static str {
            "secret"
        }
        Router::new()
            .route("/api/v1/dummy", get_route(dummy))
            .route_layer(middleware::from_fn_with_state(state(), require_auth))
            .with_state(())
    }

    #[tokio::test]
    async fn protected_route_401_without_cookie() {
        let res = protected_app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/dummy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    const MEDIA_ID: &str = "badab8c6-7c54-4305-8e80-04a5c2ea2a02";

    // Mirrors how server.rs wires the media byte-serving routes.
    fn media_app() -> Router {
        async fn dummy() -> &'static str {
            "bytes"
        }
        Router::new()
            .route("/api/v1/media/{id}/stream", get_route(dummy))
            .route_layer(middleware::from_fn_with_state(state(), require_media_auth))
            .with_state(())
    }

    async fn media_status(uri: &str, cookie: Option<&str>) -> StatusCode {
        let mut req = Request::builder().uri(uri);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        media_app()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn media_route_401_without_cookie_or_token() {
        let uri = format!("/api/v1/media/{MEDIA_ID}/stream");
        assert_eq!(media_status(&uri, None).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn media_route_200_with_valid_playback_token() {
        // The AirPlay case: no cookie, capability in the query string.
        let token = mint_playback_token("secret-code", MEDIA_ID, now_unix());
        let uri = format!("/api/v1/media/{MEDIA_ID}/stream?token={token}");
        assert_eq!(media_status(&uri, None).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn media_route_401_with_token_for_another_entry() {
        let other = "11111111-2222-3333-4444-555555555555";
        let token = mint_playback_token("secret-code", other, now_unix());
        let uri = format!("/api/v1/media/{MEDIA_ID}/stream?token={token}");
        assert_eq!(media_status(&uri, None).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn media_route_401_with_forged_token() {
        let uri = format!("/api/v1/media/{MEDIA_ID}/stream?token=99999999999.deadbeef");
        assert_eq!(media_status(&uri, None).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn media_route_200_with_session_cookie() {
        let token = mint_token("secret-code", now_unix());
        let uri = format!("/api/v1/media/{MEDIA_ID}/stream");
        let cookie = format!("mh_session={token}");
        assert_eq!(media_status(&uri, Some(&cookie)).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn playback_token_does_not_open_the_cookie_guarded_api() {
        // A leaked playback URL must not reach anything that mutates state.
        let token = mint_playback_token("secret-code", MEDIA_ID, now_unix());
        let res = protected_app()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/dummy?token={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_route_200_with_valid_cookie() {
        let token = mint_token("secret-code", now_unix());
        let res = protected_app()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/dummy")
                    .header("cookie", format!("mh_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
}
