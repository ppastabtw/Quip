//! Workstream 3: destination restore and confirmed-text commit through
//! Accessibility insertion or selection replacement, with a simulated-paste
//! fallback that preserves and restores the previous clipboard.
//!
//! Until that lands, this stub receives the opaque `destination_id` and the
//! confirmed text, logs the commit, and returns it for display. The contract
//! it must keep: never insert without explicit confirmation, and never
//! interpret `destination_id`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CommitOutcome {
    pub destination_id: String,
    pub text: String,
}

pub fn commit_text(destination_id: &str, text: &str) -> CommitOutcome {
    tracing::info!(destination_id, chars = text.len(), "commit (stub: Workstream 3 lands the real insertion)");
    CommitOutcome {
        destination_id: destination_id.to_string(),
        text: text.to_string(),
    }
}
