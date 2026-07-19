#!/usr/bin/env python3
"""Send one synthetic InputMethodKit capture through Quip's real loopback bridge."""

from __future__ import annotations

import json
import socket
import sys


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("usage: send-capture.py SESSION_ID DRAFT")
    message = {
        "type": "capture",
        "session_id": sys.argv[1],
        "generation": 1,
        "draft": sys.argv[2],
        "caret": {"x": 80.0, "y": 80.0, "width": 2.0, "height": 18.0},
    }
    with socket.create_connection(("127.0.0.1", 48731), timeout=3) as stream:
        stream.sendall(json.dumps(message, separators=(",", ":")).encode() + b"\n")
        stream.settimeout(3)
        response = stream.makefile("r", encoding="utf-8").readline()
    payload = json.loads(response)
    if payload.get("type") != "capture_accepted":
        raise SystemExit(f"native bridge rejected capture: {payload}")
    print(f"native bridge accepted {sys.argv[1]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
