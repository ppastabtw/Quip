//! Workstream 3: destination restore and confirmed-text commit through
//! Accessibility insertion or selection replacement, with a simulated-paste
//! fallback that preserves and restores the previous clipboard.
//!
//! IME model: commit means replacing the just-typed burst range in the
//! destination in place (the text is already there as typed). Until that
//! lands, this stub receives the opaque `destination_id` plus the selected
//! candidate, logs it, and returns it for display — the playground applies
//! the replacement itself. The contract it must keep: never replace without
//! an explicit selection, and never interpret `destination_id`.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CommitOutcome {
    pub destination_id: String,
    pub text: String,
}

pub fn replace_burst(destination_id: &str, burst_id: &str, text: &str) -> CommitOutcome {
    tracing::info!(
        destination_id,
        burst_id,
        chars = text.len(),
        "replace burst (stub: Workstream 3 lands the real in-place replacement)"
    );
    CommitOutcome {
        destination_id: destination_id.to_string(),
        text: text.to_string(),
    }
}
