//! Workstream 4: composition state machine
//! (`Idle → Capturing → Predicting → Presenting → Committing | Cancelled`).
//!
//! Consumes `quip_contracts::CaptureResult` events, owns candidate state, and
//! enforces the UI invariants: the exact draft is always the first option,
//! `keep` never bypasses confirmation, errors render an explicit unavailable
//! state, and cancel commits nothing.
