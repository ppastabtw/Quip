//! Workstream 3: Accessibility permission detection, focused editable element
//! recognition, secure-field exclusion, passive burst observation, caret
//! tracking, and bounded window text.
//!
//! IME model: keystrokes pass through to the destination untouched. This
//! layer observes the typed burst and the caret rectangle, emits
//! `quip_contracts::CaptureResult` on triggers, and swallows only the
//! candidate-selection keys (1–5, Escape) while the bar is visible. Element
//! handles and burst-range markers stay internal; only the opaque
//! `destination_id` and the caret rect cross the boundary.
