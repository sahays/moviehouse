# MovieHouse Access-Code Auth — Design

**Date:** 2026-07-26
**Status:** Approved decisions, pending spec review
**Related:** `2026-07-26-moviehouse-production-deploy-design.md` (this supersedes that spec's nginx Basic Auth)

## Goal

Gate the whole MovieHouse app behind a single **access code** so it can be exposed
publicly at `moviehouse.niniconai.com`. In-app (axum + React), no external auth
service, no database. Chosen model (from brainstorming): **master-code-only, single
gate** — enter one env-configured code → a 30-day session cookie → full access. No
per-person invite codes, no roles, no code-management UI (deferred).

Inspired by the invite-code pattern in the user's `superover` repo, reduced to the
single-gate case the user selected.

## Model

- One secret: env var **`MOVIEHOUSE_ACCESS_CODE`** (high-entropy). This is the gate.
- Enter it on a login screen → receive a **30-day** session cookie → full access.
- **Revocation** = rotate `MOVIEHOUSE_ACCESS_CODE`. Because the cookie is signed with
  a key derived from the code, rotating it invalidates every outstanding cookie
  automatically. No server-side session store, no cleanup.

## Session: stateless signed cookie (no storage)

- Cookie name **`mh_session`**, attributes `HttpOnly; Secure; SameSite=Lax; Path=/;
  Max-Age=2592000` (30 days).
- Value = the session **expiry** (unix seconds), stored in a **signed** cookie whose
  signing key is `cookie::Key::derive_from(MOVIEHOUSE_ACCESS_CODE bytes)`.
- Validation on each guarded request: (1) the signed cookie verifies (integrity, via
  the derived key — constant-time), AND (2) the embedded expiry is still in the
  future. Either failure → `401`.
- No sled tree, no SQLite. Rotating the access code changes the derived key, so old
  cookies stop verifying — instant global logout.

**Crypto/deps:** use `axum-extra` (feature `cookie`) `SignedCookieJar` for
sign/verify, and `subtle` for constant-time comparison of the submitted code against
`MOVIEHOUSE_ACCESS_CODE`. Both are small, vetted crates. No hand-rolled crypto.

## Endpoints (new)

All under the existing axum router in `src/web/`.

| Method | Path | Guard | Purpose |
|---|---|---|---|
| POST | `/api/v1/auth/login` | public | Body `{ "code": "..." }`. Constant-time compare to `MOVIEHOUSE_ACCESS_CODE`. On match: set `mh_session`, return `200 {"authenticated":true}`. On mismatch: `401`. |
| GET | `/api/v1/auth/status` | public | Returns `{ "authenticated": bool }` by validating the cookie. Lets the SPA decide login vs app. |
| POST | `/api/v1/auth/logout` | public | Clears `mh_session` (Max-Age=0), returns `200`. |
| GET | `/health` | public | Returns `200 "ok"`. Unguarded liveness probe for the container/deploy (the old `/api/v1/library/health` is now behind the guard). |

## Guard (axum middleware)

- A `middleware::from_fn_with_state` layer applied via `.route_layer(...)` to the
  **protected** API routes only.
- **Protected:** every `/api/v1/*` route that exists today (torrents, library, media,
  ws, settings, system, filesystem, metadata, transcode).
- **Public (not guarded):** `/api/v1/auth/*`, `/health`, and the SPA/static fallback
  (`static_handler`) — the React bundle is just JS/CSS with no secrets and must load
  so the login screen can render.
- Guard logic: read `mh_session` via `SignedCookieJar` (keyed by the derived key);
  if absent/invalid/expired → `401`. Otherwise pass through.
- Streaming (`/api/v1/media/*`) and the WebSocket (`/api/v1/ws`) are protected; the
  browser sends the cookie on `<video>`/`fetch`/WS handshakes (same-origin), so
  playback and progress work once logged in.

**Router wiring (`src/web/server.rs`):** split the current single `api` router into
`protected` (today's routes, plus `.route_layer(auth middleware)`) and `public`
(`/api/v1/auth/*` + `/health`), then `Router::new().merge(public).merge(protected)
.fallback(static_handler).layer(cors)`. `route_layer` scopes the guard to matched
routes and never runs on the fallback, so the SPA stays public.

## AppState / config changes

- **`AppState`** (`src/web/server.rs`) gains `pub access_code: String` (or an
  `Arc<str>`), threaded from `cmd_serve` in `src/main.rs`.
- **`Config`** (`src/main.rs`) gains `access_code: String`.
- **`Config::load()` must read process env first, then the `.env` file** — for each
  key try `std::env::var(key)` and fall back to the parsed `.env` map. This is
  required because the Docker container gets values via compose `env_file` as
  **process env** (there is no `.env` file in the image), and today's loader only
  reads a file. This fix also repairs TMDB key loading in the container.
- Add the same env-first lookup for `TMDB_API_KEY` and `TMDB_READ_ACCESS_TOKEN`
  (same code path — no extra logic).

## Startup guard

- In `cmd_serve`, if `access_code` is empty, **refuse to start** with a clear error:
  `MOVIEHOUSE_ACCESS_CODE is required to serve the web UI`. This prevents accidentally
  exposing an ungated instance. (CLI subcommands other than `serve` — download,
  magnet, info — are unaffected.)

## Frontend (embedded React)

- New `src/components/Login.tsx`: a single code input + submit → `POST
  /api/v1/auth/login`. On `200`, re-check status and enter the app; on `401`, show an
  inline error.
- `src/App.tsx`: on mount, `GET /api/v1/auth/status`. While loading, render nothing/a
  spinner; if `authenticated:false`, render `<Login/>`; else render the app.
- Shared fetch helper (`src/lib/api.ts` or extend `src/lib/utils.ts`): wrap `fetch`
  so a `401` response flips app state back to the login screen (handles mid-session
  cookie expiry). Same-origin cookies are sent automatically — no `credentials`
  changes needed.
- Must pass the existing gates: `eslint .` and `prettier --check src` (typed, no
  `any` leaks, formatted).

## Security

- Constant-time compare of the submitted code (`subtle`) — no timing oracle.
- Cookie `HttpOnly` (no JS access) + `Secure` (HTTPS only in prod) + `SameSite=Lax`.
- nginx rate-limits the login endpoint by **reusing bharatsc's existing `zone=auth`
  (3 r/s)** — add `location = /api/v1/auth/login { limit_req zone=auth burst=5
  nodelay; ... }` to the vhost. No http-level nginx change needed.
- CORS is unchanged (`allow_origin(Any)`, **no** `allow_credentials`), so cross-origin
  callers cannot ride the cookie; only the same-origin SPA authenticates. Acceptable.
- The access code should be long/random; document generating one with
  `openssl rand -hex 24`.

## Impact on the deployment plan/spec

Amend `2026-07-26-moviehouse-production-deploy-design.md` and its plan:

1. **nginx vhost (Task 4):** remove the `auth_basic` + `auth_basic_user_file` lines;
   add the `location = /api/v1/auth/login` rate-limit block.
2. **deploy.sh (Task 7):** remove htpasswd generation and the htpasswd install/copy.
3. **check-deps / .env (Task 1, 5):** replace `BASIC_AUTH_USER` / `BASIC_AUTH_PASSWORD`
   with `MOVIEHOUSE_ACCESS_CODE`; `deploy.sh` validates it is set and non-trivial.
4. **Health probe (Tasks 3, 7, 8):** point the container healthcheck and deploy
   verification at **`/health`** (unguarded) instead of `/api/v1/library/health`.
5. **Dockerfile (Task 2):** unchanged (already written; still needs a verified build).

## Out of scope (per user's choice)

- No per-person invite codes, expiry-per-code, revoke-by-code, roles, or admin UI.
- No server-side session store. (If per-person revocable codes are wanted later, add
  a sled `invite_codes` + `sessions` two-tree design — additive, no rework of this.)
- No password reset / user accounts (there are no accounts).

## Testing strategy

- **Rust unit tests:** cookie sign→verify round-trip; expired cookie rejected;
  tampered cookie rejected; wrong code → `401`; right code → cookie set;
  `Config::load()` prefers process env over `.env` file.
- **Guard integration test (axum):** a protected route returns `401` without a cookie
  and `200`/expected with a valid cookie; `/health` and `/api/v1/auth/status` return
  `200` without a cookie.
- **Frontend:** `eslint` + `prettier --check` + `tsc -b` (via `npm run build`) pass.
- **Manual (post-deploy):** unauth request to a protected path → 401; login → cookie;
  library loads; a protected media stream plays; rotating the code forces re-login.

## Open items for reviewer

- Confirm names `MOVIEHOUSE_ACCESS_CODE` and cookie `mh_session`.
- Confirm the stateless-signed-cookie choice (vs. a sled `sessions` tree that would
  allow "log out all devices" without rotating the code). Design recommends stateless.
