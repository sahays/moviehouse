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

echo "Access code (in-app auth):"
if [[ -n "${MOVIEHOUSE_ACCESS_CODE:-}" && "${MOVIEHOUSE_ACCESS_CODE}" != "change_me" ]]; then
    pass "MOVIEHOUSE_ACCESS_CODE is set"
else
    fail "MOVIEHOUSE_ACCESS_CODE not set (or still 'change_me') in .env"; ERRORS=$((ERRORS + 1))
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
