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

# ── 4. Validate access code ───────────────────────────────────────
if [[ -z "${MOVIEHOUSE_ACCESS_CODE:-}" || "${MOVIEHOUSE_ACCESS_CODE}" == "change_me" ]]; then
    echo "ERROR: MOVIEHOUSE_ACCESS_CODE must be set to a non-default value in .env."
    echo "Generate one with: openssl rand -hex 24"
    exit 1
fi

# ── 5. Render nginx vhost from template ───────────────────────────
TEMPLATE="$PROJECT_ROOT/nginx/conf.d/moviehouse.conf.template"
GENERATED="$PROJECT_ROOT/nginx/conf.d/moviehouse.conf"
mkdir -p "$PROJECT_ROOT/nginx/conf.d"
echo "Generating nginx config (WEB_PORT=$WEB_PORT)..."
sed "s/__PORT__/$WEB_PORT/g" "$TEMPLATE" > "$GENERATED"

# ── 6. Build image ────────────────────────────────────────────────
if [[ "$SKIP_BUILD" == false ]]; then
    echo "Building web image..."
    docker compose build web
    echo ""
fi

# ── 7. Start web service ──────────────────────────────────────────
if [[ "$FOREGROUND" == true ]]; then
    echo "Starting moviehouse-web (foreground)..."
    docker compose up
    exit 0
fi

echo "Starting moviehouse-web..."
docker compose up -d

# ── 8. Health check ───────────────────────────────────────────────
echo ""
echo "Waiting for health check..."
HEALTHY=false
for _ in $(seq 1 20); do
    if docker compose exec -T web curl -sf "http://localhost:${WEB_PORT}/health" >/dev/null 2>&1; then
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

# ── 9. Install nginx vhost into bharatsc, reload ──────────────────
DEST="$BHARATSC_DIR/nginx/conf.d"
if [[ ! -d "$DEST" ]]; then
    echo "WARNING: bharatsc nginx/conf.d not found at $DEST"
    echo "Set BHARATSC_DIR to the bharatsc project root, then re-run."
    docker compose ps
    exit 1
fi

echo ""
echo "Installing nginx vhost into bharatsc..."

# bharatsc's nginx is shared infrastructure: it also fronts bharatsc.com and
# niniconai.com. A vhost that fails `nginx -t` does not disturb the running
# process (it keeps its in-memory config), but left on disk it makes the NEXT
# `nginx -s reload`, container restart, or host reboot fail to start — taking
# every site down. So snapshot what was there and roll back if the test fails.
INSTALLED="$DEST/moviehouse.conf"
BACKUP=""
if [[ -f "$INSTALLED" ]]; then
    BACKUP="$(mktemp)"
    cp "$INSTALLED" "$BACKUP"
fi

restore_vhost() {
    if [[ -n "$BACKUP" ]]; then
        cp "$BACKUP" "$INSTALLED"
        echo "Rolled back $INSTALLED to its previous contents."
    else
        rm -f "$INSTALLED"
        echo "Removed $INSTALLED (it was not present before this run)."
    fi
}

cp "$GENERATED" "$INSTALLED"

if ! docker compose -f "$BHARATSC_DIR/docker-compose.yml" exec -T nginx nginx -t 2>/dev/null; then
    echo "ERROR: nginx config test failed with the new vhost in place."
    restore_vhost
    echo ""
    echo "Check that the wildcard cert for niniconai.com exists (niniconai's"
    echo "scripts/init-ssl.sh) and review the full test output:"
    echo "  docker compose -f $BHARATSC_DIR/docker-compose.yml exec nginx nginx -t"
    [[ -n "$BACKUP" ]] && rm -f "$BACKUP"
    exit 1
fi

if ! docker compose -f "$BHARATSC_DIR/docker-compose.yml" exec -T nginx nginx -s reload; then
    echo "ERROR: nginx reload failed even though the config tested clean."
    restore_vhost
    [[ -n "$BACKUP" ]] && rm -f "$BACKUP"
    exit 1
fi

[[ -n "$BACKUP" ]] && rm -f "$BACKUP"
echo "Nginx config installed and reloaded."

echo ""
docker compose ps
echo ""
echo "Deployed: https://$DOMAIN"
