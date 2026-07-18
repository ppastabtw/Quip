#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

cargo fmt --manifest-path src-tauri/sidecars/inference/Cargo.toml -- --check
cargo test --workspace
cargo build -p quip-inference-sidecar

sidecar="target/debug/quip-inference-sidecar"
phrase_tester="target/debug/quip-phrase-tester"
responses=$(mktemp "${TMPDIR:-/tmp}/quip-sidecar-responses.XXXXXX")
trap 'rm -f "$responses"' EXIT HUP INT TERM

{
  printf '%s\n' '{"operation":"health"}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"validation-fresh-id","profile_id":"profile_default","model_variant":"base","draft":"cnt cm tmrw","context_snippets":[],"personal_patterns":[]}}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"validation-five","profile_id":"profile_default","model_variant":"global","draft":"cnt cm tmrw","context_snippets":[],"personal_patterns":[]}}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"pred_protected_global","profile_id":"profile_default","model_variant":"global","draft":"open usr/bin and q3_finl_v2.pdf","context_snippets":[],"personal_patterns":[]}}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"pred_missing_adapter","profile_id":"profile_default","model_variant":"global","draft":"cnt cm tmrw","context_snippets":[],"personal_patterns":[]}}'
} | "$sidecar" > "$responses"

python3 - "$responses" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
responses = [json.loads(line) for line in path.read_text().splitlines() if line]
assert len(responses) == 5, responses

health, replacement, five_candidates, zero_candidates, missing_adapter = responses
assert health == {
    "status": "ready",
    "fixture_available": True,
    "loaded": {"base": False, "global_adapter": False, "user_adapter": False},
}, health
assert replacement["request_id"] == "validation-fresh-id", replacement
assert replacement["status"] == "ok", replacement
assert replacement["model_variant"] == "base", replacement
assert replacement["backend"] == "fixture", replacement
assert "action" not in replacement, replacement
assert 1 <= len(replacement["candidates"]) <= 5, replacement
assert five_candidates["status"] == "ok", five_candidates
assert "action" not in five_candidates, five_candidates
assert len(five_candidates["candidates"]) == 5, five_candidates
assert len(five_candidates["candidates"]) == len(set(five_candidates["candidates"])), five_candidates
assert zero_candidates["status"] == "ok", zero_candidates
assert "action" not in zero_candidates, zero_candidates
assert zero_candidates["candidates"] == [], zero_candidates
assert missing_adapter["request_id"] == "pred_missing_adapter", missing_adapter
assert missing_adapter["status"] == "error", missing_adapter
assert missing_adapter["error"]["code"] == "adapter_not_loaded", missing_adapter

for response in responses:
    print(json.dumps(response, separators=(",", ":")))
PY

phrase_output=$("$phrase_tester" "cnt cm tmrw")
case "$phrase_output" in
  *"Fixture mode only — no AI model is loaded."*"Base: candidates -> I can't come tomorrow."*"Global: candidates -> Can't come tomorrow."*) ;;
  *)
    printf '%s\n' "$phrase_output" >&2
    printf '%s\n' 'Phrase tester comparison failed' >&2
    exit 1
    ;;
esac
printf '%s\n' "$phrase_output"

printf '%s\n' 'Quip sidecar integration passed'
