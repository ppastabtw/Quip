//! Workstream 4: local per-user learning.
//!
//! Profile-scoped storage in the app data dir: append-only compact labeled
//! examples (confirmed candidates, stable dismissals as `keep` labels,
//! post-commit corrections) and the deduplicated pattern dictionary that feeds
//! `personal_patterns` in prediction requests. Supports pause, inspect, and
//! reset. Personal records never leave the Mac and are never committed.
