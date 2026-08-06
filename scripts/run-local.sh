#!/usr/bin/env bash
set -euo pipefail

# Run from the project root regardless of where this script is invoked.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

BINARY="./target/release/moviehouse"
PID_FILE="/tmp/moviehouse.pid"
LOG_FILE="/tmp/moviehouse.log"
BIND="${1:-0.0.0.0:9000}"
PORT="${BIND##*:}"

echo "Building moviehouse..."
cargo build --release

# ── Stop any existing instance ────────────────────────────────────
# Kill whatever is actually listening on the port (a manually-started
# instance won't be in our pidfile), then clean up the pidfile.
if command -v lsof &>/dev/null; then
    port_pids="$(lsof -ti "tcp:${PORT}" 2>/dev/null || true)"
    if [ -n "$port_pids" ]; then
        echo "Stopping process(es) on port ${PORT}: ${port_pids//$'\n'/ }"
        # shellcheck disable=SC2086
        kill $port_pids 2>/dev/null || true
        sleep 1
    fi
fi
if [ -f "$PID_FILE" ]; then
    old_pid=$(cat "$PID_FILE")
    if kill -0 "$old_pid" 2>/dev/null; then
        echo "Stopping tracked instance (PID $old_pid)..."
        kill "$old_pid" 2>/dev/null || true
        sleep 1
    fi
    rm -f "$PID_FILE"
fi

# ── Start in background ───────────────────────────────────────────
echo "Starting moviehouse serve..."
nohup "$BINARY" serve --bind "$BIND" -v > "$LOG_FILE" 2>&1 &
echo $! > "$PID_FILE"
echo "PID: $(cat "$PID_FILE")"

# Determine check URL (0.0.0.0 isn't reachable, use 127.0.0.1)
CHECK_URL="http://127.0.0.1:${PORT}"

# Wait for server to be ready. /health is public and cheap — the SPA route works
# too but drags the whole embedded bundle through on every poll.
for _ in $(seq 1 30); do
    if curl -sf "${CHECK_URL}/health" > /dev/null 2>&1; then
        echo "Web UI ready at http://$BIND"
        HOSTNAME=$(hostname -s 2>/dev/null || echo "localhost")
        echo "Network access: http://${HOSTNAME}.local:${PORT}"
        # Open browser
        if command -v open &>/dev/null; then
            open "$CHECK_URL"
        elif command -v xdg-open &>/dev/null; then
            xdg-open "$CHECK_URL"
        else
            echo "Open $CHECK_URL in your browser"
        fi
        exit 0
    fi
    sleep 0.5
done

echo "Server did not start in time. Check $LOG_FILE"
tail -5 "$LOG_FILE" 2>/dev/null || true
exit 1
