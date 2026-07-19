#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$ROOT"

TMP_ROOT="$(mktemp -d)"
SAFE_PID=""
cleanup() {
  if [[ -n "$SAFE_PID" ]] && kill -0 "$SAFE_PID" 2>/dev/null; then
    kill "$SAFE_PID" 2>/dev/null || true
    wait "$SAFE_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

echo "==> cargo fmt -p quip --check"
cargo fmt -p quip --check

echo "==> cargo test"
cargo test

echo "==> npm run build"
npm run build

echo "==> tauri selftest"
SELFTEST_LOG="$TMP_ROOT/selftest.log"
QUIP_SELFTEST=1 \
  QUIP_DATA_DIR="$TMP_ROOT/selftest-data" \
  npm run tauri -- dev >"$SELFTEST_LOG" 2>&1
grep -F "SELFTEST PASS" "$SELFTEST_LOG" >/dev/null

echo "==> safe demo startup"
DEBUG_DIR=".workspace/quip-debug"
EVENT_LOG="$DEBUG_DIR/events.jsonl"
SAFE_LOG="$TMP_ROOT/safe-demo.log"
rm -rf "$DEBUG_DIR"
mkdir -p "$DEBUG_DIR"

QUIP_DEMO_SAFE_MODE=1 \
  QUIP_SHOW=demo \
  QUIP_DATA_DIR="$TMP_ROOT/safe-demo-data" \
  QUIP_DEBUG_DIR="$DEBUG_DIR" \
  npm run tauri -- dev >"$SAFE_LOG" 2>&1 &
SAFE_PID="$!"

deadline=$((SECONDS + 20))
required_events=(
  "demo_safe_mode_started"
  "capture_ready"
  "prediction_started"
  "prediction_result"
  "bar_shown"
)

while (( SECONDS < deadline )); do
  if [[ -f "$EVENT_LOG" ]]; then
    all_present=1
    for event in "${required_events[@]}"; do
      if ! grep -F "\"event\":\"$event\"" "$EVENT_LOG" >/dev/null; then
        all_present=0
        break
      fi
    done
    if [[ "$all_present" -eq 1 ]]; then
      break
    fi
  fi
  if ! kill -0 "$SAFE_PID" 2>/dev/null; then
    echo "safe demo process exited before required events" >&2
    tail -80 "$SAFE_LOG" >&2 || true
    exit 1
  fi
  sleep 0.5
done

for event in "${required_events[@]}"; do
  if ! grep -F "\"event\":\"$event\"" "$EVENT_LOG" >/dev/null; then
    echo "missing safe demo event: $event" >&2
    echo "--- safe demo output ---" >&2
    tail -80 "$SAFE_LOG" >&2 || true
    echo "--- debug events ---" >&2
    cat "$EVENT_LOG" >&2 || true
    exit 1
  fi
done

echo "Quip demo validation passed"
