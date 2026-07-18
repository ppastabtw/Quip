//! Workstream 3: destination restore and confirmed-text commit through
//! Accessibility insertion or selection replacement, with a simulated-paste
//! fallback that preserves and restores the previous clipboard.
//!
//! Receives the opaque `destination_id` and confirmed text from the
//! composition layer. Never inserts without explicit confirmation.

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMethod {
    Accessibility,
    SimulatedPaste,
    Cancelled,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReport {
    pub destination_id: String,
    pub method: CommitMethod,
    pub inserted_text: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitError {
    UnknownDestination,
    UnsupportedClipboardContent,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClipboardSnapshot {
    PlainText(String),
}

#[cfg_attr(not(test), allow(dead_code))]
trait ClipboardSession {
    fn snapshot(&self) -> ClipboardSnapshot;
    fn set_plain_text(&mut self, text: &str);
    fn simulate_paste(&mut self);
    fn restore(&mut self, snapshot: ClipboardSnapshot);
}

#[cfg_attr(not(test), allow(dead_code))]
fn simulated_paste_fallback(
    destination_id: &str,
    confirmed_text: &str,
    clipboard: &mut impl ClipboardSession,
) -> Result<CommitReport, CommitError> {
    let previous_clipboard = clipboard.snapshot();
    clipboard.set_plain_text(confirmed_text);
    clipboard.simulate_paste();
    clipboard.restore(previous_clipboard);

    Ok(CommitReport {
        destination_id: destination_id.to_string(),
        method: CommitMethod::SimulatedPaste,
        inserted_text: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_paste_fallback_reports_plain_text_commit() {
        let mut clipboard = FakeClipboardSession::new("previous clipboard");

        let report = simulated_paste_fallback(
            "destination_textedit_0001",
            "confirmed text",
            &mut clipboard,
        )
        .expect("plain-text clipboard should allow simulated paste");

        assert_eq!(
            report,
            CommitReport {
                destination_id: "destination_textedit_0001".to_string(),
                method: CommitMethod::SimulatedPaste,
                inserted_text: true,
            }
        );
    }

    struct FakeClipboardSession {
        text: String,
    }

    impl FakeClipboardSession {
        fn new(text: &str) -> Self {
            Self {
                text: text.to_string(),
            }
        }
    }

    impl ClipboardSession for FakeClipboardSession {
        fn snapshot(&self) -> ClipboardSnapshot {
            ClipboardSnapshot::PlainText(self.text.clone())
        }

        fn set_plain_text(&mut self, text: &str) {
            self.text = text.to_string();
        }

        fn simulate_paste(&mut self) {}

        fn restore(&mut self, snapshot: ClipboardSnapshot) {
            match snapshot {
                ClipboardSnapshot::PlainText(text) => {
                    self.text = text;
                }
            }
        }
    }
}
