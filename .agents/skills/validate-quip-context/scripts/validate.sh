#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$ROOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Quip native context validation requires macOS" >&2
  exit 1
fi

if python3 - <<'PY'
import socket
with socket.socket() as stream:
    raise SystemExit(0 if stream.connect_ex(("127.0.0.1", 48731)) == 0 else 1)
PY
then
  echo "native IME bridge port 48731 is already in use; quit Quip before validation" >&2
  exit 1
fi

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/quip-context-validation.XXXXXX")"
APP_PID=""
TEXTEDIT_PID=""
CHROME_PID=""
TEXTEDIT_NAME="quip-context-textedit-$$.txt"
TEXTEDIT_FILE="$TMP_ROOT/$TEXTEDIT_NAME"
CHROME_TITLE="Quip Context Validation $$"
CHROME_FILE="$TMP_ROOT/quip-context-chrome-$$.html"
EVENTS="$TMP_ROOT/debug/events.jsonl"

cleanup() {
  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  if [[ -n "$TEXTEDIT_PID" ]] && kill -0 "$TEXTEDIT_PID" 2>/dev/null; then
    kill "$TEXTEDIT_PID" 2>/dev/null || true
  fi
  if [[ -n "$CHROME_PID" ]] && kill -0 "$CHROME_PID" 2>/dev/null; then
    kill "$CHROME_PID" 2>/dev/null || true
    wait "$CHROME_PID" 2>/dev/null || true
  fi
  while read -r chrome_child; do
    [[ -n "$chrome_child" ]] && kill "$chrome_child" 2>/dev/null || true
  done < <(pgrep -f -- "--user-data-dir=$TMP_ROOT/chrome-profile" 2>/dev/null || true)
  sleep 0.5
  rm -rf -- "$TMP_ROOT"
}
trap cleanup EXIT HUP INT TERM

printf '%s\n' 'QUIP_TEXTEDIT_CONTEXT_MARKER project review is tomorrow.' >"$TEXTEDIT_FILE"
python3 - "$CHROME_FILE" "$CHROME_TITLE" <<'PY'
import html
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
title = html.escape(sys.argv[2])
path.write_text(
    f"""<!doctype html><html><head><title>{title}</title></head>
<body><main><p>QUIP_CHROME_CONTEXT_MARKER Mira confirmed Union Station.</p>
<p>The project review is tomorrow afternoon.</p>
<textarea autofocus aria-label="Message composer"></textarea></main></body></html>""",
    encoding="utf-8",
)
PY

cargo fmt -p quip --check
cargo test -p quip accessibility::tests::window_context
cargo build -p quip

mkdir -p "$TMP_ROOT/debug"
QUIP_DATA_DIR="$TMP_ROOT/data" \
QUIP_DEBUG_DIR="$TMP_ROOT/debug" \
QUIP_DEBUG_TEXT=1 \
QUIP_BACKEND_MODE=fixture \
QUIP_MODEL_VARIANT=base \
"$ROOT/target/debug/quip" >"$TMP_ROOT/quip.log" 2>&1 &
APP_PID="$!"

deadline=$((SECONDS + 10))
until python3 - <<'PY'
import socket
with socket.socket() as stream:
    raise SystemExit(0 if stream.connect_ex(("127.0.0.1", 48731)) == 0 else 1)
PY
do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    tail -80 "$TMP_ROOT/quip.log" >&2 || true
    echo "Quip exited before the native bridge started" >&2
    exit 1
  fi
  if (( SECONDS >= deadline )); then
    tail -80 "$TMP_ROOT/quip.log" >&2 || true
    echo "timed out waiting for the native bridge" >&2
    exit 1
  fi
  sleep 0.2
done

textedit_before="$(pgrep -x TextEdit 2>/dev/null || true)"
open -n -a TextEdit "$TEXTEDIT_FILE"
sleep 1
TEXTEDIT_PID="$(pgrep -n -x TextEdit)"
if grep -qx "$TEXTEDIT_PID" <<<"$textedit_before"; then
  echo "TextEdit did not launch an isolated validation process" >&2
  exit 1
fi
python3 .agents/skills/validate-quip-context/scripts/send-capture.py \
  context_textedit "project review tmrw"
python3 .agents/skills/validate-quip-context/scripts/assert-context.py \
  "$EVENTS" native_burst_context_textedit TextEdit QUIP_TEXTEDIT_CONTEXT_MARKER

CHROME_EXEC="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
if [[ ! -x "$CHROME_EXEC" ]]; then
  echo "Google Chrome is required for Quip browser context validation" >&2
  exit 1
fi
"$CHROME_EXEC" \
  --user-data-dir="$TMP_ROOT/chrome-profile" \
  --app="file://$CHROME_FILE" \
  --no-first-run \
  --no-default-browser-check \
  --disable-extensions \
  --disable-background-mode \
  --disable-sync \
  --force-renderer-accessibility=complete \
  >"$TMP_ROOT/chrome.log" 2>&1 &
CHROME_PID="$!"
sleep 3
python3 .agents/skills/validate-quip-context/scripts/send-capture.py \
  context_chrome "meet there tmrw"
python3 .agents/skills/validate-quip-context/scripts/assert-context.py \
  "$EVENTS" native_burst_context_chrome "Google Chrome" QUIP_CHROME_CONTEXT_MARKER

printf '%s\n' 'Quip native context integration passed'
