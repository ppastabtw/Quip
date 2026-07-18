//! Workstream 3: Accessibility permission detection, focused editable element
//! recognition, secure-field exclusion, burst capture, and bounded window text.
//!
//! Produces `quip_contracts::CaptureResult` for the composition layer. Element
//! handles, insertion markers, and restoration state stay internal here; only
//! the opaque `destination_id` crosses the boundary.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use axuielement::ax_attribute::attributes::{
    AX_FOCUSED_ATTRIBUTE, AX_IS_EDITABLE_ATTRIBUTE, AX_ROLE_ATTRIBUTE, AX_SELECTED_TEXT_ATTRIBUTE,
    AX_SELECTED_TEXT_RANGE_ATTRIBUTE, AX_SUBROLE_ATTRIBUTE, AX_TITLE_ATTRIBUTE, AX_VALUE_ATTRIBUTE,
};
use axuielement::ax_attribute::subroles::AX_SECURE_TEXT_FIELD_SUBROLE;
use axuielement::ax_value::AXRange;
use axuielement::AXUIElement;
use quip_contracts::{CaptureResult, ContextSnippet, Trigger};

const DRAFT_MAX_CHARS: usize = 80;

#[allow(dead_code)]
const TEXTEDIT_BUNDLE_ID: &str = "com.apple.TextEdit";
#[allow(dead_code)]
const TEXTEDIT_APP_NAME: &str = "TextEdit";

#[allow(dead_code)]
static DESTINATION_REGISTRY: OnceLock<Mutex<DestinationRegistry>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AccessibilityPermissionStatus {
    Trusted,
    NotTrusted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AccessibilityError {
    PermissionMissing,
    DestinationNotFound,
    DestinationRegistryUnavailable,
    UnsupportedApp,
    SecureField,
    NotEditable,
    CommitFailed,
}

impl AccessibilityError {
    #[allow(dead_code)]
    fn unavailable_reason(&self) -> &'static str {
        match self {
            Self::PermissionMissing => "accessibility_permission_missing",
            Self::DestinationNotFound => "destination_not_found",
            Self::DestinationRegistryUnavailable => "destination_registry_unavailable",
            Self::UnsupportedApp => "unsupported_app",
            Self::SecureField => "secure_field",
            Self::NotEditable => "not_editable",
            Self::CommitFailed => "commit_failed",
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DestinationSnapshot {
    app_element: Option<AXUIElement>,
    element: Option<AXUIElement>,
    pid: Option<i32>,
    app_bundle_id: String,
    app_name: String,
    role: String,
    selected_range_utf16: Range<usize>,
    insertion_range_utf16: Range<usize>,
}

impl DestinationSnapshot {
    #[allow(dead_code)]
    fn from_focused_textedit(focused: &FocusedEditableElement) -> Self {
        Self {
            app_element: focused.app_element.clone(),
            element: Some(focused.element.clone()),
            pid: focused.element.pid().ok(),
            app_bundle_id: focused.app_id.clone(),
            app_name: focused.app_name.clone(),
            role: focused.snapshot.role.clone(),
            selected_range_utf16: focused.snapshot.selected_range_utf16.clone(),
            insertion_range_utf16: focused.snapshot.selected_range_utf16.end
                ..focused.snapshot.selected_range_utf16.end,
        }
    }

    #[cfg(test)]
    fn new_for_test(app_bundle_id: &str, app_name: &str, role: &str) -> Self {
        Self {
            app_element: None,
            element: None,
            pid: None,
            app_bundle_id: app_bundle_id.to_string(),
            app_name: app_name.to_string(),
            role: role.to_string(),
            selected_range_utf16: 0..0,
            insertion_range_utf16: 0..0,
        }
    }

    fn restore_focus_and_range(&self) -> Result<(), AccessibilityError> {
        if let Some(app_element) = &self.app_element {
            let _ = app_element.set_bool_attribute(AX_FOCUSED_ATTRIBUTE, true);
        }

        let element = self
            .element
            .as_ref()
            .ok_or(AccessibilityError::DestinationNotFound)?;
        element
            .set_bool_attribute(AX_FOCUSED_ATTRIBUTE, true)
            .map_err(|_| AccessibilityError::CommitFailed)?;
        element
            .set_range_attribute(
                AX_SELECTED_TEXT_RANGE_ATTRIBUTE,
                range_to_ax_range(&self.selected_range_utf16),
            )
            .map_err(|_| AccessibilityError::CommitFailed)
    }

    fn write_confirmed_text(&self, confirmed_text: &str) -> Result<(), AccessibilityError> {
        self.restore_focus_and_range()?;
        let element = self
            .element
            .as_ref()
            .ok_or(AccessibilityError::DestinationNotFound)?;
        element
            .set_string_attribute(AX_SELECTED_TEXT_ATTRIBUTE, confirmed_text)
            .map_err(|_| AccessibilityError::CommitFailed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct FocusedElementSnapshot {
    role: String,
    subrole: Option<String>,
    is_editable: Option<bool>,
    selected_range_utf16: Range<usize>,
}

impl FocusedElementSnapshot {
    fn from_ax_element(element: &AXUIElement) -> Result<Self, AccessibilityError> {
        let role = element
            .string_attribute(AX_ROLE_ATTRIBUTE)
            .map_err(|_| AccessibilityError::NotEditable)?
            .ok_or(AccessibilityError::NotEditable)?;
        let subrole = element
            .string_attribute(AX_SUBROLE_ATTRIBUTE)
            .map_err(|_| AccessibilityError::NotEditable)?;
        let is_editable = element
            .bool_attribute(AX_IS_EDITABLE_ATTRIBUTE)
            .map_err(|_| AccessibilityError::NotEditable)?
            .or_else(|| element.is_attribute_settable(AX_VALUE_ATTRIBUTE).ok());
        let selected_range_utf16 = element
            .range_attribute(AX_SELECTED_TEXT_RANGE_ATTRIBUTE)
            .ok()
            .flatten()
            .and_then(ax_range_to_range)
            .unwrap_or(0..0);

        Ok(Self {
            role,
            subrole,
            is_editable,
            selected_range_utf16,
        })
    }

    #[cfg(test)]
    fn new_for_test(role: &str, subrole: Option<&str>, is_editable: Option<bool>) -> Self {
        Self {
            role: role.to_string(),
            subrole: subrole.map(str::to_string),
            is_editable,
            selected_range_utf16: 0..0,
        }
    }
}

#[derive(Debug, Default)]
#[allow(dead_code)]
struct DestinationRegistry {
    next_id: u64,
    snapshots: HashMap<String, DestinationSnapshot>,
}

#[allow(dead_code)]
impl DestinationRegistry {
    fn register(&mut self, snapshot: DestinationSnapshot) -> String {
        self.next_id += 1;
        let destination_id = format!("destination_{:04}", self.next_id);
        self.snapshots.insert(destination_id.clone(), snapshot);
        destination_id
    }

    fn release(&mut self, destination_id: &str) -> Result<(), AccessibilityError> {
        self.snapshots
            .remove(destination_id)
            .map(|_| ())
            .ok_or(AccessibilityError::DestinationNotFound)
    }
}

#[allow(dead_code)]
fn build_ready_capture_result(
    registry: &mut DestinationRegistry,
    profile_id: &str,
    trigger: Trigger,
    draft: &str,
    snapshot: DestinationSnapshot,
) -> CaptureResult {
    let destination_id = registry.register(snapshot);
    CaptureResult::Ready {
        burst_id: format!("burst_{destination_id}"),
        destination_id,
        profile_id: profile_id.to_string(),
        draft: draft.to_string(),
        trigger,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ContextSnippetLimit {
    pub max_snippets: usize,
    pub max_chars_per_snippet: usize,
}

#[allow(dead_code)]
fn bound_context_snippets(
    snippets: Vec<ContextSnippet>,
    limit: ContextSnippetLimit,
) -> Vec<ContextSnippet> {
    snippets
        .into_iter()
        .take(limit.max_snippets)
        .map(|mut snippet| {
            snippet.visible_text = snippet
                .visible_text
                .chars()
                .take(limit.max_chars_per_snippet)
                .collect();
            snippet
        })
        .collect()
}

#[allow(dead_code)]
pub fn collect_context_snippets(limit: ContextSnippetLimit) -> Vec<ContextSnippet> {
    bound_context_snippets(Vec::new(), limit)
}

#[allow(dead_code)]
pub fn accessibility_permission_status() -> AccessibilityPermissionStatus {
    if axuielement::is_process_trusted() {
        AccessibilityPermissionStatus::Trusted
    } else {
        AccessibilityPermissionStatus::NotTrusted
    }
}

#[allow(dead_code)]
fn is_supported_app(bundle_id: &str) -> bool {
    matches!(
        bundle_id,
        "com.apple.TextEdit"
            | "com.apple.Notes"
            | "com.vivaldi.Vivaldi"
            | "com.google.Chrome"
            | "com.apple.Safari"
    )
}

#[allow(dead_code)]
fn capture_preflight(
    permission_status: AccessibilityPermissionStatus,
    bundle_id: &str,
) -> Result<(), AccessibilityError> {
    if permission_status == AccessibilityPermissionStatus::NotTrusted {
        return Err(AccessibilityError::PermissionMissing);
    }

    if !is_supported_app(bundle_id) {
        return Err(AccessibilityError::UnsupportedApp);
    }

    Ok(())
}

#[allow(dead_code)]
struct FocusedEditableElement {
    app_element: Option<AXUIElement>,
    app_id: String,
    app_name: String,
    element: AXUIElement,
    snapshot: FocusedElementSnapshot,
}

#[allow(dead_code)]
fn validate_focused_editable_element(
    app_id: &str,
    focused: &FocusedElementSnapshot,
) -> Result<(), AccessibilityError> {
    capture_preflight(AccessibilityPermissionStatus::Trusted, app_id)?;

    if focused.subrole.as_deref() == Some(AX_SECURE_TEXT_FIELD_SUBROLE) {
        return Err(AccessibilityError::SecureField);
    }

    if is_supported_app(app_id)
        && matches!(focused.role.as_str(), "AXTextArea" | "AXTextField")
        && focused.is_editable.unwrap_or(false)
    {
        return Ok(());
    }

    Err(AccessibilityError::NotEditable)
}

#[allow(dead_code)]
fn focused_editable_element() -> Result<FocusedEditableElement, AccessibilityError> {
    let system = axuielement::SystemWideElement::new().ok_or(AccessibilityError::NotEditable)?;
    let app_element = system
        .focused_application()
        .map_err(|_| AccessibilityError::NotEditable)?;
    let (app_id, app_name) = app_element
        .as_ref()
        .and_then(focused_app_identity)
        .ok_or(AccessibilityError::UnsupportedApp)?;

    capture_preflight(accessibility_permission_status(), &app_id)?;
    let element = system
        .focused_ui_element()
        .map_err(|_| AccessibilityError::NotEditable)?
        .ok_or(AccessibilityError::NotEditable)?;
    let snapshot = FocusedElementSnapshot::from_ax_element(&element)?;

    validate_focused_editable_element(&app_id, &snapshot)?;

    Ok(FocusedEditableElement {
        app_element,
        app_id,
        app_name,
        element,
        snapshot,
    })
}

#[allow(dead_code)]
fn destination_registry() -> &'static Mutex<DestinationRegistry> {
    DESTINATION_REGISTRY.get_or_init(|| Mutex::new(DestinationRegistry::default()))
}

#[allow(dead_code)]
fn draft_from_element(element: &AXUIElement) -> String {
    bound_draft(
        &element
            .string_attribute(AX_SELECTED_TEXT_ATTRIBUTE)
            .ok()
            .flatten()
            .filter(|text| !text.is_empty())
            .or_else(|| element.string_attribute(AX_VALUE_ATTRIBUTE).ok().flatten())
            .unwrap_or_default(),
    )
}

#[allow(dead_code)]
pub fn capture_focused_destination(profile_id: &str, trigger: Trigger) -> CaptureResult {
    let focused = match focused_editable_element() {
        Ok(focused) => focused,
        Err(error) => {
            return CaptureResult::Unavailable {
                reason: error.unavailable_reason().to_string(),
            };
        }
    };
    let draft = draft_from_element(&focused.element);
    let snapshot = DestinationSnapshot::from_focused_textedit(&focused);

    let Ok(mut registry) = destination_registry().lock() else {
        return CaptureResult::Unavailable {
            reason: "destination_registry_unavailable".to_string(),
        };
    };

    build_ready_capture_result(&mut registry, profile_id, trigger, &draft, snapshot)
}

#[allow(dead_code)]
pub(crate) fn destination_exists(destination_id: &str) -> bool {
    destination_registry()
        .lock()
        .map(|registry| registry.snapshots.contains_key(destination_id))
        .unwrap_or(false)
}

#[allow(dead_code)]
pub(crate) fn write_confirmed_text_to_destination(
    destination_id: &str,
    confirmed_text: &str,
) -> Result<(), AccessibilityError> {
    let snapshot = destination_registry()
        .lock()
        .map_err(|_| AccessibilityError::DestinationRegistryUnavailable)?
        .snapshots
        .get(destination_id)
        .cloned()
        .ok_or(AccessibilityError::DestinationNotFound)?;

    snapshot.write_confirmed_text(confirmed_text)
}

#[allow(dead_code)]
pub(crate) fn restore_destination(destination_id: &str) -> Result<(), AccessibilityError> {
    let snapshot = destination_registry()
        .lock()
        .map_err(|_| AccessibilityError::DestinationRegistryUnavailable)?
        .snapshots
        .get(destination_id)
        .cloned()
        .ok_or(AccessibilityError::DestinationNotFound)?;

    snapshot.restore_focus_and_range()
}

#[allow(dead_code)]
pub fn release_destination(destination_id: &str) -> Result<(), AccessibilityError> {
    destination_registry()
        .lock()
        .map_err(|_| AccessibilityError::DestinationRegistryUnavailable)?
        .release(destination_id)
}

#[allow(dead_code)]
pub(crate) fn cancel_destination(destination_id: &str) -> Result<(), AccessibilityError> {
    restore_destination(destination_id)?;
    release_destination(destination_id)
}

fn focused_app_identity(app_element: &AXUIElement) -> Option<(String, String)> {
    app_element
        .string_attribute(AX_TITLE_ATTRIBUTE)
        .ok()
        .flatten()
        .and_then(|title| match title.as_str() {
            TEXTEDIT_APP_NAME => Some((TEXTEDIT_BUNDLE_ID.to_string(), title)),
            "Notes" => Some(("com.apple.Notes".to_string(), title)),
            "Vivaldi" => Some(("com.vivaldi.Vivaldi".to_string(), title)),
            "Google Chrome" => Some(("com.google.Chrome".to_string(), title)),
            "Safari" => Some(("com.apple.Safari".to_string(), title)),
            _ => None,
        })
}

fn ax_range_to_range(range: AXRange) -> Option<Range<usize>> {
    let start = usize::try_from(range.location).ok()?;
    let length = usize::try_from(range.length).ok()?;
    Some(start..start.saturating_add(length))
}

fn range_to_ax_range(range: &Range<usize>) -> AXRange {
    AXRange {
        location: isize::try_from(range.start).unwrap_or(isize::MAX),
        length: isize::try_from(range.end.saturating_sub(range.start)).unwrap_or(isize::MAX),
    }
}

fn bound_draft(draft: &str) -> String {
    draft
        .chars()
        .rev()
        .take(DRAFT_MAX_CHARS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        bound_context_snippets, bound_draft, build_ready_capture_result, capture_preflight,
        collect_context_snippets, is_supported_app, validate_focused_editable_element,
        AccessibilityError, AccessibilityPermissionStatus, CaptureResult, ContextSnippet,
        ContextSnippetLimit, DestinationRegistry, DestinationSnapshot, FocusedElementSnapshot,
    };
    use quip_contracts::Trigger;

    #[test]
    fn destination_registry_returns_opaque_destination_id() {
        let _permission_statuses = [
            AccessibilityPermissionStatus::Trusted,
            AccessibilityPermissionStatus::NotTrusted,
        ];
        let _errors = [
            AccessibilityError::PermissionMissing,
            AccessibilityError::DestinationNotFound,
            AccessibilityError::DestinationRegistryUnavailable,
            AccessibilityError::UnsupportedApp,
            AccessibilityError::SecureField,
            AccessibilityError::NotEditable,
            AccessibilityError::CommitFailed,
        ];
        let _snippet_limit = ContextSnippetLimit {
            max_snippets: 3,
            max_chars_per_snippet: 240,
        };
        let mut registry = DestinationRegistry::default();
        let destination_id = registry.register(DestinationSnapshot::new_for_test(
            "com.apple.TextEdit",
            "TextEdit",
            "AXTextArea",
        ));

        assert!(destination_id.starts_with("destination_"));
    }

    #[test]
    fn destination_registry_release_removes_snapshot() {
        let mut registry = DestinationRegistry::default();
        let destination_id = registry.register(DestinationSnapshot::new_for_test(
            "com.apple.TextEdit",
            "TextEdit",
            "AXTextArea",
        ));

        assert_eq!(registry.release(&destination_id), Ok(()));
    }

    #[test]
    fn destination_registry_release_rejects_unknown_destination_id() {
        let mut registry = DestinationRegistry::default();

        assert_eq!(
            registry.release("destination_missing"),
            Err(AccessibilityError::DestinationNotFound)
        );
    }

    #[test]
    fn capture_ready_result_stores_destination_snapshot() {
        let mut registry = DestinationRegistry::default();
        let capture = build_ready_capture_result(
            &mut registry,
            "profile_default",
            Trigger::Idle,
            "cnt cm tmrw",
            DestinationSnapshot::new_for_test("com.apple.TextEdit", "TextEdit", "AXTextArea"),
        );

        let CaptureResult::Ready { destination_id, .. } = capture else {
            panic!("expected ready capture result");
        };
        assert_eq!(registry.release(&destination_id), Ok(()));
    }

    #[test]
    fn context_snippet_limit_bounds_snippet_count() {
        let snippets = vec![
            ContextSnippet {
                app_name: "TextEdit".to_string(),
                window_title: "First".to_string(),
                visible_text: "one".to_string(),
            },
            ContextSnippet {
                app_name: "Notes".to_string(),
                window_title: "Second".to_string(),
                visible_text: "two".to_string(),
            },
        ];

        assert_eq!(
            bound_context_snippets(
                snippets,
                ContextSnippetLimit {
                    max_snippets: 1,
                    max_chars_per_snippet: 20,
                },
            )
            .len(),
            1
        );
    }

    #[test]
    fn context_snippet_limit_bounds_visible_text_chars() {
        let snippets = vec![ContextSnippet {
            app_name: "TextEdit".to_string(),
            window_title: "Long".to_string(),
            visible_text: "abcdef".to_string(),
        }];

        assert_eq!(
            bound_context_snippets(
                snippets,
                ContextSnippetLimit {
                    max_snippets: 1,
                    max_chars_per_snippet: 3,
                },
            )[0]
            .visible_text,
            "abc"
        );
    }

    #[test]
    fn collect_context_snippets_applies_count_and_text_bounds() {
        let snippets = collect_context_snippets(ContextSnippetLimit {
            max_snippets: 1,
            max_chars_per_snippet: 4,
        });

        assert!(
            snippets.len() <= 1
                && snippets
                    .iter()
                    .all(|snippet| snippet.visible_text.len() <= 4)
        );
    }

    #[test]
    fn draft_from_value_is_bounded_to_last_eighty_chars() {
        let draft = format!("{}{}", "a".repeat(30), "b".repeat(80));

        assert_eq!(bound_draft(&draft), "b".repeat(80));
    }

    #[test]
    fn supported_app_allows_textedit_notes_vivaldi_chrome_and_safari() {
        let allowed = [
            "com.apple.TextEdit",
            "com.apple.Notes",
            "com.vivaldi.Vivaldi",
            "com.google.Chrome",
            "com.apple.Safari",
        ];

        assert!(allowed.iter().all(|bundle_id| is_supported_app(bundle_id)));
    }

    #[test]
    fn supported_app_rejects_out_of_scope_apps() {
        let rejected = [
            "com.apple.Terminal",
            "com.microsoft.VSCode",
            "com.apple.finder",
            "org.mozilla.firefox",
        ];

        assert!(rejected
            .iter()
            .all(|bundle_id| !is_supported_app(bundle_id)));
    }

    #[test]
    fn secure_field_maps_to_unavailable_secure_field() {
        assert_eq!(
            AccessibilityError::SecureField.unavailable_reason(),
            "secure_field"
        );
    }

    #[test]
    fn missing_editable_element_maps_to_unavailable_not_editable() {
        assert_eq!(
            AccessibilityError::NotEditable.unavailable_reason(),
            "not_editable"
        );
    }

    #[test]
    fn preflight_rejects_missing_process_trust_before_app_gate() {
        assert_eq!(
            capture_preflight(
                AccessibilityPermissionStatus::NotTrusted,
                "com.apple.TextEdit"
            ),
            Err(AccessibilityError::PermissionMissing)
        );
    }

    #[test]
    fn preflight_rejects_unsupported_app_when_process_is_trusted() {
        assert_eq!(
            capture_preflight(AccessibilityPermissionStatus::Trusted, "com.apple.Terminal"),
            Err(AccessibilityError::UnsupportedApp)
        );
    }

    #[test]
    fn preflight_allows_supported_app_when_process_is_trusted() {
        assert_eq!(
            capture_preflight(
                AccessibilityPermissionStatus::Trusted,
                "com.vivaldi.Vivaldi"
            ),
            Ok(())
        );
    }

    #[test]
    fn textedit_focused_text_area_is_editable_destination() {
        let focused = FocusedElementSnapshot::new_for_test("AXTextArea", None, Some(true));

        assert_eq!(
            validate_focused_editable_element("com.apple.TextEdit", &focused),
            Ok(())
        );
    }

    #[test]
    fn textedit_non_editable_text_area_is_not_editable() {
        let focused = FocusedElementSnapshot::new_for_test("AXTextArea", None, Some(false));

        assert_eq!(
            validate_focused_editable_element("com.apple.TextEdit", &focused),
            Err(AccessibilityError::NotEditable)
        );
    }

    #[test]
    fn textedit_button_focus_is_not_editable() {
        let focused = FocusedElementSnapshot::new_for_test("AXButton", None, Some(true));

        assert_eq!(
            validate_focused_editable_element("com.apple.TextEdit", &focused),
            Err(AccessibilityError::NotEditable)
        );
    }

    #[test]
    fn secure_text_field_focus_is_secure_field() {
        let focused = FocusedElementSnapshot::new_for_test(
            "AXTextField",
            Some("AXSecureTextField"),
            Some(true),
        );

        assert_eq!(
            validate_focused_editable_element("com.apple.TextEdit", &focused),
            Err(AccessibilityError::SecureField)
        );
    }
}
