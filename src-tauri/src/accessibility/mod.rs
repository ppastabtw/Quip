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
    AX_IS_EDITABLE_ATTRIBUTE, AX_ROLE_ATTRIBUTE, AX_SELECTED_TEXT_ATTRIBUTE, AX_SUBROLE_ATTRIBUTE,
    AX_VALUE_ATTRIBUTE,
};
use axuielement::ax_attribute::subroles::AX_SECURE_TEXT_FIELD_SUBROLE;
use axuielement::AXUIElement;
use quip_contracts::{CaptureResult, ContextSnippet, Trigger};

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
    UnsupportedApp,
    SecureField,
    NotEditable,
}

impl AccessibilityError {
    #[allow(dead_code)]
    fn unavailable_reason(&self) -> &'static str {
        match self {
            Self::PermissionMissing => "accessibility_permission_missing",
            Self::DestinationNotFound => "destination_not_found",
            Self::UnsupportedApp => "unsupported_app",
            Self::SecureField => "secure_field",
            Self::NotEditable => "not_editable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct DestinationSnapshot {
    app_bundle_id: String,
    app_name: String,
    role: String,
    selected_range_utf16: Range<usize>,
    insertion_range_utf16: Range<usize>,
}

impl DestinationSnapshot {
    #[allow(dead_code)]
    fn from_focused_textedit(focused: &FocusedElementSnapshot) -> Self {
        Self {
            app_bundle_id: TEXTEDIT_BUNDLE_ID.to_string(),
            app_name: TEXTEDIT_APP_NAME.to_string(),
            role: focused.role.clone(),
            selected_range_utf16: 0..0,
            insertion_range_utf16: 0..0,
        }
    }

    #[cfg(test)]
    fn new_for_test(app_bundle_id: &str, app_name: &str, role: &str) -> Self {
        Self {
            app_bundle_id: app_bundle_id.to_string(),
            app_name: app_name.to_string(),
            role: role.to_string(),
            selected_range_utf16: 0..0,
            insertion_range_utf16: 0..0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct FocusedElementSnapshot {
    role: String,
    subrole: Option<String>,
    is_editable: Option<bool>,
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

        Ok(Self {
            role,
            subrole,
            is_editable,
        })
    }

    #[cfg(test)]
    fn new_for_test(role: &str, subrole: Option<&str>, is_editable: Option<bool>) -> Self {
        Self {
            role: role.to_string(),
            subrole: subrole.map(str::to_string),
            is_editable,
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
    snippets.into_iter().take(limit.max_snippets).collect()
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
    element: AXUIElement,
    snapshot: FocusedElementSnapshot,
}

#[allow(dead_code)]
fn validate_focused_editable_element(
    bundle_id: &str,
    focused: &FocusedElementSnapshot,
) -> Result<(), AccessibilityError> {
    capture_preflight(AccessibilityPermissionStatus::Trusted, bundle_id)?;

    if focused.subrole.as_deref() == Some(AX_SECURE_TEXT_FIELD_SUBROLE) {
        return Err(AccessibilityError::SecureField);
    }

    if bundle_id == "com.apple.TextEdit"
        && focused.role == "AXTextArea"
        && focused.is_editable.unwrap_or(false)
    {
        return Ok(());
    }

    Err(AccessibilityError::NotEditable)
}

#[allow(dead_code)]
fn focused_editable_textedit_element(
    bundle_id: &str,
) -> Result<FocusedEditableElement, AccessibilityError> {
    capture_preflight(accessibility_permission_status(), bundle_id)?;

    let system = axuielement::SystemWideElement::new().ok_or(AccessibilityError::NotEditable)?;
    let element = system
        .focused_ui_element()
        .map_err(|_| AccessibilityError::NotEditable)?
        .ok_or(AccessibilityError::NotEditable)?;
    let snapshot = FocusedElementSnapshot::from_ax_element(&element)?;

    validate_focused_editable_element(bundle_id, &snapshot)?;

    Ok(FocusedEditableElement { element, snapshot })
}

#[allow(dead_code)]
fn destination_registry() -> &'static Mutex<DestinationRegistry> {
    DESTINATION_REGISTRY.get_or_init(|| Mutex::new(DestinationRegistry::default()))
}

#[allow(dead_code)]
fn draft_from_element(element: &AXUIElement) -> String {
    element
        .string_attribute(AX_SELECTED_TEXT_ATTRIBUTE)
        .ok()
        .flatten()
        .filter(|text| !text.is_empty())
        .or_else(|| element.string_attribute(AX_VALUE_ATTRIBUTE).ok().flatten())
        .unwrap_or_default()
}

#[allow(dead_code)]
pub fn capture_focused_destination(profile_id: &str, trigger: Trigger) -> CaptureResult {
    let focused = match focused_editable_textedit_element(TEXTEDIT_BUNDLE_ID) {
        Ok(focused) => focused,
        Err(error) => {
            return CaptureResult::Unavailable {
                reason: error.unavailable_reason().to_string(),
            };
        }
    };
    let draft = draft_from_element(&focused.element);
    let snapshot = DestinationSnapshot::from_focused_textedit(&focused.snapshot);

    let Ok(mut registry) = destination_registry().lock() else {
        return CaptureResult::Unavailable {
            reason: "destination_registry_unavailable".to_string(),
        };
    };

    build_ready_capture_result(&mut registry, profile_id, trigger, &draft, snapshot)
}

#[cfg(test)]
mod tests {
    use super::{
        bound_context_snippets, build_ready_capture_result, capture_preflight, is_supported_app,
        validate_focused_editable_element, AccessibilityError, AccessibilityPermissionStatus,
        CaptureResult, ContextSnippet, ContextSnippetLimit, DestinationRegistry,
        DestinationSnapshot, FocusedElementSnapshot,
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
            AccessibilityError::UnsupportedApp,
            AccessibilityError::SecureField,
            AccessibilityError::NotEditable,
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
