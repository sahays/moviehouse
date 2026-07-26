//! Access-code auth: stateless HMAC-signed session cookie.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Session cookie name.
pub const COOKIE_NAME: &str = "mh_session";
/// Session lifetime: 30 days.
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Current unix time in seconds.
pub fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Wall-clock seconds since epoch won't approach i64::MAX for billions of
    // years, so this cast never actually wraps.
    #[allow(clippy::cast_possible_wrap)]
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
        pair.strip_prefix(concat!("mh_session", "="))
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

async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    if !code_matches(&body.code, &state.access_code) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"authenticated": false})))
            .into_response();
    }
    let token = mint_token(&state.access_code, now_unix());
    let cookie = session_cookie_header(&token, request_is_secure(&headers));
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
    (StatusCode::OK, [(header::SET_COOKIE, clear_cookie_header())]).into_response()
}

async fn health() -> &'static str {
    "ok"
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
        AuthState { access_code: "secret-code".into() }
    }

    #[tokio::test]
    async fn health_is_public_200() {
        let app = auth_router(state());
        let res = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
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
}
