#!/usr/bin/env bash
# Run Playwright E2E tests via Docker Compose.
#
# Terminal output: Playwright results only.
# Full service logs (mongo + server + web + playwright): saved to a timestamped file.
#
# Usage:
#   ./run-e2e-tests.sh          # run and tear down
#   ./run-e2e-tests.sh --keep   # leave containers running after the run (for debugging)

set -uo pipefail

COMPOSE_FILE="docker-compose.playwright.yml"
LOG_DIR="tests/playwright/logs"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RAW_LOG="$LOG_DIR/$TIMESTAMP-raw.log"
KEEP_CONTAINERS=0

for arg in "$@"; do
  [[ "$arg" == "--keep" ]] && KEEP_CONTAINERS=1
done

mkdir -p "$LOG_DIR"

echo "=== Playwright E2E Tests ==="
echo "Raw service logs → $RAW_LOG"
echo ""

# Run the full stack.
#   - tee saves every line (all services) to the raw log file.
#   - awk filters to lines that start with "playwright" and strips the compose prefix.
#
# docker compose attaches a prefix like: "playwright-1  | "
# We match that and remove it before printing.
docker compose -f "$COMPOSE_FILE" \
    up --build --abort-on-container-exit --exit-code-from playwright \
    2>&1 \
  | tee "$RAW_LOG" \
  | awk '/^playwright(-[0-9]+)?[[:space:]]+\|/ {
      sub(/^playwright(-[0-9]+)?[[:space:]]+\| ?/, "")
      print
    }'

COMPOSE_EXIT=${PIPESTATUS[0]}

# Tear down unless --keep was passed
if [[ $KEEP_CONTAINERS -eq 0 ]]; then
  docker compose -f "$COMPOSE_FILE" down --remove-orphans >> "$RAW_LOG" 2>&1 || true
else
  echo ""
  echo "Containers left running (--keep). Stop them with:"
  echo "  docker compose -f $COMPOSE_FILE down"
fi

# HTML report is written to the volume-mounted test-results directory
REPORT_DIR="tests/playwright/test-results"

echo ""
if [[ $COMPOSE_EXIT -eq 0 ]]; then
  echo "All tests passed."
else
  echo "Tests failed (exit $COMPOSE_EXIT)."
  echo "  Full logs:   $RAW_LOG"
  [[ -d "$REPORT_DIR" ]] && echo "  HTML report: $REPORT_DIR/index.html  (open in a browser)"
fi
echo ""

exit "$COMPOSE_EXIT"
