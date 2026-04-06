#!/usr/bin/env bash
# Run Playwright E2E tests against a temporary Uncloud server.
#
# Usage:
#   ./tests/playwright/run.sh                  # run all tests
#   ./tests/playwright/run.sh auth.spec.ts     # run a specific test file
#   ./tests/playwright/run.sh --headed         # visible browser
#
# Prerequisites:
#   - MongoDB running on localhost:27017 (or set MONGO_URI)
#   - cargo build for uncloud-server
#   - npm install in tests/playwright/
#   - Playwright browsers installed (npx playwright install chromium)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Configurable
# Server port must be 8080 — Dioxus.toml proxies /api/ to localhost:8080.
SERVER_PORT="${SERVER_PORT:-8080}"
FRONTEND_PORT="${FRONTEND_PORT:-3000}"
MONGO_URI="${MONGO_URI:-mongodb://localhost:27017}"
MONGO_DB="${MONGO_DB:-uncloud_playwright_$(date +%s)}"
STORAGE_DIR="$(mktemp -d)"
CONFIG_FILE="$(mktemp)"

cleanup() {
    echo ""
    echo "==> Cleaning up..."
    # Kill background processes
    [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null
    [ -n "${DX_PID:-}" ] && kill "$DX_PID" 2>/dev/null && wait "$DX_PID" 2>/dev/null

    # Drop the test database
    if command -v mongosh &>/dev/null; then
        mongosh "$MONGO_URI/$MONGO_DB" --quiet --eval "db.dropDatabase()" 2>/dev/null || true
    fi

    rm -rf "$STORAGE_DIR" "$CONFIG_FILE"
    echo "==> Done."
}
trap cleanup EXIT

# ── 1. Write a temp config ────────────────────────────────────────────────────

cat > "$CONFIG_FILE" <<EOF
server:
  host: "127.0.0.1"
  port: $SERVER_PORT

database:
  uri: "$MONGO_URI"
  name: "$MONGO_DB"

storage:
  default_path: "$STORAGE_DIR"

auth:
  session_duration_hours: 1
  registration: open

uploads:
  max_chunk_size: 10485760
  max_file_size: 0
  temp_cleanup_hours: 24

search:
  enabled: false
EOF

echo "==> Config: $CONFIG_FILE"
echo "==> Database: $MONGO_DB"
echo "==> Storage: $STORAGE_DIR"

# ── 2. Build the server ──────────────────────────────────────────────────────

echo "==> Building server..."
cargo build -p uncloud-server --quiet 2>&1

# ── 3. Start the backend server ──────────────────────────────────────────────

echo "==> Starting server on port $SERVER_PORT..."
CONFIG_PATH="$CONFIG_FILE" "$ROOT_DIR/target/debug/uncloud-server" &
SERVER_PID=$!

# Wait for the server to be healthy
echo -n "==> Waiting for server health"
for i in $(seq 1 30); do
    if curl -sf "http://127.0.0.1:$SERVER_PORT/health" >/dev/null 2>&1; then
        echo " ready!"
        break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo ""
        echo "ERROR: Server exited unexpectedly"
        exit 1
    fi
    echo -n "."
    sleep 1
done

if ! curl -sf "http://127.0.0.1:$SERVER_PORT/health" >/dev/null 2>&1; then
    echo ""
    echo "ERROR: Server did not become healthy in 30 seconds"
    exit 1
fi

# ── 4. Start the Dioxus dev server (frontend proxy) ─────────────────────────

echo "==> Starting frontend dev server on port $FRONTEND_PORT..."
(cd "$ROOT_DIR/crates/uncloud-web" && dx serve --port "$FRONTEND_PORT" 2>&1) &
DX_PID=$!

# Wait for the frontend to be ready
echo -n "==> Waiting for frontend"
for i in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:$FRONTEND_PORT/" >/dev/null 2>&1; then
        echo " ready!"
        break
    fi
    if ! kill -0 "$DX_PID" 2>/dev/null; then
        echo ""
        echo "WARNING: dx serve exited — running tests against backend directly"
        DX_PID=""
        FRONTEND_PORT="$SERVER_PORT"
        break
    fi
    echo -n "."
    sleep 1
done

# ── 5. Install deps if needed ────────────────────────────────────────────────

if [ ! -d "$SCRIPT_DIR/node_modules" ]; then
    echo "==> Installing npm dependencies..."
    npm install --prefix "$SCRIPT_DIR"
fi

# ── 6. Run Playwright ────────────────────────────────────────────────────────

echo "==> Running Playwright tests..."
echo ""

export BASE_URL="http://127.0.0.1:$FRONTEND_PORT"
export MONGO_URI
export MONGO_DB

cd "$SCRIPT_DIR"
npx playwright test "$@"
