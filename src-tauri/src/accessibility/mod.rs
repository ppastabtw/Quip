//! Workstream 3: Accessibility permission detection, focused editable element
//! recognition, secure-field exclusion, destination capture, and commit restore.
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
    AX_VISIBLE_TEXT_ATTRIBUTE, AX_WINDOW_ATTRIBUTE,
};
use axuielement::ax_attribute::parameterized::AX_BOUNDS_FOR_RANGE_PARAMETERIZED_ATTRIBUTE;
use axuielement::ax_attribute::subroles::AX_SECURE_TEXT_FIELD_SUBROLE;
use axuielement::ax_value::{AXRange, AXRect, AXValue};
use axuielement::AXUIElement;
use objc2_app_kit::NSRunningApplication;
use quip_contracts::{CaptureResult, ContextSnippet, Rect, Trigger};
use serde::Serialize;

const DRAFT_MAX_CHARS: usize = 80;
pub(crate) const DEFAULT_CONTEXT_LIMIT: ContextSnippetLimit = ContextSnippetLimit {
    max_snippets: 1,
    max_chars_per_snippet: 240,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FocusedElementDiagnostic {
    pub permission: &'static str,
    pub app_bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub role: Option<String>,
    pub subrole: Option<String>,
    pub is_editable: Option<bool>,
    pub selected_range_available: bool,
}

#[allow(dead_code)]
const TEXTEDIT_BUNDLE_ID: &str = "com.apple.TextEdit";
#[allow(dead_code)]
const NOTES_BUNDLE_ID: &str = "com.apple.Notes";
#[allow(dead_code)]
const VIVALDI_BUNDLE_ID: &str = "com.vivaldi.Vivaldi";
#[allow(dead_code)]
const CHROME_BUNDLE_ID: &str = "com.google.Chrome";
#[allow(dead_code)]
const SAFARI_BUNDLE_ID: &str = "com.apple.Safari";
#[allow(dead_code)]
const TEXTEDIT_APP_NAME: &str = "TextEdit";
#[allow(dead_code)]
const NOTES_APP_NAME: &str = "Notes";
#[allow(dead_code)]
const VIVALDI_APP_NAME: &str = "Vivaldi";
#[allow(dead_code)]
const CHROME_APP_NAME: &str = "Google Chrome";
#[allow(dead_code)]
const SAFARI_APP_NAME: &str = "Safari";
#[allow(dead_code)]
const AX_TEXT_AREA_ROLE: &str = "AXTextArea";
#[allow(dead_code)]
const AX_TEXT_FIELD_ROLE: &str = "AXTextField";
#[allow(dead_code)]
const SUPPORTED_APP_BUNDLE_IDS: [&str; 5] = [
    TEXTEDIT_BUNDLE_ID,
    NOTES_BUNDLE_ID,
    VIVALDI_BUNDLE_ID,
    CHROME_BUNDLE_ID,
    SAFARI_BUNDLE_ID,
];

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
    CaretUnavailable,
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
            Self::CaretUnavailable => "caret_unavailable",
            Self::CommitFailed => "commit_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct CapturedDraft {
    text: String,
    burst_range_utf16: Range<usize>,
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
    burst_range_utf16: Range<usize>,
    selected_range_utf16: Range<usize>,
    insertion_range_utf16: Range<usize>,
}

impl DestinationSnapshot {
    #[allow(dead_code)]
    fn from_focused_editable_element(focused: &FocusedEditableElement) -> Self {
        Self {
            app_element: focused.app_element.clone(),
            element: Some(focused.element.clone()),
            pid: focused.element.pid().ok(),
            app_bundle_id: focused.app_bundle_id.clone(),
            app_name: focused.app_name.clone(),
            role: focused.snapshot.role.clone(),
            burst_range_utf16: focused.captured_draft.burst_range_utf16.clone(),
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
            burst_range_utf16: 0..0,
            selected_range_utf16: 0..0,
            insertion_range_utf16: 0..0,
        }
    }

    fn restore_focus_and_range(&self, range: &Range<usize>) -> Result<(), AccessibilityError> {
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
            .set_range_attribute(AX_SELECTED_TEXT_RANGE_ATTRIBUTE, range_to_ax_range(range))
            .map_err(|_| AccessibilityError::CommitFailed)
    }

    fn restore_original_focus_and_range(&self) -> Result<(), AccessibilityError> {
        self.restore_focus_and_range(&self.selected_range_utf16)
    }

    fn restore_burst_focus_and_range(&self) -> Result<(), AccessibilityError> {
        self.restore_focus_and_range(&self.burst_range_utf16)
    }

    fn write_confirmed_text(&self, confirmed_text: &str) -> Result<(), AccessibilityError> {
        self.restore_burst_focus_and_range()?;
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
    has_selected_range: bool,
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
            .and_then(ax_range_to_range);

        Ok(Self {
            role,
            subrole,
            is_editable,
            selected_range_utf16: selected_range_utf16.clone().unwrap_or(0..0),
            has_selected_range: selected_range_utf16.is_some(),
        })
    }

    #[cfg(test)]
    fn new_for_test(role: &str, subrole: Option<&str>, is_editable: Option<bool>) -> Self {
        Self {
            role: role.to_string(),
            subrole: subrole.map(str::to_string),
            is_editable,
            selected_range_utf16: 0..0,
            has_selected_range: true,
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
    caret: Rect,
    snapshot: DestinationSnapshot,
) -> CaptureResult {
    let destination_id = registry.register(snapshot);
    CaptureResult::Ready {
        burst_id: format!("burst_{destination_id}"),
        destination_id,
        profile_id: profile_id.to_string(),
        draft: draft.to_string(),
        trigger,
        caret,
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
    focused_context_snippet(limit).into_iter().collect()
}

#[allow(dead_code)]
pub fn accessibility_permission_status() -> AccessibilityPermissionStatus {
    if axuielement::is_process_trusted() {
        AccessibilityPermissionStatus::Trusted
    } else {
        AccessibilityPermissionStatus::NotTrusted
    }
}

pub fn focused_element_diagnostic() -> FocusedElementDiagnostic {
    let permission_status = accessibility_permission_status();
    let mut diagnostic = FocusedElementDiagnostic {
        permission: match permission_status {
            AccessibilityPermissionStatus::Trusted => "trusted",
            AccessibilityPermissionStatus::NotTrusted => "not_trusted",
        },
        ..FocusedElementDiagnostic::default()
    };

    let Some(system) = axuielement::SystemWideElement::new() else {
        return diagnostic;
    };
    if let Some(app_element) = system.focused_application().ok().flatten() {
        if let Some((bundle_id, app_name)) = focused_app_identity(&app_element) {
            diagnostic.app_bundle_id = Some(bundle_id);
            diagnostic.app_name = Some(app_name);
        }
    }
    if permission_status == AccessibilityPermissionStatus::NotTrusted {
        return diagnostic;
    }
    if let Some(element) = system.focused_ui_element().ok().flatten() {
        if let Ok(snapshot) = FocusedElementSnapshot::from_ax_element(&element) {
            diagnostic.role = Some(snapshot.role);
            diagnostic.subrole = snapshot.subrole;
            diagnostic.is_editable = snapshot.is_editable;
            diagnostic.selected_range_available = snapshot.has_selected_range;
        }
    }

    diagnostic
}

#[allow(dead_code)]
fn is_supported_app(bundle_id: &str) -> bool {
    SUPPORTED_APP_BUNDLE_IDS.contains(&bundle_id)
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
    app_bundle_id: String,
    app_name: String,
    element: AXUIElement,
    snapshot: FocusedElementSnapshot,
    captured_draft: CapturedDraft,
    caret: Rect,
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
        && matches!(
            focused.role.as_str(),
            AX_TEXT_AREA_ROLE | AX_TEXT_FIELD_ROLE
        )
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
    let (app_bundle_id, app_name) = app_element
        .as_ref()
        .and_then(focused_app_identity)
        .ok_or(AccessibilityError::UnsupportedApp)?;

    capture_preflight(accessibility_permission_status(), &app_bundle_id)?;
    let element = system
        .focused_ui_element()
        .map_err(|_| AccessibilityError::NotEditable)?
        .ok_or(AccessibilityError::NotEditable)?;
    let snapshot = FocusedElementSnapshot::from_ax_element(&element)?;

    validate_focused_editable_element(&app_bundle_id, &snapshot)?;
    let captured_draft = captured_draft_from_element(&element, &snapshot);
    let caret = caret_rect_from_element(&element, &captured_draft.burst_range_utf16)?;

    Ok(FocusedEditableElement {
        app_element,
        app_bundle_id,
        app_name,
        element,
        snapshot,
        captured_draft,
        caret,
    })
}

#[allow(dead_code)]
fn destination_registry() -> &'static Mutex<DestinationRegistry> {
    DESTINATION_REGISTRY.get_or_init(|| Mutex::new(DestinationRegistry::default()))
}

#[allow(dead_code)]
fn draft_from_element(element: &AXUIElement) -> String {
    captured_draft_from_element(
        element,
        &FocusedElementSnapshot {
            role: String::new(),
            subrole: None,
            is_editable: None,
            selected_range_utf16: 0..0,
            has_selected_range: false,
        },
    )
    .text
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
    let draft = focused.captured_draft.text.clone();
    let caret = focused.caret;
    let snapshot = DestinationSnapshot::from_focused_editable_element(&focused);

    let Ok(mut registry) = destination_registry().lock() else {
        return CaptureResult::Unavailable {
            reason: "destination_registry_unavailable".to_string(),
        };
    };

    build_ready_capture_result(&mut registry, profile_id, trigger, &draft, caret, snapshot)
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

    snapshot.restore_burst_focus_and_range()
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
    let snapshot = destination_registry()
        .lock()
        .map_err(|_| AccessibilityError::DestinationRegistryUnavailable)?
        .snapshots
        .get(destination_id)
        .cloned()
        .ok_or(AccessibilityError::DestinationNotFound)?;

    snapshot.restore_original_focus_and_range()?;
    release_destination(destination_id)
}

fn focused_context_snippet(limit: ContextSnippetLimit) -> Option<ContextSnippet> {
    if accessibility_permission_status() != AccessibilityPermissionStatus::Trusted {
        return None;
    }

    let system = axuielement::SystemWideElement::new()?;
    let app_element = system.focused_application().ok().flatten()?;
    let (app_bundle_id, app_name) = focused_app_identity(&app_element)?;
    capture_preflight(AccessibilityPermissionStatus::Trusted, &app_bundle_id).ok()?;
    let element = system.focused_ui_element().ok().flatten()?;
    let snapshot = FocusedElementSnapshot::from_ax_element(&element).ok()?;
    validate_focused_editable_element(&app_bundle_id, &snapshot).ok()?;

    let window_title = system
        .focused_window()
        .ok()
        .flatten()
        .and_then(|window| window.string_attribute(AX_TITLE_ATTRIBUTE).ok().flatten())
        .or_else(|| {
            system
                .focused_ui_element()
                .ok()
                .flatten()
                .and_then(|element| {
                    element
                        .element_attribute(AX_WINDOW_ATTRIBUTE)
                        .ok()
                        .flatten()
                        .and_then(|window| {
                            window.string_attribute(AX_TITLE_ATTRIBUTE).ok().flatten()
                        })
                })
        })
        .unwrap_or_default();
    let visible_text = element
        .string_attribute(AX_VISIBLE_TEXT_ATTRIBUTE)
        .ok()
        .flatten()
        .or_else(|| element.string_attribute(AX_VALUE_ATTRIBUTE).ok().flatten())?;
    let snippets = bound_context_snippets(
        vec![ContextSnippet {
            app_name,
            window_title,
            visible_text,
        }],
        limit,
    );

    snippets.into_iter().next()
}

fn focused_app_identity(app_element: &AXUIElement) -> Option<(String, String)> {
    if let Some(identity) = app_identity_from_pid(app_element) {
        return Some(identity);
    }

    app_element
        .string_attribute(AX_TITLE_ATTRIBUTE)
        .ok()
        .flatten()
        .and_then(|title| match title.as_str() {
            TEXTEDIT_APP_NAME => Some((TEXTEDIT_BUNDLE_ID.to_string(), title)),
            NOTES_APP_NAME => Some((NOTES_BUNDLE_ID.to_string(), title)),
            VIVALDI_APP_NAME => Some((VIVALDI_BUNDLE_ID.to_string(), title)),
            CHROME_APP_NAME => Some((CHROME_BUNDLE_ID.to_string(), title)),
            SAFARI_APP_NAME => Some((SAFARI_BUNDLE_ID.to_string(), title)),
            _ => None,
        })
}

fn app_identity_from_pid(app_element: &AXUIElement) -> Option<(String, String)> {
    let pid = app_element.pid().ok()?;
    let running_app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    let app_id = running_app.bundleIdentifier()?.to_string();
    let app_name = running_app
        .localizedName()
        .map(|name| name.to_string())
        .unwrap_or_else(|| app_id.clone());

    Some((app_id, app_name))
}

fn ax_range_to_range(range: AXRange) -> Option<Range<usize>> {
    let start = usize::try_from(range.location).ok()?;
    let length = usize::try_from(range.length).ok()?;
    Some(start..start.saturating_add(length))
}

fn ax_rect_to_rect(rect: AXRect) -> Rect {
    Rect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

fn range_to_ax_range(range: &Range<usize>) -> AXRange {
    AXRange {
        location: isize::try_from(range.start).unwrap_or(isize::MAX),
        length: isize::try_from(range.end.saturating_sub(range.start)).unwrap_or(isize::MAX),
    }
}

fn bounds_for_range(element: &AXUIElement, range: &Range<usize>) -> Option<Rect> {
    let parameter = AXValue::from_range(range_to_ax_range(range))?;
    element
        .parameterized_attribute(AX_BOUNDS_FOR_RANGE_PARAMETERIZED_ATTRIBUTE, &parameter)
        .ok()
        .flatten()
        .and_then(|value| value.as_rect())
        .map(ax_rect_to_rect)
}

fn caret_rect_from_element(
    element: &AXUIElement,
    burst_range_utf16: &Range<usize>,
) -> Result<Rect, AccessibilityError> {
    let caret_position = burst_range_utf16.end;
    let caret_range = caret_position..caret_position;
    bounds_for_range(element, &caret_range)
        .or_else(|| bounds_for_range(element, burst_range_utf16))
        .ok_or(AccessibilityError::CaretUnavailable)
}

#[cfg(test)]
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

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn prefix_at_utf16(text: &str, end_utf16: usize) -> String {
    let mut used_utf16 = 0;
    let mut prefix = String::new();
    for ch in text.chars() {
        let next_used_utf16 = used_utf16 + ch.len_utf16();
        if next_used_utf16 > end_utf16 {
            break;
        }
        prefix.push(ch);
        used_utf16 = next_used_utf16;
    }
    prefix
}

fn bounded_suffix_with_range(text: &str, full_range_utf16: Range<usize>) -> CapturedDraft {
    let total_chars = text.chars().count();
    let skipped_chars = total_chars.saturating_sub(DRAFT_MAX_CHARS);
    let skipped_utf16 = text
        .chars()
        .take(skipped_chars)
        .map(char::len_utf16)
        .sum::<usize>();
    let bounded_text = text.chars().skip(skipped_chars).collect::<String>();

    CapturedDraft {
        text: bounded_text,
        burst_range_utf16: full_range_utf16.start + skipped_utf16..full_range_utf16.end,
    }
}

fn captured_draft_from_element(
    element: &AXUIElement,
    snapshot: &FocusedElementSnapshot,
) -> CapturedDraft {
    if let Some(selected_text) = element
        .string_attribute(AX_SELECTED_TEXT_ATTRIBUTE)
        .ok()
        .flatten()
        .filter(|text| !text.is_empty())
    {
        return bounded_suffix_with_range(&selected_text, snapshot.selected_range_utf16.clone());
    }

    let value = element
        .string_attribute(AX_VALUE_ATTRIBUTE)
        .ok()
        .flatten()
        .unwrap_or_default();
    let prefix = if snapshot.has_selected_range {
        prefix_at_utf16(&value, snapshot.selected_range_utf16.end)
    } else {
        value
    };
    let prefix_range = 0..utf16_len(&prefix);

    bounded_suffix_with_range(&prefix, prefix_range)
}

#[cfg(test)]
mod tests {
    use super::{
        bound_context_snippets, bound_draft, bounded_suffix_with_range, build_ready_capture_result,
        capture_preflight, collect_context_snippets, is_supported_app,
        validate_focused_editable_element, AccessibilityError, AccessibilityPermissionStatus,
        CaptureResult, ContextSnippet, ContextSnippetLimit, DestinationRegistry,
        DestinationSnapshot, FocusedElementSnapshot,
    };
    use quip_contracts::{Rect, Trigger};

    fn test_caret() -> Rect {
        Rect {
            x: 512.0,
            y: 384.0,
            width: 2.0,
            height: 18.0,
        }
    }

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
            AccessibilityError::CaretUnavailable,
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
            test_caret(),
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
                    .all(|snippet| snippet.visible_text.chars().count() <= 4)
        );
    }

    #[test]
    fn draft_from_value_is_bounded_to_last_eighty_chars() {
        let draft = format!("{}{}", "a".repeat(30), "b".repeat(80));

        assert_eq!(bound_draft(&draft), "b".repeat(80));
    }

    #[test]
    fn bounded_draft_tracks_matching_utf16_burst_range() {
        let two_unit_char = char::from_u32(0x1D11E).unwrap();
        let suffix = two_unit_char.to_string().repeat(80);
        let draft = format!("{}{}", "a".repeat(30), suffix);
        let captured = bounded_suffix_with_range(&draft, 10..200);

        assert_eq!(captured.text, two_unit_char.to_string().repeat(80));
        assert_eq!(captured.burst_range_utf16, 40..200);
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
