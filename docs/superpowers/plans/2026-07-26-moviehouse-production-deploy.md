# MovieHouse Production Deployment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Package MovieHouse as a Dockerized satellite of the bharatsc shared stack, served over HTTPS with Basic Auth at `moviehouse.niniconai.com`, with `pre-deploy`/`deploy` scripts.

**Architecture:** MovieHouse builds its own image and runs a single `web` container on the external `shared` Docker network. bharatsc's central nginx (the only process binding host 80/443, holder of the wildcard `*.niniconai.com` cert) reverse-proxies to it. Deploy generates an nginx vhost from a template and installs it into `${BHARATSC_DIR}/nginx/conf.d/` plus an `htpasswd` file, then reloads bharatsc's nginx.

**Tech Stack:** Rust (axum, sled, rust-embed), React (embedded via `build.rs`), Docker + Compose, nginx (bharatsc), ffmpeg, bash.

## Global Constraints

- Target domain: `moviehouse.niniconai.com` (subdomain covered by niniconai's wildcard `*.niniconai.com` cert — do NOT acquire a new cert).
- The single reverse proxy is **bharatsc's** nginx. MovieHouse contributes a vhost `.conf` + `.htpasswd` into `${BHARATSC_DIR}/nginx/conf.d/`; it runs **no** nginx of its own.
- External Docker network name: `shared` (created by bharatsc; must already exist).
- Container internal web port: driven by `WEB_PORT` (default `9000`). BitTorrent peer port: `6881` internal, host-mapped via `BT_PORT` (default `6881`).
- Data paths derive from `$HOME` in the app, so the container sets `HOME=/data`. `MOVIEHOUSE_DATA_DIR`/`MOVIEHOUSE_TRANSCODE_DIR` are NOT read by the code — do not rely on them.
- Storage: host bind mounts — `${DATA_DIR}:/data` (sled + DHT) and `${MEDIA_DIR}:/media` (downloads + transcoded). Defaults: `DATA_DIR=/srv/moviehouse/data`, `MEDIA_DIR=/srv/moviehouse/media`.
- Auth: nginx HTTP Basic Auth over the whole vhost. Credentials from `.env` (`BASIC_AUTH_USER`, `BASIC_AUTH_PASSWORD`); never commit the generated htpasswd.
- Health probe: `GET /api/v1/library/health` (no app code change).
- nginx shared building blocks available in bharatsc's live config: `limit_req` zones `general` and `auth`; includes `proxy_params.inc` and `security_headers.inc`. `proxy_params.inc` sets `Connection ""` with 30s timeouts and no `Upgrade` header — so WS and streaming need dedicated locations.

**Environment note for the executor:** Steps marked **[local]** can be validated on any machine with Docker. Steps marked **[server]** require the production host where the bharatsc stack and `shared` network exist. If Docker is unavailable locally, still create files and run the non-Docker validations (`bash -n`, `shellcheck`), and defer `[server]`/`docker build` steps to the host.

---

### Task 1: Supporting config files

**Files:**
- Create: `.dockerignore`
- Modify: `.gitignore`
- Modify: `.env.example`

**Interfaces:**
- Produces: env var names consumed by later tasks — `WEB_PORT`, `BT_PORT`, `DATA_DIR`, `MEDIA_DIR`, `DOMAIN`, `BASIC_AUTH_USER`, `BASIC_AUTH_PASSWORD`, `BHARATSC_DIR`.

- [ ] **Step 1: Create `.dockerignore`**

```
target
frontend/node_modules
frontend/dist
.git
.gitignore
docs
tests
install.sh
build_number
*.log
.env
.DS_Store
```

- [ ] **Step 2: Append generated artifacts to `.gitignore`**

Add these lines to the existing `.gitignore`:

```
nginx/conf.d/moviehouse.conf
*.htpasswd
```

- [ ] **Step 3: Rewrite `.env.example`**

Replace the entire contents of `.env.example` with:

```
# ── TMDB (movie/show metadata) ────────────────────────────────────
TMDB_API_KEY=your_tmdb_api_key_here
TMDB_READ_ACCESS_TOKEN=your_tmdb_read_access_token_here

# ── Deploy: domain & ports ────────────────────────────────────────
DOMAIN=moviehouse.niniconai.com
WEB_PORT=9000
BT_PORT=6881

# ── Deploy: host storage (bind mounts) ────────────────────────────
DATA_DIR=/srv/moviehouse/data
MEDIA_DIR=/srv/moviehouse/media

# ── Deploy: Basic Auth (nginx) ────────────────────────────────────
# Anyone with these credentials has full control of the torrent engine,
# file browser, and library. Treat as an admin password.
BASIC_AUTH_USER=admin
BASIC_AUTH_PASSWORD=change_me

# ── Deploy: path to the bharatsc project (default: sibling dir) ────
# BHARATSC_DIR=/path/to/bharatsc
```

- [ ] **Step 4: Verify `.env.example` parses as shell assignments** **[local]**

Run: `bash -n <(sed 's/#.*//' .env.example) && echo OK`
Expected: `OK` (no syntax errors)

- [ ] **Step 5: Commit**

```bash
git add .dockerignore .gitignore .env.example
git commit -m "chore(deploy): add dockerignore and deploy env vars"
```

---

### Task 2: Dockerfile (multi-stage image)

**Files:**
- Create: `Dockerfile`

**Interfaces:**
- Consumes: full source tree including `frontend/` (needed by `build.rs`).
- Produces: image `moviehouse:latest` with entrypoint `moviehouse serve --bind 0.0.0.0:${WEB_PORT} --port 6881 --output /media/downloads`, runtime user `app`, `HOME=/data`, ffmpeg installed.

- [ ] **Step 1: Create `Dockerfile`**

```dockerfile
# syntax=docker/dockerfile:1

# ── Stage 1: build ────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

# Node is required at build time: build.rs runs `npm install && npm run build`
# in frontend/ and rust-embed bakes the compiled SPA into the binary.
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && \
    apt-get install -y --no-install-recommends nodejs && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Full source is needed because build.rs compiles the frontend during the build.
COPY . .

# Release build (build.rs builds the frontend and bumps the patch version in an
# ephemeral copy of Cargo.toml — harmless in the throwaway build stage).
RUN cargo build --release

# ── Stage 2: runtime ──────────────────────────────────────────────
FROM debian:bookworm-slim

# ffmpeg for transcoding; curl for the container healthcheck.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        ffmpeg && \
    rm -rf /var/lib/apt/lists/*

# Unprivileged runtime user. HOME=/data so the app's $HOME-derived paths
# (~/.movies/data, ~/.movies/transcoded, ~/.moviehouse/dht_nodes.json) land on
# the mounted volume. /media/downloads is the default download output.
RUN groupadd --system app && \
    useradd --system --gid app --home-dir /data app && \
    mkdir -p /data /media/downloads && \
    chown -R app:app /data /media

COPY --from=builder /app/target/release/moviehouse /app/bin/moviehouse

ENV HOME=/data
WORKDIR /app
USER app

EXPOSE 9000 6881

# Shell form so ${WEB_PORT} is expanded from the container environment
# (provided by env_file in docker-compose). exec keeps the binary as PID 1.
CMD ["sh", "-c", "exec /app/bin/moviehouse serve --bind 0.0.0.0:${WEB_PORT:-9000} --port 6881 --output /media/downloads"]
```

- [ ] **Step 2: Build the image** **[local]**

Run: `docker build -t moviehouse:latest .`
Expected: build completes; final line shows the image built. (First build is slow — Rust + frontend.)

- [ ] **Step 3: Verify ffmpeg and the binary are present in the runtime image** **[local]**

Run: `docker run --rm --entrypoint sh moviehouse:latest -c "ffmpeg -version | head -1 && /app/bin/moviehouse --version"`
Expected: an `ffmpeg version ...` line followed by a `moviehouse 3.x.x` version line.

- [ ] **Step 4: Verify it runs as the unprivileged user with HOME=/data** **[local]**

Run: `docker run --rm --entrypoint sh moviehouse:latest -c "id -un && echo \$HOME"`
Expected:
```
app
/data
```

- [ ] **Step 5: Commit**

```bash
git add Dockerfile
git commit -m "feat(deploy): add multi-stage Dockerfile with ffmpeg runtime"
```

---

### Task 3: docker-compose.yml

**Files:**
- Create: `docker-compose.yml`

**Interfaces:**
- Consumes: image from Task 2; env vars from Task 1 (`WEB_PORT`, `BT_PORT`, `DATA_DIR`, `MEDIA_DIR`).
- Produces: service `web` → container `moviehouse-web` reachable at `moviehouse-web:${WEB_PORT}` on the `shared` network; healthcheck on `/api/v1/library/health`.

- [ ] **Step 1: Create `docker-compose.yml`**

```yaml
services:
  # ── MovieHouse web server (axum HTTP + WS + embedded SPA) ───────
  # Joins the shared network so bharatsc's nginx can proxy the vhost.
  web:
    build:
      context: .
      dockerfile: Dockerfile
    image: moviehouse:latest
    container_name: moviehouse-web
    restart: unless-stopped
    env_file: .env
    environment:
      HOME: /data
    volumes:
      - ${DATA_DIR:-/srv/moviehouse/data}:/data      # sled db + DHT cache
      - ${MEDIA_DIR:-/srv/moviehouse/media}:/media   # downloads + transcoded
    ports:
      # BitTorrent peer/DHT port — nginx cannot proxy this; map it directly.
      - "${BT_PORT:-6881}:6881/tcp"
      - "${BT_PORT:-6881}:6881/udp"
    healthcheck:
      # Runs inside the container (localhost), so it bypasses nginx Basic Auth.
      test: ["CMD", "sh", "-c", "curl -sf http://localhost:$${WEB_PORT:-9000}/api/v1/library/health"]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 15s
    networks:
      - internal
      - shared

networks:
  internal:
    driver: bridge
  shared:
    external: true
    name: shared

# No named volumes: media and data live on host bind mounts (see volumes above).
```

- [ ] **Step 2: Validate compose config resolves** **[local]**

Run: `DATA_DIR=/tmp/mh-data MEDIA_DIR=/tmp/mh-media WEB_PORT=9000 BT_PORT=6881 docker compose config >/dev/null && echo OK`
Expected: `OK` (interpolation succeeds, YAML valid). Note: this only validates syntax/interpolation; the `shared` network is checked at `up` time (Task 7).

- [ ] **Step 3: Commit**

```bash
git add docker-compose.yml
git commit -m "feat(deploy): add docker-compose with bind mounts and healthcheck"
```

---

### Task 4: nginx vhost template

**Files:**
- Create: `nginx/conf.d/moviehouse.conf.template`

**Interfaces:**
- Consumes: `__PORT__` placeholder (substituted with `WEB_PORT` by `deploy.sh`); bharatsc-provided `limit_req` zone `general`, includes `proxy_params.inc` + `security_headers.inc`, wildcard cert at `/etc/letsencrypt/live/niniconai.com/`, and `/etc/nginx/conf.d/moviehouse.htpasswd` (installed by `deploy.sh`).
- Produces: `moviehouse.conf` routing `moviehouse.niniconai.com` → `moviehouse-web:__PORT__`.

- [ ] **Step 1: Create `nginx/conf.d/moviehouse.conf.template`**

```nginx
# ── moviehouse.niniconai.com ──────────────────────────────────────
# Generated from moviehouse.conf.template by scripts/deploy.sh, then
# installed into bharatsc/nginx/conf.d/ and loaded by bharatsc's nginx.
# __PORT__ is replaced with WEB_PORT at deploy time.

# ── HTTP → HTTPS redirect ─────────────────────────────────────────
server {
    listen 80;
    listen [::]:80;
    server_name moviehouse.niniconai.com;

    location /.well-known/acme-challenge/ {
        root /var/www/certbot;
    }

    location / {
        return 301 https://$host$request_uri;
    }
}

# ── HTTPS ─────────────────────────────────────────────────────────
server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;
    server_name moviehouse.niniconai.com;

    # Covered by niniconai's wildcard *.niniconai.com certificate.
    ssl_certificate     /etc/letsencrypt/live/niniconai.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/niniconai.com/privkey.pem;
    ssl_trusted_certificate /etc/letsencrypt/live/niniconai.com/chain.pem;

    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:DHE-RSA-AES128-GCM-SHA256:DHE-RSA-AES256-GCM-SHA384;
    ssl_prefer_server_ciphers off;

    ssl_session_cache shared:SSL:10m;
    ssl_session_timeout 1d;
    ssl_session_tickets off;

    ssl_stapling on;
    ssl_stapling_verify on;
    resolver 1.1.1.1 8.8.8.8 valid=300s;
    resolver_timeout 5s;

    include /etc/nginx/conf.d/security_headers.inc;

    # Large enough for .torrent uploads; streamed responses are not buffered.
    client_max_body_size 50m;

    # ── Basic Auth — gates the whole app (torrent adder, file browser) ──
    auth_basic "MovieHouse";
    auth_basic_user_file /etc/nginx/conf.d/moviehouse.htpasswd;

    # ── WebSocket — download-progress stream ──────────────────────
    location = /api/v1/ws {
        proxy_pass http://moviehouse-web:__PORT__;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }

    # ── Media streaming — HLS playlists, segments, MP4 range ──────
    location ^~ /api/v1/media/ {
        proxy_pass http://moviehouse-web:__PORT__;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_buffering off;
        proxy_request_buffering off;
        proxy_read_timeout 3600s;
    }

    # ── Everything else — web UI + REST API ───────────────────────
    location / {
        limit_req zone=general burst=20 nodelay;
        include /etc/nginx/conf.d/proxy_params.inc;
        include /etc/nginx/conf.d/security_headers.inc;
        proxy_pass http://moviehouse-web:__PORT__;
    }
}
```

- [ ] **Step 2: Verify the rendered config passes `nginx -t`** **[local]**

This renders the template and tests it in a throwaway nginx container, stubbing the includes/certs/htpasswd/upstream that only exist in the live bharatsc stack:

```bash
tmp="$(mktemp -d)"
mkdir -p "$tmp/conf.d" "$tmp/certs/live/niniconai.com" "$tmp/www/certbot"
sed 's/__PORT__/9000/g' nginx/conf.d/moviehouse.conf.template > "$tmp/conf.d/moviehouse.conf"
# Stub includes and a self-signed cert so `nginx -t` can parse the vhost.
: > "$tmp/conf.d/security_headers.inc"
printf 'proxy_set_header Host $host;\n' > "$tmp/conf.d/proxy_params.inc"
printf 'admin:$apr1$abc$def\n' > "$tmp/conf.d/moviehouse.htpasswd"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
  -keyout "$tmp/certs/live/niniconai.com/privkey.pem" \
  -out "$tmp/certs/live/niniconai.com/fullchain.pem" -subj "/CN=test" 2>/dev/null
cp "$tmp/certs/live/niniconai.com/fullchain.pem" "$tmp/certs/live/niniconai.com/chain.pem"
docker run --rm \
  -v "$tmp/conf.d:/etc/nginx/conf.d:ro" \
  -v "$tmp/certs:/etc/letsencrypt:ro" \
  -v "$tmp/www:/var/www/certbot:ro" \
  nginx:1.25-alpine sh -c '
    printf "events{}\nhttp{\n  limit_req_zone \$binary_remote_addr zone=general:10m rate=10r/s;\n  include /etc/nginx/conf.d/*.conf;\n}\n" > /etc/nginx/nginx.conf
    nginx -t'
rm -rf "$tmp"
```

Expected: `nginx: configuration file /etc/nginx/nginx.conf test is successful`

- [ ] **Step 3: Commit**

```bash
git add nginx/conf.d/moviehouse.conf.template
git commit -m "feat(deploy): add nginx vhost template with WS + streaming + basic auth"
```

---

### Task 5: build.sh and check-deps.sh

**Files:**
- Create: `scripts/build.sh`
- Create: `scripts/check-deps.sh`

**Interfaces:**
- Produces: `scripts/build.sh` (builds `moviehouse:latest`); `scripts/check-deps.sh` (exits non-zero on any critical failure). `deploy.sh` (Task 7) calls `check-deps.sh`.

- [ ] **Step 1: Create `scripts/build.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

IMAGE_NAME="moviehouse"
IMAGE_TAG="${1:-latest}"

echo "Building Docker image: ${IMAGE_NAME}:${IMAGE_TAG}"
docker build -t "${IMAGE_NAME}:${IMAGE_TAG}" "$PROJECT_ROOT"

echo ""
echo "Build complete."
docker images "${IMAGE_NAME}:${IMAGE_TAG}" --format "  Image: {{.Repository}}:{{.Tag}}  Size: {{.Size}}"
```

- [ ] **Step 2: Create `scripts/check-deps.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; NC='\033[0m'
pass() { echo -e "  ${GREEN}[OK]${NC} $1"; }
fail() { echo -e "  ${RED}[FAIL]${NC} $1"; }
warn() { echo -e "  ${YELLOW}[WARN]${NC} $1"; }

ERRORS=0

echo "Pre-flight dependency checks"
echo "============================="
echo ""

# Load .env for DATA_DIR / MEDIA_DIR / DOMAIN.
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    set -a; # shellcheck source=/dev/null
    source "$PROJECT_ROOT/.env"; set +a
fi

echo "Docker:"
if command -v docker &>/dev/null && docker info &>/dev/null; then
    pass "Docker is installed and running"
else
    fail "Docker is not installed or the daemon is not running"; ERRORS=$((ERRORS + 1))
fi

echo "Docker Compose:"
if docker compose version &>/dev/null; then
    pass "Docker Compose plugin available"
else
    fail "Docker Compose plugin not found (need 'docker compose')"; ERRORS=$((ERRORS + 1))
fi

echo "curl / openssl:"
command -v curl    &>/dev/null && pass "curl available"    || { fail "curl not found";    ERRORS=$((ERRORS + 1)); }
command -v openssl &>/dev/null && pass "openssl available" || { fail "openssl not found"; ERRORS=$((ERRORS + 1)); }

echo "Environment:"
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    pass ".env file exists"
else
    fail ".env not found — copy .env.example to .env"; ERRORS=$((ERRORS + 1))
fi

echo "Storage (host bind mounts):"
for d in "${DATA_DIR:-/srv/moviehouse/data}" "${MEDIA_DIR:-/srv/moviehouse/media}"; do
    if [[ -d "$d" ]]; then
        pass "$d exists"
    else
        warn "$d does not exist — create it: sudo mkdir -p $d"
    fi
done

echo "Shared network:"
if docker network inspect shared &>/dev/null; then
    pass "shared network exists"
else
    fail "shared network not found — start bharatsc first"; ERRORS=$((ERRORS + 1))
fi

echo "TLS (wildcard cert):"
if docker run --rm -v bharatsc_certbot-etc:/etc/letsencrypt:ro debian:bookworm-slim \
        test -f /etc/letsencrypt/live/niniconai.com/fullchain.pem 2>/dev/null; then
    pass "wildcard *.niniconai.com cert present"
else
    warn "niniconai.com cert not found in bharatsc_certbot-etc — run niniconai's scripts/init-ssl.sh first"
fi

echo ""
if [[ $ERRORS -gt 0 ]]; then
    echo -e "${RED}$ERRORS critical check(s) failed. Fix before deploying.${NC}"; exit 1
else
    echo -e "${GREEN}All checks passed.${NC}"
fi
```

- [ ] **Step 3: Make executable and syntax-check** **[local]**

```bash
chmod +x scripts/build.sh scripts/check-deps.sh
bash -n scripts/build.sh && bash -n scripts/check-deps.sh && echo "syntax OK"
```
Expected: `syntax OK`

- [ ] **Step 4: Lint with shellcheck if available** **[local]**

Run: `command -v shellcheck >/dev/null && shellcheck scripts/build.sh scripts/check-deps.sh || echo "shellcheck not installed — skipping"`
Expected: no warnings, or the skip message.

- [ ] **Step 5: Commit**

```bash
git add scripts/build.sh scripts/check-deps.sh
git commit -m "feat(deploy): add build.sh and check-deps.sh"
```

---

### Task 6: pre-deploy.sh (strict, replaces existing)

**Files:**
- Modify (replace contents): `pre-deploy.sh`

**Interfaces:**
- Produces: a strict verify-only gate. Exits non-zero if the tree isn't formatted, clippy warns, tests fail, or the frontend lint/format check fails.

- [ ] **Step 1: Replace `pre-deploy.sh` contents**

```bash
#!/usr/bin/env bash
# Pre-deploy gate. Strict verify-only: nothing is auto-fixed. Run your
# formatters during development; this only confirms the tree is already clean.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$REPO_DIR"

echo "Running pre-deploy checks (strict)..."

# ── 1. Rust format — verify only ────────────────────────────────
echo "  Checking Rust formatting (cargo fmt --check)..."
cargo fmt --all -- --check

# ── 2. Rust lint — strict ───────────────────────────────────────
echo "  Linting Rust (cargo clippy — strict)..."
cargo clippy --all-targets --all-features --locked -- -D warnings

# ── 3. Rust tests ───────────────────────────────────────────────
echo "  Running unit tests (cargo test)..."
cargo test --locked --no-fail-fast

# ── 4. Build check ──────────────────────────────────────────────
echo "  Build check (cargo check --locked)..."
cargo check --locked --all-targets

# ── 5. Frontend — format + lint (verify only) ───────────────────
echo "  Checking React formatting (prettier --check)..."
(cd frontend && npx prettier --check src)

echo "  Linting React (eslint)..."
(cd frontend && npm run lint)

echo "All checks passed."
```

- [ ] **Step 2: Make executable and syntax-check** **[local]**

```bash
chmod +x pre-deploy.sh
bash -n pre-deploy.sh && echo "syntax OK"
```
Expected: `syntax OK`

- [ ] **Step 3: Run the gate** **[local, needs Rust + Node toolchain]**

Run: `./pre-deploy.sh`
Expected: ends with `All checks passed.` If clippy/fmt/tests fail, that is a real finding to fix in the app before deploying — not a plan defect. If the toolchain isn't present on this machine, defer this step to a machine that has it.

- [ ] **Step 4: Commit**

```bash
git add pre-deploy.sh
git commit -m "refactor(deploy): make pre-deploy strict verify-only + frontend lint"
```

---

### Task 7: deploy.sh (capstone)

**Files:**
- Create: `scripts/deploy.sh`

**Interfaces:**
- Consumes: `.env`; `scripts/check-deps.sh`; `docker-compose.yml`; `nginx/conf.d/moviehouse.conf.template`; `${BHARATSC_DIR}/nginx/conf.d/` (writable host dir mounted into bharatsc nginx); the running `bharatsc-nginx` container.
- Produces: a running `moviehouse-web` container and an installed, reloaded nginx vhost + htpasswd.

- [ ] **Step 1: Create `scripts/deploy.sh`**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

SKIP_BUILD=false
FOREGROUND=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) SKIP_BUILD=true; shift ;;
        --foreground|-f) FOREGROUND=true; shift ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

cd "$PROJECT_ROOT"

# ── 1. Source .env ────────────────────────────────────────────────
if [[ -f ".env" ]]; then
    echo "Sourcing .env"
    set -a; # shellcheck source=/dev/null
    source .env; set +a
else
    echo "ERROR: .env not found. Copy .env.example to .env and configure it."
    exit 1
fi

WEB_PORT="${WEB_PORT:-9000}"
DOMAIN="${DOMAIN:-moviehouse.niniconai.com}"
BHARATSC_DIR="${BHARATSC_DIR:-$(dirname "$PROJECT_ROOT")/bharatsc}"

# ── 2. Pre-flight checks ─────────────────────────────────────────
echo ""
"$SCRIPT_DIR/check-deps.sh"
echo ""

# ── 3. Verify shared network ─────────────────────────────────────
if ! docker network inspect shared &>/dev/null; then
    echo "ERROR: shared network not found."
    echo "BharatSC must be running first: cd $BHARATSC_DIR && ./scripts/deploy.sh"
    exit 1
fi
echo "shared network found."

# ── 4. Validate Basic Auth credentials ───────────────────────────
if [[ -z "${BASIC_AUTH_USER:-}" || -z "${BASIC_AUTH_PASSWORD:-}" || "${BASIC_AUTH_PASSWORD}" == "change_me" ]]; then
    echo "ERROR: BASIC_AUTH_USER / BASIC_AUTH_PASSWORD must be set to non-default values in .env."
    exit 1
fi

# ── 5. Generate htpasswd (apr1 via openssl — no htpasswd binary) ──
HTPASSWD_FILE="$PROJECT_ROOT/nginx/conf.d/moviehouse.htpasswd"
mkdir -p "$PROJECT_ROOT/nginx/conf.d"
printf '%s:%s\n' "$BASIC_AUTH_USER" "$(openssl passwd -apr1 "$BASIC_AUTH_PASSWORD")" > "$HTPASSWD_FILE"
echo "Generated htpasswd for user '$BASIC_AUTH_USER'."

# ── 6. Render nginx vhost from template ──────────────────────────
TEMPLATE="$PROJECT_ROOT/nginx/conf.d/moviehouse.conf.template"
GENERATED="$PROJECT_ROOT/nginx/conf.d/moviehouse.conf"
echo "Generating nginx config (WEB_PORT=$WEB_PORT)..."
sed "s/__PORT__/$WEB_PORT/g" "$TEMPLATE" > "$GENERATED"

# ── 7. Build image ───────────────────────────────────────────────
if [[ "$SKIP_BUILD" == false ]]; then
    echo "Building web image..."
    docker compose build web
    echo ""
fi

# ── 8. Start web service ─────────────────────────────────────────
if [[ "$FOREGROUND" == true ]]; then
    echo "Starting moviehouse-web (foreground)..."
    docker compose up
    exit 0
fi

echo "Starting moviehouse-web..."
docker compose up -d

# ── 9. Health check ──────────────────────────────────────────────
echo ""
echo "Waiting for health check..."
HEALTHY=false
for _ in $(seq 1 20); do
    if docker compose exec -T web curl -sf "http://localhost:${WEB_PORT}/api/v1/library/health" >/dev/null 2>&1; then
        echo "moviehouse-web is healthy."
        HEALTHY=true
        break
    fi
    sleep 2
done
if [[ "$HEALTHY" == false ]]; then
    echo "WARNING: Health check did not pass within 40s."
    echo "Check logs: docker compose logs web"
    docker compose ps
    exit 1
fi

# ── 10. Install nginx vhost + htpasswd into bharatsc, reload ──────
DEST="$BHARATSC_DIR/nginx/conf.d"
if [[ ! -d "$DEST" ]]; then
    echo "WARNING: bharatsc nginx/conf.d not found at $DEST"
    echo "Set BHARATSC_DIR to the bharatsc project root, then re-run."
    docker compose ps
    exit 1
fi

echo ""
echo "Installing nginx vhost + htpasswd into bharatsc..."
cp "$GENERATED" "$DEST/moviehouse.conf"
cp "$HTPASSWD_FILE" "$DEST/moviehouse.htpasswd"

if docker compose -f "$BHARATSC_DIR/docker-compose.yml" exec -T nginx nginx -t 2>/dev/null; then
    docker compose -f "$BHARATSC_DIR/docker-compose.yml" exec -T nginx nginx -s reload
    echo "Nginx config installed and reloaded."
else
    echo "WARNING: nginx config test failed. Check that the wildcard cert for"
    echo "niniconai.com exists (niniconai's scripts/init-ssl.sh) and review:"
    echo "  docker compose -f $BHARATSC_DIR/docker-compose.yml exec nginx nginx -t"
    exit 1
fi

echo ""
docker compose ps
echo ""
echo "Deployed: https://$DOMAIN"
```

- [ ] **Step 2: Make executable and syntax-check** **[local]**

```bash
chmod +x scripts/deploy.sh
bash -n scripts/deploy.sh && echo "syntax OK"
```
Expected: `syntax OK`

- [ ] **Step 3: shellcheck if available** **[local]**

Run: `command -v shellcheck >/dev/null && shellcheck scripts/deploy.sh || echo "shellcheck not installed — skipping"`
Expected: no warnings, or the skip message.

- [ ] **Step 4: Commit**

```bash
git add scripts/deploy.sh
git commit -m "feat(deploy): add deploy.sh (build, healthcheck, nginx install + reload)"
```

---

### Task 8: End-to-end deploy + docs

**Files:**
- Modify: `README.md` (add a Production Deployment section)

**Interfaces:**
- Consumes: everything from Tasks 1–7.

- [ ] **Step 1: Prepare the server** **[server]**

On the production host, with the bharatsc stack already running (so `shared` and the wildcard cert exist):

```bash
sudo mkdir -p /srv/moviehouse/data /srv/moviehouse/media
cp .env.example .env
# Edit .env: set TMDB keys, BASIC_AUTH_USER/PASSWORD, and confirm DATA_DIR/MEDIA_DIR.
```

- [ ] **Step 2: Run the pre-deploy gate** **[server or dev]**

Run: `./pre-deploy.sh`
Expected: `All checks passed.`

- [ ] **Step 3: Deploy** **[server]**

Run: `./scripts/deploy.sh`
Expected: ends with `moviehouse-web is healthy.`, `Nginx config installed and reloaded.`, and `Deployed: https://moviehouse.niniconai.com`.

- [ ] **Step 4: Verify HTTPS + Basic Auth from outside** **[server]**

```bash
# Unauthenticated request should be rejected by nginx:
curl -s -o /dev/null -w "%{http_code}\n" https://moviehouse.niniconai.com/
# Expected: 401

# Authenticated request should reach the app:
curl -s -o /dev/null -w "%{http_code}\n" -u "$BASIC_AUTH_USER:$BASIC_AUTH_PASSWORD" \
  https://moviehouse.niniconai.com/api/v1/library/health
# Expected: 200
```

- [ ] **Step 5: Verify the SPA loads and WS upgrades** **[server]**

```bash
# SPA index (200, HTML):
curl -s -u "$BASIC_AUTH_USER:$BASIC_AUTH_PASSWORD" https://moviehouse.niniconai.com/ | head -c 100
# WebSocket upgrade handshake returns 101:
curl -s -o /dev/null -w "%{http_code}\n" -u "$BASIC_AUTH_USER:$BASIC_AUTH_PASSWORD" \
  -H "Connection: Upgrade" -H "Upgrade: websocket" \
  -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
  https://moviehouse.niniconai.com/api/v1/ws
# Expected: 101
```

- [ ] **Step 6: Add a Production Deployment section to `README.md`**

Insert after the existing "Quick Start" section:

```markdown
## Production Deployment

MovieHouse deploys as a satellite of the bharatsc shared Docker stack, served at
`https://moviehouse.niniconai.com` behind bharatsc's nginx (wildcard TLS + Basic Auth).

### Prerequisites
- The bharatsc stack running (provides the `shared` network, nginx, and the
  `*.niniconai.com` wildcard cert via niniconai's `scripts/init-ssl.sh`).
- Host directories for storage (defaults): `/srv/moviehouse/data`, `/srv/moviehouse/media`.

### Steps
```bash
cp .env.example .env      # set TMDB keys + BASIC_AUTH_USER/PASSWORD
./pre-deploy.sh           # strict fmt/clippy/test/lint gate
./scripts/deploy.sh       # build image, start container, install nginx vhost, reload
```

Flags: `./scripts/deploy.sh --no-build` (skip image build), `--foreground` (run attached).

### Viewing
- **Phone (travelling):** open the site in the browser; HLS/H.264 play directly.
- **Apple TV:** AirPlay from an iPhone/iPad Safari session (tvOS has no browser).
- **LG TV (webOS) browser:** open the site; use the **H.264** transcode (HLS/HEVC are unreliable in that browser).

Ports: 443 (via bharatsc nginx) for the web UI; `6881/tcp+udp` mapped directly for BitTorrent peers.
```

- [ ] **Step 7: Commit**

```bash
git add README.md
git commit -m "docs: add production deployment section"
```

- [ ] **Step 8: Final smoke test in a real client** **[server]**

Open `https://moviehouse.niniconai.com` in a browser, authenticate with the Basic Auth credentials, confirm the library loads, add a small torrent, and confirm progress updates stream over the WebSocket. Then play a transcoded title and confirm it streams (seek works).

---

## Self-Review

**Spec coverage:**
- Satellite pattern / install into bharatsc nginx → Tasks 4, 7. ✓
- Reuse wildcard cert (no init-ssl) → Task 4 (cert path), Task 5 (cert presence warn). ✓
- Host bind-mount storage → Tasks 1, 3. ✓
- nginx Basic Auth → Tasks 4 (vhost), 7 (htpasswd gen + install), 8 (401/200 verify). ✓
- ffmpeg in runtime → Task 2. ✓
- HOME=/data for data paths → Task 2. ✓
- Direct 6881 peer port → Task 3. ✓
- WS + streaming dedicated locations → Task 4 (+ Task 8 WS 101 check). ✓
- Health probe `/api/v1/library/health` → Tasks 3, 7, 8. ✓
- Strict verify-only pre-deploy + frontend lint → Task 6. ✓
- build/check-deps/deploy scripts → Tasks 5, 7. ✓
- `.env.example` / `.dockerignore` / `.gitignore` → Task 1. ✓
- Out of scope (no init-ssl, no OAuth, no DB, keep install.sh) → honored (not present). ✓

**Placeholder scan:** No TBD/TODO; every file's full contents and every verification command are inline. `__PORT__` is an intentional template token, substituted in Task 7 Step 6 and Task 4 Step 2.

**Type/name consistency:** Env var names (`WEB_PORT`, `BT_PORT`, `DATA_DIR`, `MEDIA_DIR`, `BASIC_AUTH_USER`, `BASIC_AUTH_PASSWORD`, `BHARATSC_DIR`, `DOMAIN`) are identical across `.env.example` (Task 1), compose (Task 3), and scripts (Tasks 5, 7). Container name `moviehouse-web`, image `moviehouse:latest`, upstream `moviehouse-web:__PORT__`, health path `/api/v1/library/health`, htpasswd path `/etc/nginx/conf.d/moviehouse.htpasswd`, and cert path `/etc/letsencrypt/live/niniconai.com/` are consistent across Tasks 2, 3, 4, 7.
