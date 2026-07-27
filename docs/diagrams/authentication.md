# Access-code authentication

A single access code gates the whole app. Login mints a stateless HMAC-SHA256 session cookie; a middleware verifies it on every `/api/v1/*` route except the public auth/health endpoints. **The access code is both the login secret and the HMAC key** (`src/web/auth.rs`), so rotating `MOVIEHOUSE_ACCESS_CODE` invalidates every session at once.

## Startup & router wiring

At boot, `cmd_serve` refuses to start if the code is shorter than 16 chars, then wraps the API router with the auth middleware while leaving the auth router public.

```mermaid
flowchart TB
  A[Config::load — MOVIEHOUSE_ACCESS_CODE] --> B{"len >= 16?"}
  B -- no --> C[bail! refuse to start]
  B -- yes --> D[AppState.access_code]
  D --> E[api router: /api/v1/* + ws]
  E --> F[route_layer require_auth]
  D --> G[auth_router: login / status / logout / health — PUBLIC]
  F --> H[merge + SPA fallback + CorsLayer]
  G --> H
```

## (A) Login

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant UI as Login.tsx / api.ts
    participant Login as login handler (auth.rs)

    User->>UI: submit access code
    UI->>Login: POST /api/v1/auth/login { code }
    Login->>Login: code_matches (constant-time ct_eq vs configured code)
    alt wrong code
        Login->>Login: warn(ip), sleep 500ms (anti-brute-force)
        Login-->>UI: 401 { authenticated: false }
        UI-->>User: "Invalid access code."
    else correct
        Login->>Login: mint_token = "{exp}.{HMAC(code, exp)}", exp = now + 30d
        Login->>Login: request_is_secure? (x-forwarded-proto == https)
        Login-->>UI: 200 Set-Cookie: mh_session=… HttpOnly, SameSite=Lax, [Secure]
        UI-->>User: onSuccess → leave login screen
    end
```

## (B) Guarded request

```mermaid
sequenceDiagram
    autonumber
    actor Browser
    participant MW as require_auth middleware
    participant H as API handler

    Browser->>MW: GET /api/v1/… (Cookie: mh_session=…)
    MW->>MW: token_from_cookie_header → verify_token
    Note over MW: split "{exp}.{sig}", recompute HMAC(code, exp),<br/>ct_eq(sig), exp > now
    alt valid & unexpired
        MW->>H: next.run(request)
        H-->>Browser: 200 + response
    else missing / tampered / expired / wrong-or-rotated code
        MW-->>Browser: 401 Unauthorized (handler never runs)
        Note over Browser: api.ts dispatches "mh-unauthorized" → app returns to login
    end
```

## Notes

- **Public routes:** `POST /auth/login`, `GET /auth/status`, `POST /auth/logout`, `GET /health`, and the SPA static fallback. Everything else under `/api/v1/*` is guarded.
- **Cookie:** `mh_session`, `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age=2592000` (30 days); `Secure` added only over HTTPS. The signed message is just the `exp` timestamp.
- **Fail-closed:** if HMAC key init ever fails, `sign` returns `""`, which never verifies. Both the login compare and the signature compare use `subtle::ConstantTimeEq`.
- **Stateless:** no server-side session store, so individual sessions can't be revoked — only rotating the access code (the HMAC key) invalidates all of them. CORS allows any origin but sets no `allow_credentials`, limiting cross-origin CSRF against the cookie.
- Source: `src/web/auth.rs`, `src/web/server.rs`, `src/main.rs` (`cmd_serve`), `frontend/src/components/Login.tsx`, `frontend/src/lib/api.ts`.
