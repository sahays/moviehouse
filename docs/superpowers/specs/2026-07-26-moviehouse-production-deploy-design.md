# MovieHouse Production Deployment — Design

**Date:** 2026-07-26
**Domain:** `moviehouse.niniconai.com`
**Status:** Approved decisions, pending spec review

## Goal

Package MovieHouse for production so it runs as a public, HTTPS-served web app at
`moviehouse.niniconai.com`, deployed as a **satellite of the bharatsc shared
Docker stack** — the same pattern niniconai already uses. Add `pre-deploy` and
`deploy` scripts (plus supporting build/dep-check scripts) modeled on the
bharatsc/niniconai script suite.

Primary use cases driving the "public" requirement:

- Stream to a **phone browser while travelling** (direct playback over public HTTPS).
- Open the site in an **LG TV (webOS) browser** on the same network or remotely.
- **AirPlay from an iPhone/iPad Safari** to an Apple TV (Apple TV has no browser).

## Architecture: satellite of the bharatsc shared stack

bharatsc owns the shared infrastructure:

- The central **nginx** (terminates TLS, reverse-proxies every app) — mounts
  `./nginx/conf.d:/etc/nginx/conf.d:ro` and `certbot-etc:/etc/letsencrypt:ro`.
- The **certbot** container that acquires/renews Let's Encrypt certs via Google
  Cloud DNS (DNS-01), including the **wildcard `*.niniconai.com`** cert already
  obtained by niniconai.
- The external Docker network **`shared`**.

MovieHouse joins this stack exactly like niniconai: it builds its own image,
joins `shared`, generates an nginx vhost from a template, installs it into
`${BHARATSC_DIR}/nginx/conf.d/`, and reloads bharatsc's nginx.

```
Internet ── 443 ──▶ bharatsc-nginx ──▶ moviehouse-web:9000   (web UI / API / WS / HLS)
                    (TLS, wildcard cert,
                     Basic Auth, WS + stream
                     proxy tuning)

Internet ── 6881 ─▶ moviehouse-web:6881                       (BitTorrent peers, direct)
```

## Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| Runtime | Docker on Linux, single `web` container | sled is embedded — no DB container needed |
| TLS | **Reuse niniconai's wildcard `*.niniconai.com`** | Subdomain already covered; no cert acquisition / init-ssl script |
| Storage | **Host bind mounts** for data + media | Media is large; must live on a real disk, accessible outside Docker |
| Auth | **nginx HTTP Basic Auth** (one shared credential) | App has no auth; a public torrent-adder + file browser must be gated. Browsers remember it, so it's invisible after first login and doesn't affect playback/AirPlay |
| Health probe | Existing `/api/v1/library/health` | Keeps this a zero-app-code packaging task |
| pre-deploy style | **Strict verify-only** (niniconai style) + frontend lint | Matches the reference scripts; CI-grade gate, no silent auto-fix |

## App-specific facts that shape the packaging

Discovered from the codebase (differences from niniconai):

- **Frontend is embedded in the binary** (`rust-embed`); `build.rs` runs
  `npm install && npm run build`. The runtime image needs **no** static assets
  and nginx needs **no** `/static` block.
- **Data paths derive from `$HOME`**, not env vars. The `MOVIEHOUSE_DATA_DIR` /
  `MOVIEHOUSE_TRANSCODE_DIR` entries in `.env.example` are **not read by the
  code**. Effective paths: `~/.movies/data` (sled), `~/.movies/transcoded`,
  `~/.moviehouse/dht_nodes.json`. → Set `HOME=/data` so all of them land on the
  mounted volume.
- **ffmpeg is required** at runtime for transcoding (niniconai doesn't need it).
- **BitTorrent** listens for inbound peers on `--port` (default `6881`, TCP);
  DHT is outbound from an ephemeral UDP port. nginx cannot proxy this — it needs
  a direct `6881` port mapping.
- **WebSockets** at `/api/v1/ws` (download progress) and **HLS/range streaming**
  at `/api/v1/media/{id}/stream` and `/api/v1/media/{id}/segment/{filename}`.
  The shared `proxy_params.inc` sets `Connection ""` with 30s timeouts and no
  `Upgrade` header — so these paths need **dedicated nginx locations**.
- Streaming serves **HLS** (`application/vnd.apple.mpegurl`) or **MP4 with range
  requests**; transcodes to **H.264 / HEVC** (all Apple-native). `stream_media`
  prefers the **h264** version first (best for the LG webOS browser).

## Deliverables

### 1. `Dockerfile` (multi-stage)

- **Builder:** `rust:1-bookworm` + Node 20. Cache-friendly dependency build,
  then full build; `build.rs` builds and embeds the React frontend.
- **Runtime:** `debian:bookworm-slim` + `ca-certificates`, `curl` (healthcheck),
  **`ffmpeg`**. Non-root `app` user. `ENV HOME=/data`. Copy the release binary.
- **CMD:** `moviehouse serve --bind 0.0.0.0:9000 --port 6881 --output /media/downloads`.

### 2. `docker-compose.yml` (modeled on niniconai)

```yaml
services:
  web:
    build: { context: ., dockerfile: Dockerfile }
    image: moviehouse:latest
    container_name: moviehouse-web
    restart: unless-stopped
    env_file: .env
    environment:
      HOME: /data
    volumes:
      - ${DATA_DIR}:/data       # sled db + DHT cache (host bind mount)
      - ${MEDIA_DIR}:/media     # downloads + transcoded (host bind mount)
    ports:
      - "${BT_PORT:-6881}:6881/tcp"
      - "${BT_PORT:-6881}:6881/udp"
    healthcheck:
      test: ["CMD","sh","-c","curl -sf http://localhost:${WEB_PORT:-9000}/api/v1/library/health"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 10s
    networks: [internal, shared]

networks:
  internal: { driver: bridge }
  shared:   { external: true, name: shared }
```

Notes: the healthcheck runs **inside** the container (localhost), so it bypasses
nginx Basic Auth. Inbound peer connectivity on 6881 also requires the host
firewall/router to allow it; without it, downloads still work via outbound
connections and DHT bootstrap (just fewer peers).

### 3. `nginx/conf.d/moviehouse.conf.template`

Same structure as niniconai's, adapted:

- Port-80 server: ACME `/.well-known/acme-challenge/` + 301 to HTTPS.
- Port-443 server for `moviehouse.niniconai.com`, using the **wildcard cert**:
  `ssl_certificate /etc/letsencrypt/live/niniconai.com/fullchain.pem;` (+ key,
  chain), the same TLS hardening block niniconai uses.
- **Basic Auth** applied at the server level:
  `auth_basic "MovieHouse"; auth_basic_user_file /etc/nginx/conf.d/moviehouse.htpasswd;`
- **`location = /api/v1/ws`** — WebSocket: `proxy_http_version 1.1;`
  `proxy_set_header Upgrade $http_upgrade;` `proxy_set_header Connection "upgrade";`
  long `proxy_read_timeout` (e.g. 3600s). (Uses a static `Connection "upgrade"`
  so no `map` in bharatsc's `nginx.conf` is required.)
- **`location ^~ /api/v1/media/`** — streaming: `proxy_buffering off;`
  `proxy_request_buffering off;` long timeouts; forward `Range`/`If-Range`
  (nginx forwards these by default); `proxy_set_header` host/proto as usual.
- **`location /`** — default proxy to `http://moviehouse-web:__PORT__` with
  security headers and general rate limit.
- Placeholder `__PORT__` is substituted with `WEB_PORT` by `deploy.sh` (same
  `sed` mechanism niniconai uses).
- **No `/static` block** (assets embedded). **No `/health` internal-only block**
  needed (container-internal healthcheck).

### 4. `scripts/`

Learned from bharatsc + niniconai; same flags and layout.

- **`build.sh`** — `docker build -t moviehouse:latest .` + size report.
- **`check-deps.sh`** — docker running, compose plugin, `curl`, `openssl`
  (for htpasswd generation), `.env` present, `shared` network exists,
  `${DATA_DIR}` and `${MEDIA_DIR}` exist, and **wildcard cert present** in the
  `bharatsc_certbot-etc` volume (warn + point to niniconai's `init-ssl.sh` if
  missing). Colorized pass/fail/warn, non-zero exit on critical failures.
- **`pre-deploy.sh`** — strict, verify-only (no auto-fix):
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features --locked -- -D warnings`
    (with the two documented axum `-A` allows)
  - `cargo test --locked --no-fail-fast`
  - `cargo check --locked --all-targets`
  - frontend: `(cd frontend && npx prettier --check src && npm run lint)`
  - Replaces the current auto-formatting `pre-deploy.sh`.
- **`deploy.sh`** — flags `--no-build`, `--foreground|-f`. Steps:
  1. Source `.env`; require it. Resolve `WEB_PORT`, `BHARATSC_DIR`.
  2. Run `check-deps.sh`.
  3. Verify `shared` network (error → "start bharatsc first").
  4. **Generate/refresh `moviehouse.htpasswd`** from `.env`
     (`BASIC_AUTH_USER` / `BASIC_AUTH_PASSWORD`) via `openssl passwd -apr1`
     — no `htpasswd` binary dependency.
  5. `sed s/__PORT__/$WEB_PORT/` template → `nginx/conf.d/moviehouse.conf`.
  6. Build image (unless `--no-build`).
  7. `docker compose up -d` (or foreground).
  8. Health-check the container (`/api/v1/library/health`), ~40s budget.
  9. Copy `moviehouse.conf` **and** `moviehouse.htpasswd` into
     `${BHARATSC_DIR}/nginx/conf.d/`, then `nginx -t` and `nginx -s reload` in
     the bharatsc nginx container (warn if cert test fails).

### 5. Supporting files

- **`.env.example`** — add: `WEB_PORT=9000`, `BT_PORT=6881`,
  `DATA_DIR=/srv/moviehouse/data`, `MEDIA_DIR=/srv/moviehouse/media`,
  `DOMAIN=moviehouse.niniconai.com`, `BASIC_AUTH_USER=`, `BASIC_AUTH_PASSWORD=`,
  `# BHARATSC_DIR=/path/to/bharatsc`. Keep existing `TMDB_API_KEY` /
  `TMDB_READ_ACCESS_TOKEN`. Remove the two unused `MOVIEHOUSE_*_DIR` lines (dead).
- **`.dockerignore`** — exclude `target`, `frontend/node_modules`,
  `frontend/dist`, `.git`, `docs`, and local media/data dirs.
- **`.gitignore`** — add `nginx/conf.d/moviehouse.conf` (generated) and
  `*.htpasswd`.

## Security notes

- **No app-level auth by design (this iteration).** nginx Basic Auth is the only
  gate. The credential lives in `.env` (git-ignored) and is materialized into an
  `htpasswd` file (git-ignored). Anyone with the credential has full control of
  the torrent engine, file browser, and library — treat it as an admin password.
- Basic Auth covers the **entire vhost including the API and WS**, so the torrent
  adder and filesystem browser are not reachable unauthenticated.
- The BitTorrent port (6881) is intentionally unauthenticated (protocol-level) —
  that is normal and carries no web-app authority.
- HSTS/security headers come from bharatsc's shared `security_headers.inc`.

## Out of scope

- No `init-ssl.sh` (wildcard cert already covers the subdomain).
- No application-level / Google OAuth (explicitly deferred — would be a separate
  spec: oauth2 crate, sled sessions, route guards, login UI).
- No database container (sled embedded).
- No CI pipeline.
- The existing `install.sh` (local/dev background run) stays as-is.

## Open items for reviewer

- Confirm host paths `DATA_DIR=/srv/moviehouse/data` and
  `MEDIA_DIR=/srv/moviehouse/media` (or supply your preferred paths / mounted
  media disk).
- Confirm the download output path `/media/downloads` (inside `${MEDIA_DIR}`).
