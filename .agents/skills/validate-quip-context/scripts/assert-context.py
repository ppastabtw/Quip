#!/usr/bin/env python3
"""Wait for one redaction-disabled context event without printing its text."""

from __future__ import annotations

import json
import pathlib
import sys
import time


def main() -> int:
    if len(sys.argv) not in (5, 6):
        raise SystemExit(
            "usage: assert-context.py EVENTS BURST_PART APP MARKER "
            "[EXCLUDED_MARKER]"
        )
    path = pathlib.Path(sys.argv[1])
    burst_part, expected_app, marker = sys.argv[2:5]
    excluded_marker = sys.argv[5] if len(sys.argv) == 6 else None
    deadline = time.monotonic() + 12
    last_event: dict[str, object] | None = None
    captured_chars: int | None = None
    prediction_started = False
    prediction_used_context = False
    while time.monotonic() < deadline:
        if path.exists():
            for line in path.read_text(encoding="utf-8").splitlines():
                event = json.loads(line)
                payload = event.get("payload", {})
                if (
                    event.get("event") == "capture_ready"
                    and burst_part in str(payload.get("burst_id", ""))
                ):
                    last_event = event
                    snippets = payload.get("context_snippets", [])
                    if payload.get("context_count") != 1 or len(snippets) != 1:
                        break
                    snippet = snippets[0]
                    if snippet.get("app_name") != expected_app:
                        break
                    text = snippet.get("visible_text", "")
                    if marker not in text:
                        break
                    if excluded_marker is not None and excluded_marker in text:
                        raise SystemExit(
                            "context snippet contained the excluded marker"
                        )
                    if len(text) > 240:
                        raise SystemExit("context snippet exceeded 240 characters")
                    captured_chars = snippet.get("visible_text_chars", len(text))
                elif (
                    event.get("event") == "prediction_started"
                    and burst_part in str(payload.get("burst_id", ""))
                ):
                    prediction_started = payload.get("context_count") == 1
                elif (
                    event.get("event") == "prediction_result"
                    and burst_part in str(payload.get("burst_id", ""))
                ):
                    prediction_used_context = (
                        payload.get("has_context") is True
                        and payload.get("context_count") == 1
                    )
            if (
                captured_chars is not None
                and prediction_started
                and prediction_used_context
            ):
                print(
                    f"{expected_app} context reached inference: "
                    f"{captured_chars} characters"
                )
                return 0
        time.sleep(0.2)
    if last_event is None:
        raise SystemExit(f"missing capture_ready event for {burst_part}")
    payload = last_event.get("payload", {})
    summary = {
        "context_count": payload.get("context_count"),
        "prediction_started_with_context": prediction_started,
        "prediction_result_used_context": prediction_used_context,
        "apps": [
            snippet.get("app_name")
            for snippet in payload.get("context_snippets", [])
            if isinstance(snippet, dict)
        ],
    }
    raise SystemExit(f"context assertion failed without printing text: {summary}")


if __name__ == "__main__":
    raise SystemExit(main())
