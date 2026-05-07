#!/usr/bin/env bash
# macOS ships bash 3.2 (no associative arrays). Re-exec under brew bash if
# available; otherwise emulate with parallel arrays below.
# Boot iii-engine + lineage workers for local development.
#
# Architecture: engine on :3211 (HTTP), :3212 (stream), worker WS on :49234.
# Engine ports shifted +100 from iii defaults to avoid clashing with any
# other iii engine you have running. Console runs on its canonical default
# :3113 (since :3113 is unused by our engine, no need to shift).
#
# Worker supervision:
# Lineage workers run as separate processes that connect to the engine over
# WebSocket. This script supervises them: each worker is restarted if it
# dies. Once iii's worker registry supports local-dev binary registration
# (currently only registry/OCI workers), this script will be replaced by
# native iii-worker-manager supervision.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

LINEAGE_REPO_PATH="${LINEAGE_REPO_PATH:-$ROOT}"
III_URL="${III_URL:-ws://127.0.0.1:49234}"
RESTART_BACKOFF_MS="${RESTART_BACKOFF_MS:-500}"
LOG_DIR="${LOG_DIR:-$ROOT/data/logs}"
mkdir -p "$LOG_DIR"

WORKERS=(
  lineage-gitops
  lineage-strategy
  hook-claude-code
  hook-runtime-events
)

# Parallel arrays (bash 3.2-compatible) instead of associative arrays.
# WORKER_NAMES[i] -> WORKER_PIDS[i]
PIDS=()
WORKER_NAMES=()
WORKER_PIDS=()

cleanup() {
  echo "==> stopping..."
  for p in "${PIDS[@]:-}"; do
    kill "$p" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

start_worker() {
  local name="$1"
  III_URL="$III_URL" LINEAGE_REPO_PATH="$LINEAGE_REPO_PATH" \
    "$ROOT/target/release/$name" \
    >"$LOG_DIR/$name.log" 2>&1 &
  local pid=$!
  PIDS+=("$pid")
  # Find existing slot or append.
  local found=""
  local i
  for i in "${!WORKER_NAMES[@]}"; do
    if [ "${WORKER_NAMES[$i]}" = "$name" ]; then
      WORKER_PIDS[$i]=$pid
      found=1
      break
    fi
  done
  if [ -z "$found" ]; then
    WORKER_NAMES+=("$name")
    WORKER_PIDS+=("$pid")
  fi
  echo "    [$name] pid=$pid (logs: $LOG_DIR/$name.log)"
}

supervise() {
  while true; do
    sleep 2
    local i
    for i in "${!WORKER_NAMES[@]}"; do
      local name="${WORKER_NAMES[$i]}"
      local pid="${WORKER_PIDS[$i]}"
      if [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then
        echo "==> [$name] died, restarting in ${RESTART_BACKOFF_MS}ms..."
        sleep "$(awk "BEGIN{print $RESTART_BACKOFF_MS/1000}")"
        start_worker "$name"
      fi
    done
  done
}

echo "==> building lineage workers..."
cargo build --release

echo "==> starting iii-engine..."
echo "    HTTP    :3211"
echo "    stream  :3212"
echo "    worker  :49234"
iii --no-update-check --config "$ROOT/iii.config.yaml" \
  >"$LOG_DIR/iii-engine.log" 2>&1 &
PIDS+=("$!")

echo "==> waiting for engine WS port..."
for _ in $(seq 1 30); do
  if nc -z 127.0.0.1 49234 2>/dev/null; then break; fi
  sleep 0.2
done

echo "==> starting iii-console (canonical :3113, pointed at lineage's engine)..."
iii console \
  --port 3113 \
  --engine-port 3211 \
  --ws-port 3212 \
  --bridge-port 49234 \
  >"$LOG_DIR/iii-console.log" 2>&1 &
PIDS+=("$!")
echo "    console http://127.0.0.1:3113"

echo "==> starting lineage workers (LINEAGE_REPO_PATH=$LINEAGE_REPO_PATH)..."
for w in "${WORKERS[@]}"; do
  start_worker "$w"
done

echo "==> ready. Probe with:"
echo "    curl -X POST http://127.0.0.1:3211/hook/claude-code/stop \\"
echo "         -H 'Content-Type: application/json' \\"
echo "         -d '{\"session_id\":\"test\",\"hook_event_name\":\"Stop\"}'"
echo "==> Ctrl-C to stop everything. Worker crashes auto-restart."

# Run supervisor in foreground so we hold the script open and Ctrl-C works.
supervise
