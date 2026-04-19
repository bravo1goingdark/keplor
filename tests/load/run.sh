#!/usr/bin/env bash
# Load test harness for Keplor.
#
# Usage:
#   ./tests/load/run.sh [DURATION_SECS] [CONNECTIONS] [FEATURES]
#
# Prerequisites: hey (https://github.com/rakyll/hey)
#
# Builds the server, starts it with a tmpdir DB, generates load, prints
# latency histogram, then tears everything down.

set -euo pipefail

DURATION="${1:-30}"
CONNECTIONS="${2:-50}"
FEATURES="${3:-}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
CONFIG="$ROOT_DIR/tests/fixtures/bench-config.toml"
PAYLOAD="$ROOT_DIR/tests/fixtures/load/single-event.json"
DB_DIR="$(mktemp -d)"
DB_PATH="$DB_DIR/keplor-load.db"
BASE_URL="http://127.0.0.1:8080"

cleanup() {
    if [ -n "${SERVER_PID:-}" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$DB_DIR"
}
trap cleanup EXIT

# ── Preflight ────────────────────────────────────────────────────────
if ! command -v hey >/dev/null 2>&1; then
    echo "ERROR: 'hey' not found. Install: go install github.com/rakyll/hey@latest"
    exit 1
fi

if [ ! -f "$PAYLOAD" ]; then
    echo "ERROR: payload not found at $PAYLOAD"
    exit 1
fi

# ── Build ────────────────────────────────────────────────────────────
echo "=== Building keplor (profile=bench${FEATURES:+, features=$FEATURES}) ==="
BUILD_ARGS=(--profile bench -p keplor-cli)
if [ -n "$FEATURES" ]; then
    BUILD_ARGS+=(--features "$FEATURES")
fi
cargo build "${BUILD_ARGS[@]}" 2>&1

BINARY="$ROOT_DIR/target/bench/keplor"
if [ ! -x "$BINARY" ]; then
    # Fallback: profile.bench outputs may go to target/release on some configs
    BINARY="$ROOT_DIR/target/release/keplor"
fi

# ── Start server ─────────────────────────────────────────────────────
echo "=== Starting keplor (db=$DB_PATH) ==="
KEPLOR_STORAGE_DB_PATH="$DB_PATH" "$BINARY" run --config "$CONFIG" &
SERVER_PID=$!

for i in $(seq 1 50); do
    if curl -sf "$BASE_URL/health" >/dev/null 2>&1; then
        echo "Server ready (waited $((i * 100))ms)"
        break
    fi
    if [ "$i" -eq 50 ]; then
        echo "ERROR: server did not become healthy within 5s"
        exit 1
    fi
    sleep 0.1
done

# ── Warmup ───────────────────────────────────────────────────────────
echo "=== Warmup (2s) ==="
hey -z 2s -c 10 -m POST \
    -H "Content-Type: application/json" \
    -D "$PAYLOAD" \
    "$BASE_URL/v1/events" >/dev/null 2>&1 || true

# ── Load test: single event ─────────────────────────────────────────
echo ""
echo "=== Single event ingestion (${DURATION}s, ${CONNECTIONS} connections) ==="
hey -z "${DURATION}s" -c "$CONNECTIONS" -m POST \
    -H "Content-Type: application/json" \
    -D "$PAYLOAD" \
    "$BASE_URL/v1/events" 2>&1

# ── RSS check ────────────────────────────────────────────────────────
echo ""
echo "=== Memory usage ==="
if [ -f "/proc/$SERVER_PID/status" ]; then
    RSS_KB=$(grep VmRSS "/proc/$SERVER_PID/status" | awk '{print $2}')
    RSS_MB=$((RSS_KB / 1024))
    echo "VmRSS: ${RSS_MB} MB (target: <30 MB)"
    if [ "$RSS_MB" -gt 30 ]; then
        echo "WARNING: RSS exceeds 30 MB target"
    fi
else
    echo "(proc filesystem not available — skipping RSS check)"
fi

# ── DB stats ─────────────────────────────────────────────────────────
echo ""
echo "=== Database ==="
DB_SIZE=$(stat -c '%s' "$DB_PATH" 2>/dev/null || echo 0)
DB_SIZE_MB=$((DB_SIZE / 1024 / 1024))
echo "DB size: ${DB_SIZE_MB} MB (${DB_SIZE} bytes)"

echo ""
echo "=== Done ==="
