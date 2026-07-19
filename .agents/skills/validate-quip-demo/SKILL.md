---
name: validate-quip-demo
description: Validate Quip's TextEdit demo path and safe demo fallback through the app runtime. Use after changing manual focused capture, safe demo mode, candidate-bar demo UI, demo fixtures, debug events, or app-side commit flow.
---

# Validate Quip Demo

Run the repository's deterministic app demo validator from the repository root:

```bash
.agents/skills/validate-quip-demo/scripts/validate.sh
```

The validator must:

1. Format-check the Tauri package.
2. Run the Rust tests that cover capture, prediction, commit, and fixture lookup behavior.
3. Build the demo webviews with TypeScript checking.
4. Run the app selftest through the real Tauri runtime and verify `SELFTEST PASS`.
5. Run the safe demo startup path with `QUIP_DEMO_SAFE_MODE=1 QUIP_SHOW=demo` and verify the debug event log contains `demo_safe_mode_started`, `capture_ready`, `prediction_started`, `prediction_result`, and `bar_shown`.
6. Keep generated debug logs outside tracked source or under `.workspace/quip-debug`.

Treat unit tests alone as insufficient. A successful run ends with `Quip demo validation passed` after the app-runtime checks. On failure, report the failing command and the relevant log excerpt; do not claim completion.
