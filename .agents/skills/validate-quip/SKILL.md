---
name: validate-quip
description: Validate the Quip Tauri app end-to-end after behavior changes — unit tests, headless selftest through the real app runtime, and a visual check of the IME candidate bar. Use after changing anything under src-tauri/, src/ui/, or crates/quip-contracts.
---

# Validate Quip

Unit tests alone are not completion evidence (AGENTS.md). Run all three layers
and report the visible results.

## 1. Unit and contract tests

```sh
cargo test            # workspace: contracts round-trip + engine/learning/inference tests
npm run build         # tsc typecheck + vite bundle
```

## 2. Headless selftest (real app runtime)

Drives the full IME fixture flow through the running app: capture →
prediction → suggesting bar state, candidate selection with in-place
replacement, keep-shows-nothing, dismissal learning records, simulated
adapter failure, profile divergence, metrics.

```sh
npm run dev &                       # debug builds load the vite dev server
cargo build
QUIP_DATA_DIR=$(mktemp -d) QUIP_SELFTEST=1 ./target/debug/quip | grep SELFTEST
```

Expect `SELFTEST ok:` lines and a final `SELFTEST PASS` (exit 0). Then inspect
the temp data dir: `profiles/*/examples.jsonl` should contain the confirmed
and dismissal records, and `logs/quip.log.*` should contain JSON log lines.

## 3. Visual check

```sh
QUIP_DATA_DIR=$(mktemp -d) QUIP_SHOW=demo ./target/debug/quip &
```

- Message composer (demo window): type `cnt cm tmrw`, pause ~1 s → the bar appears
  above the caret; `1` or click replaces the text in place; Esc or continued
  typing dismisses it and the typed text stays. Try `ship spec tn` on
  profile_a vs profile_b (tray → Profile) for divergent candidates.
- A `keep` case (`open usr/bin and q3_finl_v2.pdf` with global variant) must
  show no bar at all.
- Demo window: run each corpus case — every case shows two visibly different
  sides; "Simulate adapter failure" flips health to `degraded` and the bar to
  an explicit error chip.

Kill the app and vite when done. Report what was observed, not just exit codes.
