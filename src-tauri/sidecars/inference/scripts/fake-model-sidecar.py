#!/usr/bin/env python3
"""Contract-faithful fake inference sidecar for UI-feel testing.

Speaks the newline-delimited JSON sidecar protocol and answers every draft
with plausible candidates after ~600 ms (jittered), matching the local
model's latency without needing mistral.rs or Qwen. Use via run-fake-app.sh.
"""
import json
import random
import sys
import time

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    command = json.loads(line)
    if command.get("operation") == "health":
        reply = {
            "status": "ready",
            "fixture_available": False,
            "loaded": {"base": True, "global_adapter": False, "user_adapter": False},
        }
    else:
        request = command["request"]
        started = time.monotonic()
        time.sleep(random.uniform(0.45, 0.75))
        draft = request["draft"]
        reply = {
            "status": "ok",
            "request_id": request["request_id"],
            "model_variant": request["model_variant"],
            "backend": "live",
            "candidates": [draft.capitalize() + ".", "ALT " + draft],
            "latency_ms": int((time.monotonic() - started) * 1000),
        }
    sys.stdout.write(json.dumps(reply) + "\n")
    sys.stdout.flush()
