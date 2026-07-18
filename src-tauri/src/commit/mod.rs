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
    AccessibilityWriteFailed,
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
trait CommitSession {
    fn has_destination(&self, destination_id: &str) -> bool;
    fn write_accessibility(
        &mut self,
        destination_id: &str,
        confirmed_text: &str,
    ) -> Result<(), CommitError>;
    fn release_destination(&mut self, destination_id: &str);
}

#[cfg_attr(not(test), allow(dead_code))]
fn commit_confirmed_text_with_session(
    destination_id: &str,
    confirmed_text: &str,
    session: &mut impl CommitSession,
    clipboard: &mut impl ClipboardSession,
) -> Result<CommitReport, CommitError> {
    if !session.has_destination(destination_id) {
        return Err(CommitError::UnknownDestination);
    }

    match session.write_accessibility(destination_id, confirmed_text) {
        Ok(()) => {}
        Err(CommitError::AccessibilityWriteFailed) => {
            return simulated_paste_fallback(destination_id, confirmed_text, clipboard);
        }
        Err(error) => return Err(error),
    }

    Ok(CommitReport {
        destination_id: destination_id.to_string(),
        method: CommitMethod::Accessibility,
        inserted_text: true,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn cancel_destination_with_session(
    destination_id: &str,
    session: &mut impl CommitSession,
) -> Result<CommitReport, CommitError> {
    if !session.has_destination(destination_id) {
        return Err(CommitError::UnknownDestination);
    }

    session.release_destination(destination_id);

    Ok(CommitReport {
        destination_id: destination_id.to_string(),
        method: CommitMethod::Cancelled,
        inserted_text: false,
    })
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
    fn commit_refuses_unknown_destination_id() {
        let mut session = FakeCommitSession::default();
        let mut clipboard = FakeClipboardSession::new("previous clipboard");

        let result = commit_confirmed_text_with_session(
            "destination_missing",
            "confirmed text",
            &mut session,
            &mut clipboard,
        );

        assert_eq!(result, Err(CommitError::UnknownDestination));
    }

    #[test]
    fn cancel_releases_destination_without_text() {
        let mut session = FakeCommitSession::with_destination("destination_textedit_0001");

        let report = cancel_destination_with_session("destination_textedit_0001", &mut session)
            .expect("known destination should cancel cleanly");

        assert_eq!(
            (
                report.inserted_text,
                session.has_destination("destination_textedit_0001")
            ),
            (false, false)
        );
    }

    #[test]
    fn commit_uses_paste_fallback_when_accessibility_write_fails() {
        let mut session = FakeCommitSession::with_destination("destination_textedit_0001")
            .with_accessibility_write_result(Err(CommitError::AccessibilityWriteFailed));
        let mut clipboard = FakeClipboardSession::new("previous clipboard");

        let report = commit_confirmed_text_with_session(
            "destination_textedit_0001",
            "confirmed text",
            &mut session,
            &mut clipboard,
        )
        .expect("fallback should handle direct write failure");

        assert_eq!(report.method, CommitMethod::SimulatedPaste);
    }

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

    struct FakeCommitSession {
        destination_id: Option<String>,
        accessibility_write_result: Result<(), CommitError>,
    }

    impl Default for FakeCommitSession {
        fn default() -> Self {
            Self {
                destination_id: None,
                accessibility_write_result: Ok(()),
            }
        }
    }

    impl FakeCommitSession {
        fn with_destination(destination_id: &str) -> Self {
            Self {
                destination_id: Some(destination_id.to_string()),
                accessibility_write_result: Ok(()),
            }
        }

        fn with_accessibility_write_result(mut self, result: Result<(), CommitError>) -> Self {
            self.accessibility_write_result = result;
            self
        }
    }

    impl CommitSession for FakeCommitSession {
        fn has_destination(&self, destination_id: &str) -> bool {
            self.destination_id.as_deref() == Some(destination_id)
        }

        fn write_accessibility(
            &mut self,
            _destination_id: &str,
            _confirmed_text: &str,
        ) -> Result<(), CommitError> {
            self.accessibility_write_result.clone()
        }

        fn release_destination(&mut self, destination_id: &str) {
            if self.has_destination(destination_id) {
                self.destination_id = None;
            }
        }
    }
}
