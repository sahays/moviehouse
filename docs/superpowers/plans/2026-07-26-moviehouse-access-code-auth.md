# MovieHouse Access-Code Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Gate all of MovieHouse behind a single env-configured access code with a stateless signed session cookie and an in-app React login screen.

**Architecture:** A pure `src/web/auth.rs` module mints/verifies HMAC-SHA256 session tokens (key = the access code, 30-day embedded expiry) and formats/parses the `mh_session` cookie. axum guard middleware protects all existing `/api/v1/*` routes; `/api/v1/auth/*` and `/health` stay public so the embedded React SPA can render its login screen. `Config::load()` reads process env before the `.env` file so the Docker container (compose `env_file`) receives the code and TMDB keys.

**Tech Stack:** Rust (axum 0.8, hmac, sha2, subtle, hex), React 19 + TypeScript (embedded via `build.rs`).

## Global Constraints

- Env var: `MOVIEHOUSE_ACCESS_CODE` (single gate secret). Cookie name: `mh_session`.
- Session TTL: 30 days (`2592000` seconds), expiry embedded in the signed token and enforced server-side.
- Token format: `"{exp_unix}.{hex(HMAC_SHA256(key=access_code, msg=exp_unix_ascii))}"`.
- Cookie attributes: `HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000`, and `Secure` **only** when the request arrived over HTTPS (`X-Forwarded-Proto: https`) — so plain-HTTP LAN access still works.
- Constant-time comparison (`subtle`) for the submitted-code check; HMAC verification is inherently constant-time.
- Protected: all existing `/api/v1/*` routes. Public: `/api/v1/auth/login`, `/api/v1/auth/status`, `/api/v1/auth/logout`, `/health`, and the SPA/static fallback.
- `serve` refuses to start if `MOVIEHOUSE_ACCESS_CODE` is empty.
- `Config::load()` resolves each key as `std::env::var(key)` first, then the parsed `.env` file map.
- Auth handlers and guard middleware use a lightweight `AuthState { access_code: Arc<str> }` (Clone), NOT the heavy `Arc<AppState>`, so they are unit-testable without opening sled.
- Rust gates (must pass at the end): `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` (repo lints include `unwrap_used`/`expect_used` = warn → deny). Frontend gates: `npm run build` (tsc+vite), `eslint .`, `prettier --check src`.

**Environment note:** Rust toolchain + Node are available locally. All build/test steps run locally. No server needed for this feature (the deploy steps live in the deploy plan).

---

### Task 1: Dependencies, env-first config, access code in state, startup guard

**Files:**
- Modify: `Cargo.toml` (add `hmac`, `sha2`, `subtle`)
- Modify: `src/main.rs` (Config struct + `Config::load()` + `cmd_serve` + AppState construction)
- Modify: `src/web/server.rs:18-22` (add `access_code` to `AppState`)

**Interfaces:**
- Produces: `AppState.access_code: String`; `Config.access_code: String`; a pure `pick(env_val: Option<String>, file_val: Option<String>) -> Option<String>` precedence helper and its `env_or_file(key, &HashMap<String,String>) -> Option<String>` wrapper.

> **Lint note:** `Cargo.toml` sets `unsafe_code = "forbid"`, so tests must NOT call `std::env::set_var` (it is `unsafe` in edition 2024 and `forbid` cannot be locally allowed). The precedence logic is therefore tested via the pure `pick` helper, not by mutating the process environment.

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

In the `[dependencies]` section, add (alphabetical-ish, near the other small crates):

```toml
hmac = "0.12"
sha2 = "0.10"
subtle = "2"
```

- [ ] **Step 2: Write the failing test for env-first resolution**

Add to `src/main.rs` (bottom of file):

```rust
#[cfg(test)]
mod config_tests {
    use super::pick;

    #[test]
    fn env_value_wins_when_present() {
        assert_eq!(pick(Some("env".into()), Some("file".into())).as_deref(), Some("env"));
        assert_eq!(pick(Some("env".into()), None).as_deref(), Some("env"));
    }

    #[test]
    fn empty_env_falls_back_to_file() {
        assert_eq!(pick(Some(String::new()), Some("file".into())).as_deref(), Some("file"));
    }

    #[test]
    fn file_used_when_env_absent() {
        assert_eq!(pick(None, Some("file".into())).as_deref(), Some("file"));
    }

    #[test]
    fn none_when_both_absent() {
        assert_eq!(pick(None, None), None);
        assert_eq!(pick(Some(String::new()), None), None);
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test --lib config_tests 2>&1 | tail -20` (or `cargo test config_tests`)
Expected: FAIL — `pick` not found / does not compile.

- [ ] **Step 4: Implement `env_or_file` and rewrite `Config`**

Replace the existing `struct Config { ... }` and `impl Config { fn load() ... }` block in `src/main.rs` with:

```rust
/// App configuration resolved from process env first, then the `.env` file.
struct Config {
    tmdb_api_key: String,
    tmdb_read_access_token: String,
    access_code: String,
}

/// Precedence: a non-empty env value wins; otherwise the file value.
/// Pure (no env access) so it is testable without mutating the process
/// environment (which `unsafe_code = "forbid"` disallows).
fn pick(env_val: Option<String>, file_val: Option<String>) -> Option<String> {
    match env_val {
        Some(v) if !v.is_empty() => Some(v),
        _ => file_val,
    }
}

/// Resolve a key: process environment wins, then the parsed `.env` map.
/// (The Docker container receives values as process env via compose `env_file`;
/// local runs use the `.env` file. Both must work.)
fn env_or_file(key: &str, file: &std::collections::HashMap<String, String>) -> Option<String> {
    pick(std::env::var(key).ok(), file.get(key).cloned())
}

impl Config {
    fn load() -> Self {
        let mut values = std::collections::HashMap::new();
        if let Ok(contents) = std::fs::read_to_string(".env") {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    values.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }
        Self {
            tmdb_api_key: env_or_file("TMDB_API_KEY", &values).unwrap_or_default(),
            tmdb_read_access_token: env_or_file("TMDB_READ_ACCESS_TOKEN", &values)
                .unwrap_or_default(),
            access_code: env_or_file("MOVIEHOUSE_ACCESS_CODE", &values).unwrap_or_default(),
        }
    }
}
```

- [ ] **Step 5: Add `access_code` to `AppState`**

In `src/web/server.rs`, change the `AppState` struct (currently lines 18-22):

```rust
pub struct AppState {
    pub manager: Arc<SessionManager>,
    pub store: Arc<Store>,
    pub transcode: TranscodeHandle,
    pub access_code: String,
}
```

- [ ] **Step 6: Startup guard + thread access_code in `cmd_serve`**

In `src/main.rs` `cmd_serve`, immediately after `let config = Config::load();`, add:

```rust
    if config.access_code.trim().is_empty() {
        anyhow::bail!(
            "MOVIEHOUSE_ACCESS_CODE is required to serve the web UI. \
             Set it in .env or the environment (generate one: openssl rand -hex 24)."
        );
    }
```

Then in the same function, update the `AppState` construction (currently `let state = Arc::new(web::server::AppState { manager, store, transcode: transcode_handle });`) to include the code:

```rust
    let state = Arc::new(web::server::AppState {
        manager,
        store,
        transcode: transcode_handle,
        access_code: config.access_code.clone(),
    });
```

- [ ] **Step 7: Run the config tests — verify they pass**

Run: `cargo test config_tests 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 8: Verify the crate builds**

Run: `cargo build 2>&1 | tail -20`
Expected: builds successfully (frontend rebuild is expected via build.rs).

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml src/main.rs src/web/server.rs
git commit -m "feat(auth): env-first config, access_code in state, serve startup guard"
```

---

### Task 2: Auth core module (token mint/verify, cookie format, code compare)

**Files:**
- Create: `src/web/auth.rs`
- Modify: `src/web/mod.rs` (add `pub mod auth;`)

**Interfaces:**
- Produces (all `pub`): `COOKIE_NAME: &str`, `SESSION_TTL_SECS: i64`, `mint_token(access_code: &str, now_unix: i64) -> String`, `verify_token(token: &str, access_code: &str, now_unix: i64) -> bool`, `code_matches(submitted: &str, configured: &str) -> bool`, `session_cookie_header(token: &str, secure: bool) -> String`, `clear_cookie_header() -> String`, `token_from_cookie_header(header: &str) -> Option<&str>`, `now_unix() -> i64`.

- [ ] **Step 1: Register the module**

In `src/web/mod.rs`, add a line:

```rust
pub mod auth;
```

- [ ] **Step 2: Write the failing tests**

Create `src/web/auth.rs` with ONLY the test module first (so it fails to compile → drives the impl):

```rust
//! Access-code auth: stateless HMAC-signed session cookie.

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
        let mut t = mint_token(CODE, NOW);
        t.pop();
        t.push('0');
        assert!(!verify_token(&t, CODE, NOW + 10));
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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib web::auth 2>&1 | tail -20`
Expected: FAIL — functions/constants not defined (compile error).

- [ ] **Step 4: Implement the module**

Prepend the implementation ABOVE the `#[cfg(test)] mod tests` block in `src/web/auth.rs`:

```rust
use hmac::{Hmac, Mac};
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// HMAC-SHA256(key = access_code, msg) → lowercase hex.
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
```

Note: `code_matches` with `ct_eq` returns false for differing lengths (`subtle`'s slice impl returns `Choice(0)` when lengths differ). The `now_unix` `as i64` cast is covered by the repo's `cast_possible_truncation = "allow"`. `sign` is written `unwrap`/`expect`-free (see its doc comment) to satisfy the `unwrap_used`/`expect_used` deny.

- [ ] **Step 5: Run tests — verify they pass**

Run: `cargo test --lib web::auth 2>&1 | tail -20`
Expected: PASS (all auth tests).

- [ ] **Step 6: Clippy-clean the new module**

Run: `cargo clippy --lib 2>&1 | tail -20`
Expected: no warnings for `src/web/auth.rs`. If `expect_used`/`cast_possible_truncation` fire, add a scoped `#[allow(...)]` with a one-line justification comment (the `now_unix` cast and the HMAC `expect` are the likely spots).

- [ ] **Step 7: Commit**

```bash
git add src/web/auth.rs src/web/mod.rs Cargo.toml
git commit -m "feat(auth): HMAC session token + cookie helpers (pure, tested)"
```

---

### Task 3: Auth route handlers + /health, wired as a public sub-router

**Files:**
- Modify: `src/web/auth.rs` (add `AuthState`, handlers, and a `pub fn auth_router(AuthState) -> Router`)
- Modify: `src/web/server.rs` (build `AuthState` from `AppState.access_code`, merge the public router)

**Interfaces:**
- Consumes: `mint_token`, `verify_token`, `code_matches`, `session_cookie_header`, `clear_cookie_header`, `token_from_cookie_header`, `now_unix` (Task 2).
- Produces: `#[derive(Clone)] pub struct AuthState { pub access_code: Arc<str> }`; `pub fn auth_router(state: AuthState) -> Router` exposing `POST /api/v1/auth/login`, `GET /api/v1/auth/status`, `POST /api/v1/auth/logout`, `GET /health`.

- [ ] **Step 1: Write the failing router test**

Add a second test module at the bottom of `src/web/auth.rs`:

```rust
#[cfg(test)]
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib web::auth::router_tests 2>&1 | tail -20`
Expected: FAIL — `AuthState` / `auth_router` not defined.

- [ ] **Step 3: Implement `AuthState`, handlers, and `auth_router`**

Add to `src/web/auth.rs` (above the test modules), keeping imports at the top of the file:

```rust
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

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
```

If `serde_json` is not already imported in this file, the fully-qualified `serde_json::json!` calls above avoid needing a `use`. `serde_json` is already a workspace dependency.

- [ ] **Step 4: Merge the public router in `create_router`**

In `src/web/server.rs`, at the start of `create_router` add the auth state, and change the final `Router::new().merge(api)...` to also merge the public auth router. Replace the final return expression:

```rust
    let auth_state = super::auth::AuthState {
        access_code: state.access_code.clone().into(),
    };

    // CORS: allow any origin for LAN access (phones, TVs, other devices).
    // Credentials are not allowed (no .allow_credentials), limiting CSRF risk.
    Router::new()
        .merge(super::auth::auth_router(auth_state))
        .merge(api)
        .fallback(static_handler)
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                .allow_headers([header::CONTENT_TYPE]),
        )
}
```

(The guard middleware on `api` is added in Task 4; this task only wires the public routes.)

- [ ] **Step 5: Run the router tests — verify they pass**

Run: `cargo test --lib web::auth 2>&1 | tail -20`
Expected: PASS (all auth + router tests).

- [ ] **Step 6: Build check**

Run: `cargo build 2>&1 | tail -20`
Expected: builds.

- [ ] **Step 7: Commit**

```bash
git add src/web/auth.rs src/web/server.rs
git commit -m "feat(auth): login/status/logout + /health public routes"
```

---

### Task 4: Guard middleware over the protected API

**Files:**
- Modify: `src/web/auth.rs` (add `require_auth` middleware + a test)
- Modify: `src/web/server.rs` (apply `.route_layer` to the `api` router)

**Interfaces:**
- Consumes: `AuthState`, `has_valid_session` (Task 3).
- Produces: `pub async fn require_auth(State<AuthState>, Request, Next) -> Response`.

- [ ] **Step 1: Write the failing guard test**

Add to the `router_tests` module in `src/web/auth.rs`:

```rust
    use axum::middleware;
    use axum::routing::get as get_route;

    // A tiny protected router mirroring how server.rs wires the guard.
    fn protected_app() -> Router {
        async fn dummy() -> &'static str { "secret" }
        Router::new()
            .route("/api/v1/dummy", get_route(dummy))
            .route_layer(middleware::from_fn_with_state(state(), require_auth))
            .with_state(())
    }

    #[tokio::test]
    async fn protected_route_401_without_cookie() {
        let res = protected_app()
            .oneshot(Request::builder().uri("/api/v1/dummy").body(Body::empty()).unwrap())
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib web::auth::router_tests 2>&1 | tail -20`
Expected: FAIL — `require_auth` not defined.

- [ ] **Step 3: Implement `require_auth`**

Add to `src/web/auth.rs` (near the handlers). Add `use axum::extract::Request;` and `use axum::middleware::Next;` to the file's imports:

```rust
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
```

- [ ] **Step 4: Apply the guard to the protected `api` router**

In `src/web/server.rs`, the `api` router currently ends with `.with_state(state.clone());`. Insert the guard layer immediately after `.with_state(state.clone())` and before the semicolon closes the statement — i.e. change:

```rust
        .with_state(state.clone());
```

to:

```rust
        .with_state(state.clone())
        .route_layer(axum::middleware::from_fn_with_state(
            super::auth::AuthState { access_code: state.access_code.clone().into() },
            super::auth::require_auth,
        ));
```

(`route_layer` runs the guard only for matched `/api/v1/*` routes — never the fallback SPA, and the separately-merged public `auth_router` is unaffected.)

- [ ] **Step 5: Run all auth tests — verify they pass**

Run: `cargo test --lib web::auth 2>&1 | tail -20`
Expected: PASS (pure + router + guard tests).

- [ ] **Step 6: Full test + clippy + build**

Run: `cargo test 2>&1 | tail -15 && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -15`
Expected: tests pass; clippy clean (no warnings).

- [ ] **Step 7: Commit**

```bash
git add src/web/auth.rs src/web/server.rs
git commit -m "feat(auth): guard middleware protecting /api/v1/*"
```

---

### Task 5: Frontend login screen + auth gate

**Files:**
- Create: `frontend/src/lib/api.ts`
- Create: `frontend/src/components/Login.tsx`
- Modify: `frontend/src/App.tsx`

**Interfaces:**
- Consumes: `GET /api/v1/auth/status`, `POST /api/v1/auth/login`, `POST /api/v1/auth/logout`.
- Produces: `apiFetch(input, init?)` (fetch wrapper that dispatches a `mh-unauthorized` event on 401), `<Login onSuccess>` component.

- [ ] **Step 1: Read the current App shell**

Run: `sed -n '1,60p' frontend/src/App.tsx`
Purpose: learn the top-level render + how the app tree is returned, so the gate wraps it without disturbing existing structure. (No code change in this step.)

- [ ] **Step 2: Create the fetch wrapper `frontend/src/lib/api.ts`**

```ts
// Thin fetch wrapper. Same-origin cookies are sent automatically; on a 401 we
// broadcast so the app can drop back to the login screen (mid-session expiry).
export async function apiFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  const res = await fetch(input, init);
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent("mh-unauthorized"));
  }
  return res;
}

export async function checkAuth(): Promise<boolean> {
  try {
    const res = await fetch("/api/v1/auth/status");
    if (!res.ok) return false;
    const data = (await res.json()) as { authenticated: boolean };
    return data.authenticated;
  } catch {
    return false;
  }
}

export async function login(code: string): Promise<boolean> {
  const res = await fetch("/api/v1/auth/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code }),
  });
  return res.ok;
}
```

- [ ] **Step 3: Create `frontend/src/components/Login.tsx`**

```tsx
import { useState, type FormEvent } from "react";
import { login } from "../lib/api";

export function Login({ onSuccess }: { onSuccess: () => void }) {
  const [code, setCode] = useState("");
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(false);
    const ok = await login(code);
    setBusy(false);
    if (ok) {
      onSuccess();
    } else {
      setError(true);
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center p-6">
      <form
        onSubmit={handleSubmit}
        className="w-full max-w-sm space-y-4 rounded-lg border p-6"
      >
        <h1 className="text-lg font-semibold">MovieHouse</h1>
        <label className="block text-sm" htmlFor="access-code">
          Access code
        </label>
        <input
          id="access-code"
          type="password"
          autoComplete="current-password"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          className="w-full rounded border px-3 py-2"
          autoFocus
        />
        {error && (
          <p role="alert" className="text-sm text-red-600">
            Invalid access code.
          </p>
        )}
        <button
          type="submit"
          disabled={busy || code.length === 0}
          className="w-full rounded bg-black px-3 py-2 text-white disabled:opacity-50"
        >
          {busy ? "Checking…" : "Enter"}
        </button>
      </form>
    </main>
  );
}
```

Note: match the app's existing styling conventions if they differ (the project uses Tailwind v4 + shadcn). If there is an existing `Button`/`Input` in `frontend/src/components`, prefer those for visual consistency; the classes above are a plain-Tailwind fallback that will still render correctly.

- [ ] **Step 4: Gate the app in `frontend/src/App.tsx`**

Wrap the existing top-level component. Add near the top of the file:

```tsx
import { useEffect, useState } from "react";
import { Login } from "./components/Login";
import { checkAuth } from "./lib/api";
```

Introduce an auth gate around whatever `App` currently renders. Rename the existing exported `App` component body to an inner component (e.g. `AppInner`) if needed, and export a new gate:

```tsx
export default function App() {
  const [authed, setAuthed] = useState<boolean | null>(null);

  useEffect(() => {
    checkAuth().then(setAuthed);
    const onUnauth = () => setAuthed(false);
    window.addEventListener("mh-unauthorized", onUnauth);
    return () => window.removeEventListener("mh-unauthorized", onUnauth);
  }, []);

  if (authed === null) return null; // brief auth check
  if (!authed) return <Login onSuccess={() => setAuthed(true)} />;
  return <AppInner />;
}
```

Ensure the previously-`export default`ed component is renamed to `AppInner` (remove its `export default`), leaving exactly one `export default function App`.

- [ ] **Step 5: Build the frontend (tsc + vite)**

Run: `cd frontend && npm run build 2>&1 | tail -25`
Expected: `tsc -b` passes (no type errors) and vite writes `dist/`. Fix any type errors (e.g. the AppInner rename) until clean.

- [ ] **Step 6: Lint + format check**

Run: `cd frontend && npx eslint . 2>&1 | tail -25 && npx prettier --check src 2>&1 | tail -10`
Expected: eslint reports no errors; prettier reports all matched files formatted. If prettier flags files, run `npx prettier --write src` on the NEW/modified files only and re-check.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/lib/api.ts frontend/src/components/Login.tsx frontend/src/App.tsx
git commit -m "feat(auth): React login screen + auth gate + 401-aware fetch"
```

---

### Task 6: Wire the app data-loading through the 401-aware fetch + docs/env

**Files:**
- Modify: `frontend/src/App.tsx` or the primary polling hook (whichever issues the library poll) — route the recurring library fetch through `apiFetch`
- Modify: `.env.example` (replace Basic Auth vars with the access code)
- Modify: `docs/superpowers/specs/2026-07-26-moviehouse-production-deploy-design.md` and `docs/superpowers/plans/2026-07-26-moviehouse-production-deploy.md` (fold in the auth amendments)

**Interfaces:**
- Consumes: `apiFetch` (Task 5).

- [ ] **Step 1: Find the recurring data fetch**

Run: `grep -rn "fetch(\"/api/v1/library\"\|fetch(\`/api/v1/library\|setInterval\|3000" frontend/src | head`
Purpose: locate the ~3s library poll so a mid-session 401 (expired cookie) triggers the login screen.

- [ ] **Step 2: Route the primary poll through `apiFetch`**

In the file that performs the recurring library fetch, import `apiFetch` (`import { apiFetch } from "../lib/api";` — adjust relative path) and replace that fetch call's `fetch(` with `apiFetch(`. Only the recurring/primary data load needs this; do not churn every fetch call (YAGNI — the poll re-fires every few seconds and will surface the 401 promptly).

- [ ] **Step 3: Rebuild + lint to confirm no breakage**

Run: `cd frontend && npm run build 2>&1 | tail -15 && npx eslint . 2>&1 | tail -15`
Expected: clean build + lint.

- [ ] **Step 4: Update `.env.example`**

In `.env.example`, replace the Basic Auth block:

```
# ── Deploy: Basic Auth (nginx) ────────────────────────────────────
# Anyone with these credentials has full control of the torrent engine,
# file browser, and library. Treat as an admin password.
BASIC_AUTH_USER=admin
BASIC_AUTH_PASSWORD=change_me
```

with the access-code block:

```
# ── Access code (in-app auth) ─────────────────────────────────────
# Single gate for the whole app. Anyone with this code has full control
# of the torrent engine, file browser, and library. Generate a strong one:
#   openssl rand -hex 24
# Rotating this value logs out every device.
MOVIEHOUSE_ACCESS_CODE=change_me
```

- [ ] **Step 5: Verify `.env.example` parses**

Run: `bash -n <(sed 's/#.*//' .env.example) && echo OK`
Expected: `OK`

- [ ] **Step 6: Fold auth amendments into the deploy plan/spec docs**

Edit `docs/superpowers/specs/2026-07-26-moviehouse-production-deploy-design.md` and `docs/superpowers/plans/2026-07-26-moviehouse-production-deploy.md`:
- In the deploy **spec**: change the Auth decision row to "in-app access-code auth (see access-code-auth spec)"; note nginx Basic Auth is removed; health probe is `/health`.
- In the deploy **plan**: (a) Task 3 healthcheck `test:` → `/health`; (b) Task 4 nginx template — remove the `auth_basic`/`auth_basic_user_file` lines, add `location = /api/v1/auth/login { limit_req zone=auth burst=5 nodelay; include /etc/nginx/conf.d/proxy_params.inc; proxy_pass http://moviehouse-web:__PORT__; }`; (c) Task 5 check-deps — replace the Basic Auth mention, add a check that `MOVIEHOUSE_ACCESS_CODE` is set in `.env`; (d) Task 7 deploy.sh — delete the htpasswd generation (step 4/5) and the htpasswd copy in step 10, add a guard that `MOVIEHOUSE_ACCESS_CODE` is set and not `change_me`; (e) Task 8 — the `401 vs 200` checks now exercise `/api/v1/auth/login` + a protected path with the `mh_session` cookie rather than Basic Auth.

Keep edits surgical — these are doc updates so the deploy work resumes against a correct plan.

- [ ] **Step 7: Commit**

```bash
git add frontend/src .env.example docs/superpowers
git commit -m "feat(auth): route poll through 401-aware fetch; deploy docs + env for access code"
```

---

### Task 7: Full-stack verification

**Files:** none (verification only)

- [ ] **Step 1: Full Rust gate**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -15 && cargo test 2>&1 | tail -15`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 2: Full frontend gate**

Run: `cd frontend && npm run build 2>&1 | tail -15 && npx eslint . 2>&1 | tail -10 && npx prettier --check src 2>&1 | tail -10`
Expected: build + lint + format all clean.

- [ ] **Step 3: Runtime smoke test — startup guard**

Run: `MOVIEHOUSE_ACCESS_CODE= ./target/release/moviehouse serve --bind 127.0.0.1:9187 2>&1 | head -3 || true` (build release first if needed: `cargo build --release`)
Expected: exits with the error `MOVIEHOUSE_ACCESS_CODE is required...` (does NOT start serving).

- [ ] **Step 4: Runtime smoke test — auth flow**

```bash
MOVIEHOUSE_ACCESS_CODE=test-code-123 ./target/release/moviehouse serve --bind 127.0.0.1:9187 &
SRV=$!; sleep 2
echo "health:"; curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9187/health
echo "status (no cookie):"; curl -s http://127.0.0.1:9187/api/v1/auth/status
echo "protected without cookie:"; curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9187/api/v1/library
echo "login wrong:"; curl -s -o /dev/null -w "%{http_code}\n" -X POST -H "Content-Type: application/json" -d '{"code":"nope"}' http://127.0.0.1:9187/api/v1/auth/login
echo "login right + use cookie:"; curl -s -c /tmp/mh.jar -X POST -H "Content-Type: application/json" -d '{"code":"test-code-123"}' http://127.0.0.1:9187/api/v1/auth/login >/dev/null
echo "protected with cookie:"; curl -s -b /tmp/mh.jar -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9187/api/v1/library
kill $SRV 2>/dev/null; rm -f /tmp/mh.jar
```
Expected: `health: 200`; status shows `{"authenticated":false}`; protected-without-cookie `401`; login-wrong `401`; protected-with-cookie `200`.

- [ ] **Step 5: Commit (if any incidental fixes were needed)**

```bash
git add -A && git commit -m "test(auth): full-stack verification fixes" || echo "nothing to commit"
```

---

## Self-Review

**Spec coverage** (against `2026-07-26-moviehouse-access-code-auth-design.md`):
- `MOVIEHOUSE_ACCESS_CODE` single gate → Task 1 (config), Task 3 (login). ✓
- Stateless signed cookie, key = access code, 30-day expiry → Task 2. ✓
- Endpoints login/status/logout + `/health` → Task 3. ✓
- Guard over `/api/v1/*`, public auth + health + SPA → Task 4. ✓
- `Config::load()` env-first (fixes Docker TMDB too) → Task 1. ✓
- Startup refuses if code empty → Task 1 (Step 6), verified Task 7 (Step 3). ✓
- Secure cookie only on HTTPS (X-Forwarded-Proto) → Task 2 + Task 3. ✓
- React login + gate + 401-aware fetch → Tasks 5, 6. ✓
- Constant-time compare (subtle) → Task 2. ✓
- nginx `zone=auth` login rate-limit + deploy amendments (drop Basic Auth, health→/health) → Task 6 (docs) . ✓
- `.env.example` access code → Task 6. ✓

**Placeholder scan:** No TBD/TODO. Every code step has complete code. Task 5 Step 1 and Task 6 Step 1 are read/grep steps (deliberately no code — they locate an existing integration point the implementer must see before editing).

**Type/name consistency:** `AuthState { access_code: Arc<str> }`, `COOKIE_NAME="mh_session"`, `SESSION_TTL_SECS=2592000`, functions `mint_token`/`verify_token`/`code_matches`/`session_cookie_header`/`clear_cookie_header`/`token_from_cookie_header`/`now_unix`/`require_auth`/`auth_router` are used identically across Tasks 2–4. `AppState.access_code: String` (Task 1) is `.clone().into()`-converted to `Arc<str>` for `AuthState` at both wiring sites (Task 3 Step 4, Task 4 Step 4) — consistent. Frontend `apiFetch`/`checkAuth`/`login` (Task 5) reused in Task 6.
