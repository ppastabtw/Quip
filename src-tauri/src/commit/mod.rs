//! Workstream 3: destination restore and confirmed-text commit through
//! Accessibility insertion or selection replacement, with a simulated-paste
//! fallback that preserves and restores the previous clipboard.
//!
//! Receives the opaque `destination_id` and confirmed text from the
//! composition layer. Never inserts without explicit confirmation.

use crate::accessibility;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CommitOutcome {
    pub destination_id: String,
    pub text: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitMethod {
    Accessibility,
    SimulatedPaste,
    Cancelled,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommitReport {
    pub destination_id: String,
    pub method: CommitMethod,
    pub inserted_text: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitError {
    UnknownDestination,
    AccessibilityWriteFailed,
    ClipboardUnavailable,
    PasteSimulationFailed,
    UnsupportedClipboardContent,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClipboardSnapshot {
    PlainText(String),
}

#[cfg_attr(not(test), allow(dead_code))]
trait ClipboardSession {
    fn snapshot(&mut self) -> Result<ClipboardSnapshot, CommitError>;
    fn set_plain_text(&mut self, text: &str) -> Result<(), CommitError>;
    fn simulate_paste(&mut self) -> Result<(), CommitError>;
    fn restore(&mut self, snapshot: ClipboardSnapshot) -> Result<(), CommitError>;
}

#[cfg_attr(not(test), allow(dead_code))]
trait CommitSession {
    fn has_destination(&self, destination_id: &str) -> bool;
    fn write_accessibility(
        &mut self,
        destination_id: &str,
        confirmed_text: &str,
    ) -> Result<(), CommitError>;
    fn restore_destination(&mut self, destination_id: &str) -> Result<(), CommitError>;
    fn release_destination(&mut self, destination_id: &str);
}

#[cfg_attr(not(test), allow(dead_code))]
trait ClipboardProvider {
    type Clipboard: ClipboardSession;

    fn open(&mut self) -> Result<Self::Clipboard, CommitError>;
}

#[cfg_attr(not(test), allow(dead_code))]
fn cancel_destination_with_session(
    destination_id: &str,
    session: &mut impl CommitSession,
) -> Result<CommitReport, CommitError> {
    if !session.has_destination(destination_id) {
        return Err(CommitError::UnknownDestination);
    }

    session.restore_destination(destination_id)?;
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
    let previous_clipboard = clipboard.snapshot()?;
    clipboard.set_plain_text(confirmed_text)?;
    let paste_result = clipboard.simulate_paste();
    let restore_result = clipboard.restore(previous_clipboard);

    paste_result?;
    restore_result?;

    Ok(CommitReport {
        destination_id: destination_id.to_string(),
        method: CommitMethod::SimulatedPaste,
        inserted_text: true,
    })
}

#[allow(dead_code)]
pub fn commit_confirmed_text(
    destination_id: &str,
    confirmed_text: &str,
) -> Result<CommitReport, CommitError> {
    let mut session = LiveCommitSession;
    let mut clipboard = LiveClipboardProvider;

    commit_confirmed_text_with_clipboard_provider(
        destination_id,
        confirmed_text,
        &mut session,
        &mut clipboard,
    )
}

#[allow(dead_code)]
pub fn cancel_destination(destination_id: &str) -> Result<CommitReport, CommitError> {
    let mut session = LiveCommitSession;
    cancel_destination_with_session(destination_id, &mut session)
}

pub fn replace_burst(
    destination_id: &str,
    _burst_id: &str,
    text: &str,
) -> Result<CommitOutcome, String> {
    if is_virtual_destination(destination_id) {
        return Ok(CommitOutcome {
            destination_id: destination_id.to_string(),
            text: text.to_string(),
        });
    }

    commit_confirmed_text(destination_id, text).map_err(|error| {
        tracing::warn!(
            destination_id,
            error = ?error,
            "real accessibility commit failed"
        );
        format!("real accessibility commit failed: {error:?}")
    })?;

    Ok(CommitOutcome {
        destination_id: destination_id.to_string(),
        text: text.to_string(),
    })
}

fn is_virtual_destination(destination_id: &str) -> bool {
    matches!(
        destination_id,
        "destination_playground"
            | "destination_selftest"
            | "destination_textedit"
            | "destination_test"
    )
}

struct LiveCommitSession;

impl CommitSession for LiveCommitSession {
    fn has_destination(&self, destination_id: &str) -> bool {
        accessibility::destination_exists(destination_id)
    }

    fn write_accessibility(
        &mut self,
        destination_id: &str,
        confirmed_text: &str,
    ) -> Result<(), CommitError> {
        accessibility::write_confirmed_text_to_destination(destination_id, confirmed_text)
            .map_err(CommitError::from)
    }

    fn restore_destination(&mut self, destination_id: &str) -> Result<(), CommitError> {
        accessibility::restore_destination(destination_id).map_err(CommitError::from)
    }

    fn release_destination(&mut self, destination_id: &str) {
        let _ = accessibility::release_destination(destination_id);
    }
}

struct ArboardClipboardSession {
    clipboard: arboard::Clipboard,
}

impl ArboardClipboardSession {
    fn new() -> Result<Self, CommitError> {
        Ok(Self {
            clipboard: arboard::Clipboard::new().map_err(|_| CommitError::ClipboardUnavailable)?,
        })
    }
}

impl ClipboardSession for ArboardClipboardSession {
    fn snapshot(&mut self) -> Result<ClipboardSnapshot, CommitError> {
        let text = self
            .clipboard
            .get_text()
            .map_err(|_| CommitError::UnsupportedClipboardContent)?;
        Ok(ClipboardSnapshot::PlainText(text))
    }

    fn set_plain_text(&mut self, text: &str) -> Result<(), CommitError> {
        self.clipboard
            .set_text(text)
            .map_err(|_| CommitError::ClipboardUnavailable)
    }

    fn simulate_paste(&mut self) -> Result<(), CommitError> {
        let status = std::process::Command::new("osascript")
            .args([
                "-e",
                r#"tell application "System Events" to keystroke "v" using command down"#,
            ])
            .status()
            .map_err(|_| CommitError::PasteSimulationFailed)?;
        if status.success() {
            Ok(())
        } else {
            Err(CommitError::PasteSimulationFailed)
        }
    }

    fn restore(&mut self, snapshot: ClipboardSnapshot) -> Result<(), CommitError> {
        match snapshot {
            ClipboardSnapshot::PlainText(text) => self.set_plain_text(&text),
        }
    }
}

struct LiveClipboardProvider;

impl ClipboardProvider for LiveClipboardProvider {
    type Clipboard = ArboardClipboardSession;

    fn open(&mut self) -> Result<Self::Clipboard, CommitError> {
        ArboardClipboardSession::new()
    }
}

impl From<accessibility::AccessibilityError> for CommitError {
    fn from(error: accessibility::AccessibilityError) -> Self {
        match error {
            accessibility::AccessibilityError::DestinationNotFound => Self::UnknownDestination,
            accessibility::AccessibilityError::CommitFailed => Self::AccessibilityWriteFailed,
            _ => Self::AccessibilityWriteFailed,
        }
    }
}

fn commit_confirmed_text_with_clipboard_provider(
    destination_id: &str,
    confirmed_text: &str,
    session: &mut impl CommitSession,
    clipboard_provider: &mut impl ClipboardProvider,
) -> Result<CommitReport, CommitError> {
    if !session.has_destination(destination_id) {
        return Err(CommitError::UnknownDestination);
    }

    let commit_method_result = match session.write_accessibility(destination_id, confirmed_text) {
        Ok(()) => Ok(CommitMethod::Accessibility),
        Err(CommitError::AccessibilityWriteFailed) => session
            .restore_destination(destination_id)
            .and_then(|()| clipboard_provider.open())
            .and_then(|mut clipboard| {
                simulated_paste_fallback(destination_id, confirmed_text, &mut clipboard)
                    .map(|report| report.method)
            }),
        Err(error) => Err(error),
    };
    session.release_destination(destination_id);
    let method = commit_method_result?;

    Ok(CommitReport {
        destination_id: destination_id.to_string(),
        method,
        inserted_text: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_confirmed_text_rejects_unknown_live_destination() {
        let result = commit_confirmed_text("destination_missing", "confirmed text");

        assert_eq!(result, Err(CommitError::UnknownDestination));
    }

    #[test]
    fn cancel_destination_rejects_unknown_live_destination() {
        let result = cancel_destination("destination_missing");

        assert_eq!(result, Err(CommitError::UnknownDestination));
    }

    #[test]
    fn commit_refuses_unknown_destination_id() {
        let mut session = FakeCommitSession::default();
        let mut clipboard =
            FakeClipboardProvider::new(Ok(FakeClipboardSession::new("previous clipboard")));

        let result = commit_confirmed_text_with_clipboard_provider(
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
                session.restore_count,
                session.has_destination("destination_textedit_0001")
            ),
            (false, 1, false)
        );
    }

    #[test]
    fn commit_direct_accessibility_does_not_open_clipboard() {
        let mut session = FakeCommitSession::with_destination("destination_textedit_0001");
        let mut clipboard = FakeClipboardProvider::new(Err(CommitError::ClipboardUnavailable));

        let report = commit_confirmed_text_with_clipboard_provider(
            "destination_textedit_0001",
            "confirmed text",
            &mut session,
            &mut clipboard,
        )
        .expect("direct accessibility commit should not need clipboard access");

        assert_eq!(
            (report.method, clipboard.open_count),
            (CommitMethod::Accessibility, 0)
        );
    }

    #[test]
    fn commit_uses_paste_fallback_when_accessibility_write_fails() {
        let mut session = FakeCommitSession::with_destination("destination_textedit_0001")
            .with_accessibility_write_result(Err(CommitError::AccessibilityWriteFailed));
        let mut clipboard =
            FakeClipboardProvider::new(Ok(FakeClipboardSession::new("previous clipboard")));

        let report = commit_confirmed_text_with_clipboard_provider(
            "destination_textedit_0001",
            "confirmed text",
            &mut session,
            &mut clipboard,
        )
        .expect("fallback should handle direct write failure");

        assert_eq!(report.method, CommitMethod::SimulatedPaste);
    }

    #[test]
    fn commit_releases_destination_after_paste_fallback_failure() {
        let mut session = FakeCommitSession::with_destination("destination_textedit_0001")
            .with_accessibility_write_result(Err(CommitError::AccessibilityWriteFailed));
        let mut clipboard =
            FakeClipboardProvider::new(Ok(FakeClipboardSession::new("previous clipboard")
                .with_paste_result(Err(CommitError::PasteSimulationFailed))));

        let result = commit_confirmed_text_with_clipboard_provider(
            "destination_textedit_0001",
            "confirmed text",
            &mut session,
            &mut clipboard,
        );

        assert_eq!(
            (result, session.has_destination("destination_textedit_0001")),
            (Err(CommitError::PasteSimulationFailed), false)
        );
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

    #[test]
    fn paste_fallback_restores_plain_text_clipboard_after_paste_failure() {
        let mut clipboard = FakeClipboardSession::new("previous clipboard")
            .with_paste_result(Err(CommitError::PasteSimulationFailed));

        let result = simulated_paste_fallback(
            "destination_textedit_0001",
            "confirmed text",
            &mut clipboard,
        );

        assert_eq!(
            (result, clipboard.text),
            (
                Err(CommitError::PasteSimulationFailed),
                "previous clipboard".to_string()
            )
        );
    }

    #[test]
    fn paste_fallback_restores_plain_text_clipboard_after_success() {
        let mut clipboard = FakeClipboardSession::new("previous clipboard");

        let result = simulated_paste_fallback(
            "destination_textedit_0001",
            "confirmed text",
            &mut clipboard,
        );

        assert_eq!(
            (result.map(|report| report.method), clipboard.text),
            (
                Ok(CommitMethod::SimulatedPaste),
                "previous clipboard".to_string()
            )
        );
    }

    #[test]
    fn paste_fallback_refuses_unknown_clipboard_type() {
        let mut clipboard = FakeClipboardSession::new("previous clipboard")
            .with_snapshot_result(Err(CommitError::UnsupportedClipboardContent));

        let result = simulated_paste_fallback(
            "destination_textedit_0001",
            "confirmed text",
            &mut clipboard,
        );

        assert_eq!(
            (result, clipboard.text),
            (
                Err(CommitError::UnsupportedClipboardContent),
                "previous clipboard".to_string()
            )
        );
    }

    #[derive(Clone)]
    struct FakeClipboardSession {
        text: String,
        snapshot_result: Result<ClipboardSnapshot, CommitError>,
        paste_result: Result<(), CommitError>,
    }

    impl FakeClipboardSession {
        fn new(text: &str) -> Self {
            Self {
                text: text.to_string(),
                snapshot_result: Ok(ClipboardSnapshot::PlainText(text.to_string())),
                paste_result: Ok(()),
            }
        }

        fn with_snapshot_result(mut self, result: Result<ClipboardSnapshot, CommitError>) -> Self {
            self.snapshot_result = result;
            self
        }

        fn with_paste_result(mut self, result: Result<(), CommitError>) -> Self {
            self.paste_result = result;
            self
        }
    }

    impl ClipboardSession for FakeClipboardSession {
        fn snapshot(&mut self) -> Result<ClipboardSnapshot, CommitError> {
            self.snapshot_result.clone()
        }

        fn set_plain_text(&mut self, text: &str) -> Result<(), CommitError> {
            self.text = text.to_string();
            Ok(())
        }

        fn simulate_paste(&mut self) -> Result<(), CommitError> {
            self.paste_result.clone()
        }

        fn restore(&mut self, snapshot: ClipboardSnapshot) -> Result<(), CommitError> {
            match snapshot {
                ClipboardSnapshot::PlainText(text) => {
                    self.text = text;
                }
            }
            Ok(())
        }
    }

    struct FakeCommitSession {
        destination_id: Option<String>,
        accessibility_write_result: Result<(), CommitError>,
        restore_count: usize,
    }

    impl Default for FakeCommitSession {
        fn default() -> Self {
            Self {
                destination_id: None,
                accessibility_write_result: Ok(()),
                restore_count: 0,
            }
        }
    }

    impl FakeCommitSession {
        fn with_destination(destination_id: &str) -> Self {
            Self {
                destination_id: Some(destination_id.to_string()),
                accessibility_write_result: Ok(()),
                restore_count: 0,
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

        fn restore_destination(&mut self, destination_id: &str) -> Result<(), CommitError> {
            if self.has_destination(destination_id) {
                self.restore_count += 1;
                Ok(())
            } else {
                Err(CommitError::UnknownDestination)
            }
        }

        fn release_destination(&mut self, destination_id: &str) {
            if self.has_destination(destination_id) {
                self.destination_id = None;
            }
        }
    }

    struct FakeClipboardProvider {
        open_result: Result<FakeClipboardSession, CommitError>,
        open_count: usize,
    }

    impl FakeClipboardProvider {
        fn new(open_result: Result<FakeClipboardSession, CommitError>) -> Self {
            Self {
                open_result,
                open_count: 0,
            }
        }
    }

    impl ClipboardProvider for FakeClipboardProvider {
        type Clipboard = FakeClipboardSession;

        fn open(&mut self) -> Result<Self::Clipboard, CommitError> {
            self.open_count += 1;
            self.open_result.clone()
        }
    }
}
