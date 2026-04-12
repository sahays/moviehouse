#!/usr/bin/env bash
set -euo pipefail

echo "Building moviehouse..."
cargo build --release

BINARY="./target/release/moviehouse"
PID_FILE="/tmp/moviehouse.pid"
LOG_FILE="/tmp/moviehouse.log"
BIND="${1:-0.0.0.0:3000}"

# Kill existing instance
if [ -f "$PID_FILE" ]; then
    old_pid=$(cat "$PID_FILE")
    if kill -0 "$old_pid" 2>/dev/null; then
        echo "Stopping existing instance (PID $old_pid)..."
        kill "$old_pid" 2>/dev/null || true
        sleep 1
    fi
    rm -f "$PID_FILE"
fi

# Start in background
echo "Starting moviehouse serve..."
nohup "$BINARY" serve --bind "$BIND" -v > "$LOG_FILE" 2>&1 &
echo $! > "$PID_FILE"
echo "PID: $(cat "$PID_FILE")"

# Determine check URL (0.0.0.0 isn't reachable, use 127.0.0.1)
PORT="${BIND##*:}"
CHECK_URL="http://127.0.0.1:${PORT}"

# Wait for server to be ready
for i in $(seq 1 30); do
    if curl -s "$CHECK_URL" > /dev/null 2>&1; then
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
exit 1
