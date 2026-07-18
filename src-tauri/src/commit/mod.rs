//! Workstream 3: destination restore and confirmed-text commit through
//! Accessibility insertion or selection replacement, with a simulated-paste
//! fallback that preserves and restores the previous clipboard.
//!
//! Receives the opaque `destination_id` and confirmed text from the
//! composition layer. Never inserts without explicit confirmation.
