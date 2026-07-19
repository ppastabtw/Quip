//! Workstream 3: Accessibility permission detection, focused editable element
//! recognition, secure-field exclusion, destination capture, and commit restore.
//!
//! Produces `quip_contracts::CaptureResult` for the composition layer. Element
//! handles, insertion markers, and restoration state stay internal here; only
//! the opaque `destination_id` crosses the boundary.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use axuielement::ax_attribute::attributes::{
    AX_CHILDREN_ATTRIBUTE, AX_FOCUSED_ATTRIBUTE, AX_FOCUSED_UI_ELEMENT_ATTRIBUTE,
    AX_IS_EDITABLE_ATTRIBUTE, AX_ROLE_ATTRIBUTE, AX_SELECTED_TEXT_ATTRIBUTE,
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
const DESCENDANT_SEARCH_LIMIT: usize = 64;
const CONTEXT_DESCENDANT_SEARCH_LIMIT: usize = 512;
const CONTEXT_CHILD_LIMIT: usize = 96;
pub(crate) const DEFAULT_CONTEXT_LIMIT: ContextSnippetLimit = ContextSnippetLimit {
    max_snippets: 1,
    max_chars_per_snippet: 240,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FocusedElementDiagnostic {
    pub permission: &'static str,
    pub focused_app_pid: Option<i32>,
    pub app_bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub system_focused: Option<RawElementDiagnostic>,
    pub app_focused: Option<RawElementDiagnostic>,
    pub chosen_source: Option<&'static str>,
    pub chosen_reason: Option<&'static str>,
    pub resolution_error: Option<&'static str>,
    pub resolver_candidates: Vec<ResolverCandidateDiagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RawElementDiagnostic {
    pub source: &'static str,
    pub element_pid: Option<i32>,
    pub role: RawAttributeDiagnostic,
    pub is_editable: RawAttributeDiagnostic,
    pub value: RawAttributeDiagnostic,
    pub selected_text_range: RawAttributeDiagnostic,
    pub selected_text_range_settable: RawSettableDiagnostic,
    pub selected_text_settable: RawSettableDiagnostic,
    pub subrole: Option<String>,
    pub caret_bounds_computable: bool,
    pub can_restore_and_write: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RawAttributeDiagnostic {
    pub status: &'static str,
    pub value_kind: Option<String>,
    pub value_summary: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RawSettableDiagnostic {
    pub status: &'static str,
    pub settable: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ElementContract {
    role: String,
    subrole: Option<String>,
    is_editable: Option<bool>,
    readable_text: bool,
    selected_range_available: bool,
    caret_bounds_computable: bool,
    can_restore_and_write: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ResolverCandidateDiagnostic {
    pub source: &'static str,
    pub depth: usize,
    pub role: Option<String>,
    pub subrole: RawAttributeDiagnostic,
    pub is_editable: RawAttributeDiagnostic,
    pub readable_text: bool,
    pub selected_range_available: bool,
    pub caret_bounds_computable: bool,
    pub can_restore_and_write: bool,
    pub accepted: bool,
    pub reject_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusSource {
    SystemFocused,
    AppFocused,
}

impl FocusSource {
    fn label(self) -> &'static str {
        match self {
            Self::SystemFocused => "system.focused_ui_element",
            Self::AppFocused => "focused_application.AXFocusedUIElement",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FocusResolution<T> {
    element: T,
    source: FocusSource,
    reason: &'static str,
    diagnostics: Vec<ResolverCandidateDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum ContractError {
    UnsupportedApp,
    SecureField,
    RoleReadError,
    SubroleReadError,
    NotTextRole,
    ExplicitNotEditable,
    TextUnavailable,
    SelectedRangeUnavailable,
    CaretUnavailable,
    CommitUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateEvaluation {
    contract: Result<ElementContract, ContractError>,
    diagnostic: ResolverCandidateDiagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FocusResolutionError {
    reason: ContractError,
    diagnostics: Vec<ResolverCandidateDiagnostic>,
}

impl From<ContractError> for AccessibilityError {
    fn from(error: ContractError) -> Self {
        match error {
            ContractError::UnsupportedApp => Self::UnsupportedApp,
            ContractError::SecureField => Self::SecureField,
            ContractError::CaretUnavailable => Self::CaretUnavailable,
            ContractError::RoleReadError
            | ContractError::SubroleReadError
            | ContractError::NotTextRole
            | ContractError::ExplicitNotEditable
            | ContractError::TextUnavailable
            | ContractError::SelectedRangeUnavailable
            | ContractError::CommitUnavailable => Self::NotEditable,
        }
    }
}

impl ContractError {
    fn unavailable_reason(&self) -> &'static str {
        match self {
            Self::UnsupportedApp => "unsupported_app",
            Self::SecureField => "secure_field",
            Self::RoleReadError => "role_read_error",
            Self::SubroleReadError => "subrole_read_error",
            Self::NotTextRole => "not_text_role",
            Self::ExplicitNotEditable => "explicit_not_editable",
            Self::TextUnavailable => "text_unavailable",
            Self::SelectedRangeUnavailable => "selected_range_unavailable",
            Self::CaretUnavailable => "caret_unavailable",
            Self::CommitUnavailable => "commit_unavailable",
        }
    }
}

impl RawAttributeDiagnostic {
    fn ok(value_kind: impl Into<String>, value_summary: impl Into<String>) -> Self {
        Self {
            status: "ok",
            value_kind: Some(value_kind.into()),
            value_summary: Some(value_summary.into()),
            error: None,
        }
    }

    fn none() -> Self {
        Self {
            status: "none",
            value_kind: None,
            value_summary: None,
            error: None,
        }
    }

    fn error(error: impl Into<String>) -> Self {
        Self {
            status: "error",
            value_kind: None,
            value_summary: None,
            error: Some(error.into()),
        }
    }
}

impl RawSettableDiagnostic {
    fn ok(settable: bool) -> Self {
        Self {
            status: "ok",
            settable: Some(settable),
            error: None,
        }
    }

    fn error(error: impl Into<String>) -> Self {
        Self {
            status: "error",
            settable: None,
            error: Some(error.into()),
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeElement {
    app_bundle_id: String,
    contract: ElementContract,
    children: Vec<ProbeElement>,
}

impl ElementContract {
    #[cfg(test)]
    fn editable_text(role: &str) -> Self {
        Self {
            role: role.to_string(),
            subrole: None,
            is_editable: Some(true),
            readable_text: true,
            selected_range_available: true,
            caret_bounds_computable: true,
            can_restore_and_write: true,
        }
    }

    #[cfg(test)]
    fn container(role: &str) -> Self {
        Self {
            role: role.to_string(),
            subrole: None,
            is_editable: Some(false),
            readable_text: false,
            selected_range_available: false,
            caret_bounds_computable: false,
            can_restore_and_write: false,
        }
    }

    #[cfg(test)]
    fn secure_text() -> Self {
        Self {
            subrole: Some(AX_SECURE_TEXT_FIELD_SUBROLE.to_string()),
            ..Self::editable_text(AX_TEXT_FIELD_ROLE)
        }
    }
}

impl ResolverCandidateDiagnostic {
    fn from_contract(source: FocusSource, depth: usize, contract: &ElementContract) -> Self {
        Self {
            source: source.label(),
            depth,
            role: Some(contract.role.clone()),
            subrole: raw_attribute_from_optional_string(contract.subrole.as_deref()),
            is_editable: raw_attribute_from_optional_bool(contract.is_editable),
            readable_text: contract.readable_text,
            selected_range_available: contract.selected_range_available,
            caret_bounds_computable: contract.caret_bounds_computable,
            can_restore_and_write: contract.can_restore_and_write,
            accepted: false,
            reject_reason: None,
        }
    }

    fn from_error(source: FocusSource, depth: usize, error: ContractError) -> Self {
        Self {
            source: source.label(),
            depth,
            reject_reason: Some(error.unavailable_reason()),
            ..Self::default()
        }
    }
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
const AX_STATIC_TEXT_ROLE: &str = "AXStaticText";
const AX_HEADING_ROLE: &str = "AXHeading";
const AX_LINK_ROLE: &str = "AXLink";
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
            .ok()
            .flatten();
        let is_editable = element
            .bool_attribute(AX_IS_EDITABLE_ATTRIBUTE)
            .ok()
            .flatten()
            .or_else(|| {
                editability_from_fallback(element.is_attribute_settable(AX_VALUE_ATTRIBUTE).ok())
            });
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

fn editability_from_fallback(value_settable: Option<bool>) -> Option<bool> {
    value_settable.filter(|settable| *settable)
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
    // Word positions within the composition session are not tracked by the
    // Accessibility observer yet; the accumulator simply skips such bursts.
    CaptureResult::Ready {
        word_offset: None,
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

fn normalize_context_segment(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_context_line_break(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}')
}

fn byte_index_at_utf16(value: &str, target_utf16: usize) -> usize {
    let mut used_utf16 = 0;
    for (byte_index, ch) in value.char_indices() {
        let next_utf16 = used_utf16 + ch.len_utf16();
        if next_utf16 > target_utf16 {
            return byte_index;
        }
        used_utf16 = next_utf16;
        if used_utf16 == target_utf16 {
            return byte_index + ch.len_utf8();
        }
    }
    value.len()
}

fn normalize_context_lines(value: &str) -> String {
    value
        .split(is_context_line_break)
        .map(normalize_context_segment)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn notes_context_excluding_current_line(value: &str, selected_range_utf16: Range<usize>) -> String {
    let caret_utf16 = selected_range_utf16.end.min(utf16_len(value));
    let mut caret_byte = byte_index_at_utf16(value, caret_utf16);

    // Treat a position between CR and LF as the end of the preceding line.
    if caret_byte > 0
        && caret_byte < value.len()
        && value.as_bytes()[caret_byte - 1] == b'\r'
        && value.as_bytes()[caret_byte] == b'\n'
    {
        caret_byte -= 1;
    }

    let line_start = value[..caret_byte]
        .char_indices()
        .rev()
        .find(|(_, ch)| is_context_line_break(*ch))
        .map(|(index, ch)| index + ch.len_utf8())
        .unwrap_or(0);
    let next_break = value[caret_byte..]
        .char_indices()
        .find(|(_, ch)| is_context_line_break(*ch));
    let remove_end = next_break
        .map(|(relative_index, ch)| {
            let mut end = caret_byte + relative_index + ch.len_utf8();
            if ch == '\r' && value[end..].starts_with('\n') {
                end += 1;
            }
            end
        })
        .unwrap_or(value.len());

    let mut remaining = String::with_capacity(value.len() - (remove_end - line_start));
    remaining.push_str(&value[..line_start]);
    remaining.push_str(&value[remove_end..]);
    normalize_context_lines(&remaining)
}

fn collect_context_text<T: Clone>(
    roots: Vec<T>,
    max_chars: usize,
    role: impl Fn(&T) -> Option<String>,
    editable: impl Fn(&T) -> bool,
    text: impl Fn(&T) -> Option<String>,
    children: impl Fn(&T) -> Vec<T>,
) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut queue = VecDeque::from(roots);
    let mut visited = 0;
    let mut segments = Vec::new();
    let mut chars = 0;

    while let Some(element) = queue.pop_front() {
        visited += 1;
        if visited > CONTEXT_DESCENDANT_SEARCH_LIMIT || chars >= max_chars {
            break;
        }

        let element_role = role(&element).unwrap_or_default();
        let is_text_container = matches!(
            element_role.as_str(),
            AX_TEXT_AREA_ROLE | AX_TEXT_FIELD_ROLE
        );
        let is_editable = editable(&element);
        if !is_editable
            && !is_text_container
            && matches!(
                element_role.as_str(),
                AX_STATIC_TEXT_ROLE | AX_HEADING_ROLE | AX_LINK_ROLE
            )
        {
            if let Some(value) = text(&element) {
                let value = normalize_context_segment(&value);
                if !value.is_empty() && !segments.iter().any(|seen| seen == &value) {
                    let separator_chars = usize::from(!segments.is_empty());
                    let remaining = max_chars.saturating_sub(chars + separator_chars);
                    if remaining == 0 {
                        break;
                    }
                    let bounded = value.chars().take(remaining).collect::<String>();
                    chars += separator_chars + bounded.chars().count();
                    segments.push(bounded);
                }
            }
        }

        if !is_editable && !is_text_container {
            queue.extend(children(&element));
        }
    }

    segments.join("\n")
}

fn ax_context_children(element: &AXUIElement) -> Vec<AXUIElement> {
    let count = element
        .attribute_value_count(AX_CHILDREN_ATTRIBUTE)
        .unwrap_or(0)
        .min(CONTEXT_CHILD_LIMIT);
    if count == 0 {
        return Vec::new();
    }
    element
        .element_array_attribute_range(AX_CHILDREN_ATTRIBUTE, 0, count)
        .unwrap_or_default()
}

fn ax_context_text(element: &AXUIElement) -> Option<String> {
    element
        .string_attribute(AX_VISIBLE_TEXT_ATTRIBUTE)
        .ok()
        .flatten()
        .or_else(|| element.string_attribute(AX_VALUE_ATTRIBUTE).ok().flatten())
        .or_else(|| element.string_attribute(AX_TITLE_ATTRIBUTE).ok().flatten())
}

fn notes_editor_context(element: &AXUIElement) -> Option<String> {
    let value = element
        .string_attribute(AX_VALUE_ATTRIBUTE)
        .ok()
        .flatten()?;
    let selected_range_utf16 = element
        .range_attribute(AX_SELECTED_TEXT_RANGE_ATTRIBUTE)
        .ok()
        .flatten()
        .and_then(ax_range_to_range)?;
    let context = notes_context_excluding_current_line(&value, selected_range_utf16);
    (!context.is_empty()).then_some(context)
}

fn collect_ax_window_text(window: &AXUIElement, max_chars: usize) -> String {
    collect_context_text(
        vec![window.clone()],
        max_chars,
        |element| element.string_attribute(AX_ROLE_ATTRIBUTE).ok().flatten(),
        |element| {
            element
                .bool_attribute(AX_IS_EDITABLE_ATTRIBUTE)
                .ok()
                .flatten()
                .unwrap_or(false)
        },
        ax_context_text,
        ax_context_children,
    )
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
    let app_element = system.focused_application().ok().flatten();
    if let Some(app_element) = &app_element {
        diagnostic.focused_app_pid = app_element.pid().ok();
        if let Some((bundle_id, app_name)) = focused_app_identity(&app_element) {
            diagnostic.app_bundle_id = Some(bundle_id);
            diagnostic.app_name = Some(app_name);
        }
    }
    if permission_status == AccessibilityPermissionStatus::NotTrusted {
        return diagnostic;
    }
    let system_focused = system.focused_ui_element().ok().flatten();
    let app_focused = app_element.as_ref().and_then(|app| {
        app.element_attribute(AX_FOCUSED_UI_ELEMENT_ATTRIBUTE)
            .ok()
            .flatten()
    });
    diagnostic.system_focused = system_focused
        .as_ref()
        .map(|element| raw_element_diagnostic("system.focused_ui_element", element));
    diagnostic.app_focused = app_focused
        .as_ref()
        .map(|element| raw_element_diagnostic("focused_application.AXFocusedUIElement", element));
    if diagnostic.focused_app_pid.is_none() {
        diagnostic.focused_app_pid = system_focused
            .as_ref()
            .or(app_focused.as_ref())
            .and_then(|element| element.pid().ok());
    }
    if diagnostic.app_bundle_id.is_none() {
        let fallback = system_focused
            .as_ref()
            .or(app_focused.as_ref())
            .and_then(app_identity_from_pid);
        if let Some((bundle_id, app_name)) = fallback {
            diagnostic.app_bundle_id = Some(bundle_id);
            diagnostic.app_name = Some(app_name);
        }
    }
    if let Some(app_id) = diagnostic.app_bundle_id.as_deref() {
        match resolve_ax_focus(app_id, system_focused, app_focused) {
            Ok(resolution) => {
                diagnostic.chosen_source = Some(resolution.source.label());
                diagnostic.chosen_reason = Some(resolution.reason);
                diagnostic.resolver_candidates = resolution.diagnostics;
            }
            Err(error) => {
                diagnostic.resolution_error = Some(error.reason.unavailable_reason());
                diagnostic.resolver_candidates = error.diagnostics;
            }
        }
    }

    diagnostic
}

fn raw_element_diagnostic(source: &'static str, element: &AXUIElement) -> RawElementDiagnostic {
    let selected_range = element
        .range_attribute(AX_SELECTED_TEXT_RANGE_ATTRIBUTE)
        .ok()
        .flatten()
        .and_then(ax_range_to_range);
    let selected_text_range_settable =
        raw_settable_diagnostic(element, AX_SELECTED_TEXT_RANGE_ATTRIBUTE);
    let selected_text_settable = raw_settable_diagnostic(element, AX_SELECTED_TEXT_ATTRIBUTE);
    let can_restore_and_write = selected_text_range_settable.settable == Some(true)
        && selected_text_settable.settable == Some(true);
    RawElementDiagnostic {
        source,
        element_pid: element.pid().ok(),
        role: raw_attribute_diagnostic(element, AX_ROLE_ATTRIBUTE),
        is_editable: raw_attribute_diagnostic(element, AX_IS_EDITABLE_ATTRIBUTE),
        value: raw_attribute_diagnostic(element, AX_VALUE_ATTRIBUTE),
        selected_text_range: raw_attribute_diagnostic(element, AX_SELECTED_TEXT_RANGE_ATTRIBUTE),
        selected_text_range_settable,
        selected_text_settable,
        subrole: element
            .string_attribute(AX_SUBROLE_ATTRIBUTE)
            .ok()
            .flatten(),
        caret_bounds_computable: selected_range.as_ref().is_some_and(|range| {
            bounds_for_range(element, range).is_some()
                || bounds_for_range(element, &(range.end..range.end)).is_some()
        }),
        can_restore_and_write,
    }
}

fn raw_attribute_diagnostic(element: &AXUIElement, attribute: &str) -> RawAttributeDiagnostic {
    match element.attribute(attribute) {
        Ok(Some(value)) => {
            let kind = format!("{:?}", value.kind());
            let summary = value_summary(attribute, &value);
            RawAttributeDiagnostic::ok(kind, summary)
        }
        Ok(None) => RawAttributeDiagnostic::none(),
        Err(error) => RawAttributeDiagnostic::error(format!("{error:?}")),
    }
}

fn raw_attribute_from_optional_string(value: Option<&str>) -> RawAttributeDiagnostic {
    match value {
        Some(value) => RawAttributeDiagnostic::ok("String", format!("string:{value}")),
        None => RawAttributeDiagnostic::none(),
    }
}

fn raw_attribute_from_optional_bool(value: Option<bool>) -> RawAttributeDiagnostic {
    match value {
        Some(value) => RawAttributeDiagnostic::ok("Boolean", format!("bool:{value}")),
        None => RawAttributeDiagnostic::none(),
    }
}

fn raw_settable_diagnostic(element: &AXUIElement, attribute: &str) -> RawSettableDiagnostic {
    match element.is_attribute_settable(attribute) {
        Ok(settable) => RawSettableDiagnostic::ok(settable),
        Err(error) => RawSettableDiagnostic::error(format!("{error:?}")),
    }
}

fn value_summary(attribute: &str, value: &AXValue) -> String {
    match attribute {
        AX_ROLE_ATTRIBUTE => value
            .as_string()
            .map(|value| format!("string:{value}"))
            .unwrap_or_else(|| "non_string".to_string()),
        AX_IS_EDITABLE_ATTRIBUTE => value
            .as_bool()
            .map(|value| format!("bool:{value}"))
            .unwrap_or_else(|| "non_bool".to_string()),
        AX_VALUE_ATTRIBUTE => value
            .as_string()
            .map(|value| format!("string_chars:{}", value.chars().count()))
            .unwrap_or_else(|| "non_string".to_string()),
        AX_SELECTED_TEXT_RANGE_ATTRIBUTE => value
            .as_range()
            .map(|range| format!("range:{}:{}", range.location, range.length))
            .unwrap_or_else(|| "non_range".to_string()),
        _ => "present".to_string(),
    }
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
    let contract = ElementContract {
        role: focused.role.clone(),
        subrole: focused.subrole.clone(),
        is_editable: focused.is_editable,
        readable_text: true,
        selected_range_available: focused.has_selected_range,
        caret_bounds_computable: true,
        can_restore_and_write: true,
    };
    validate_element_contract(app_id, &contract).map_err(Into::into)
}

fn validate_element_contract(
    app_id: &str,
    contract: &ElementContract,
) -> Result<(), ContractError> {
    if !is_supported_app(app_id) {
        return Err(ContractError::UnsupportedApp);
    }
    if contract.subrole.as_deref() == Some(AX_SECURE_TEXT_FIELD_SUBROLE) {
        return Err(ContractError::SecureField);
    }
    if !matches!(
        contract.role.as_str(),
        AX_TEXT_AREA_ROLE | AX_TEXT_FIELD_ROLE
    ) {
        return Err(ContractError::NotTextRole);
    }
    if contract.is_editable == Some(false) {
        return Err(ContractError::ExplicitNotEditable);
    }
    if !contract.readable_text {
        return Err(ContractError::TextUnavailable);
    }
    if !contract.selected_range_available {
        return Err(ContractError::SelectedRangeUnavailable);
    }
    if !contract.caret_bounds_computable {
        return Err(ContractError::CaretUnavailable);
    }
    if !contract.can_restore_and_write {
        return Err(ContractError::CommitUnavailable);
    }
    Ok(())
}

fn contract_from_ax_element(element: &AXUIElement) -> Result<ElementContract, ContractError> {
    let snapshot = FocusedElementSnapshot::from_ax_element(element)
        .map_err(|_| ContractError::RoleReadError)?;
    let readable_text = element
        .string_attribute(AX_SELECTED_TEXT_ATTRIBUTE)
        .map(|value| value.is_some())
        .unwrap_or(false)
        || element
            .string_attribute(AX_VALUE_ATTRIBUTE)
            .map(|value| value.is_some())
            .unwrap_or(false);
    let caret_bounds_computable = if snapshot.has_selected_range {
        bounds_for_range(element, &snapshot.selected_range_utf16).is_some()
            || bounds_for_range(
                element,
                &(snapshot.selected_range_utf16.end..snapshot.selected_range_utf16.end),
            )
            .is_some()
    } else {
        false
    };
    let can_restore_and_write = element
        .is_attribute_settable(AX_SELECTED_TEXT_RANGE_ATTRIBUTE)
        .unwrap_or(false)
        && element
            .is_attribute_settable(AX_SELECTED_TEXT_ATTRIBUTE)
            .unwrap_or(false);

    Ok(ElementContract {
        role: snapshot.role,
        subrole: snapshot.subrole,
        is_editable: snapshot.is_editable,
        readable_text,
        selected_range_available: snapshot.has_selected_range,
        caret_bounds_computable,
        can_restore_and_write,
    })
}

fn resolve_contract<T: Clone>(
    app_id: &str,
    system_focused: Option<T>,
    app_focused: Option<T>,
    evaluate: impl Fn(FocusSource, usize, &T) -> CandidateEvaluation,
    children: impl Fn(&T) -> Vec<T>,
) -> Result<FocusResolution<T>, FocusResolutionError> {
    if !is_supported_app(app_id) {
        return Err(FocusResolutionError {
            reason: ContractError::UnsupportedApp,
            diagnostics: Vec::new(),
        });
    }

    let mut diagnostics = Vec::new();
    let mut first_reject_reason: Option<ContractError> = None;

    for (source, element) in [
        (FocusSource::SystemFocused, system_focused),
        (FocusSource::AppFocused, app_focused),
    ]
    .into_iter()
    .filter_map(|(source, element)| element.map(|element| (source, element)))
    {
        let evaluation = evaluate(source, 0, &element);
        match candidate_reject_reason(app_id, &evaluation.contract) {
            None => {
                let mut diagnostic = evaluation.diagnostic;
                diagnostic.accepted = true;
                diagnostics.push(diagnostic);
                return Ok(FocusResolution {
                    element,
                    source,
                    reason: "direct",
                    diagnostics,
                });
            }
            Some(error) => {
                let mut diagnostic = evaluation.diagnostic;
                diagnostic.reject_reason = Some(error.unavailable_reason());
                diagnostics.push(diagnostic);
                if error == ContractError::SecureField {
                    return Err(FocusResolutionError {
                        reason: error,
                        diagnostics,
                    });
                }
                first_reject_reason.get_or_insert(error);
            }
        }

        let mut queue = VecDeque::from(children(&element));
        let mut visited = 0;
        while let Some(child) = queue.pop_front() {
            visited += 1;
            if visited > DESCENDANT_SEARCH_LIMIT {
                break;
            }
            let evaluation = evaluate(source, visited, &child);
            match candidate_reject_reason(app_id, &evaluation.contract) {
                None => {
                    let mut diagnostic = evaluation.diagnostic;
                    diagnostic.accepted = true;
                    diagnostics.push(diagnostic);
                    return Ok(FocusResolution {
                        element: child,
                        source,
                        reason: "resolved_descendant",
                        diagnostics,
                    });
                }
                Some(error) => {
                    let mut diagnostic = evaluation.diagnostic;
                    diagnostic.reject_reason = Some(error.unavailable_reason());
                    diagnostics.push(diagnostic);
                    if error == ContractError::SecureField {
                        return Err(FocusResolutionError {
                            reason: error,
                            diagnostics,
                        });
                    }
                    first_reject_reason.get_or_insert(error);
                    queue.extend(children(&child));
                }
            }
        }
    }

    Err(FocusResolutionError {
        reason: first_reject_reason.unwrap_or(ContractError::NotTextRole),
        diagnostics,
    })
}

fn candidate_reject_reason(
    app_id: &str,
    contract: &Result<ElementContract, ContractError>,
) -> Option<ContractError> {
    match contract {
        Ok(contract) => validate_element_contract(app_id, contract).err(),
        Err(error) => Some(error.clone()),
    }
}

#[cfg(test)]
fn evaluate_contract_candidate<T>(
    source: FocusSource,
    depth: usize,
    element: &T,
    contract: impl Fn(&T) -> Result<ElementContract, ContractError>,
) -> CandidateEvaluation {
    match contract(element) {
        Ok(contract) => CandidateEvaluation {
            diagnostic: ResolverCandidateDiagnostic::from_contract(source, depth, &contract),
            contract: Ok(contract),
        },
        Err(error) => CandidateEvaluation {
            diagnostic: ResolverCandidateDiagnostic::from_error(source, depth, error.clone()),
            contract: Err(error),
        },
    }
}

fn ax_candidate_evaluation(
    source: FocusSource,
    depth: usize,
    element: &AXUIElement,
) -> CandidateEvaluation {
    let contract = contract_from_ax_element(element);
    let mut diagnostic = match contract.as_ref() {
        Ok(contract) => ResolverCandidateDiagnostic::from_contract(source, depth, contract),
        Err(error) => ResolverCandidateDiagnostic::from_error(source, depth, error.clone()),
    };
    diagnostic.role = raw_attribute_diagnostic(element, AX_ROLE_ATTRIBUTE).value_summary;
    diagnostic.subrole = raw_attribute_diagnostic(element, AX_SUBROLE_ATTRIBUTE);
    diagnostic.is_editable = raw_attribute_diagnostic(element, AX_IS_EDITABLE_ATTRIBUTE);

    CandidateEvaluation {
        contract,
        diagnostic,
    }
}

fn resolve_ax_focus(
    app_id: &str,
    system_focused: Option<AXUIElement>,
    app_focused: Option<AXUIElement>,
) -> Result<FocusResolution<AXUIElement>, FocusResolutionError> {
    resolve_contract(
        app_id,
        system_focused,
        app_focused,
        ax_candidate_evaluation,
        |element| {
            element
                .element_array_attribute(AX_CHILDREN_ATTRIBUTE)
                .unwrap_or_default()
        },
    )
}

#[allow(dead_code)]
fn focused_editable_element() -> Result<FocusedEditableElement, AccessibilityError> {
    let system = axuielement::SystemWideElement::new().ok_or(AccessibilityError::NotEditable)?;
    let app_element = system
        .focused_application()
        .map_err(|_| AccessibilityError::NotEditable)?;
    let system_focused = system
        .focused_ui_element()
        .map_err(|_| AccessibilityError::NotEditable)?;
    let app_focused = app_element.as_ref().and_then(|app| {
        app.element_attribute(AX_FOCUSED_UI_ELEMENT_ATTRIBUTE)
            .ok()
            .flatten()
    });
    let (app_bundle_id, app_name) = app_element
        .as_ref()
        .and_then(focused_app_identity)
        .or_else(|| system_focused.as_ref().and_then(app_identity_from_pid))
        .or_else(|| app_focused.as_ref().and_then(app_identity_from_pid))
        .ok_or(AccessibilityError::UnsupportedApp)?;

    capture_preflight(accessibility_permission_status(), &app_bundle_id)?;
    let resolution = resolve_ax_focus(&app_bundle_id, system_focused, app_focused)
        .map_err(|error| AccessibilityError::from(error.reason))?;
    let element = resolution.element;
    let snapshot = FocusedElementSnapshot::from_ax_element(&element)?;

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

pub(crate) fn focused_caret_rect() -> Option<Rect> {
    focused_editable_element().ok().map(|focused| focused.caret)
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
    let app_element = system.focused_application().ok().flatten();
    let system_focused = system.focused_ui_element().ok().flatten();
    let app_focused = app_element.as_ref().and_then(|app| {
        app.element_attribute(AX_FOCUSED_UI_ELEMENT_ATTRIBUTE)
            .ok()
            .flatten()
    });
    let (app_bundle_id, app_name) = app_element
        .as_ref()
        .and_then(focused_app_identity)
        .or_else(|| system_focused.as_ref().and_then(app_identity_from_pid))
        .or_else(|| app_focused.as_ref().and_then(app_identity_from_pid))?;
    capture_preflight(AccessibilityPermissionStatus::Trusted, &app_bundle_id).ok()?;
    let element = resolve_ax_focus(&app_bundle_id, system_focused, app_focused)
        .ok()?
        .element;

    let focused_window = system.focused_window().ok().flatten().or_else(|| {
        element
            .element_attribute(AX_WINDOW_ATTRIBUTE)
            .ok()
            .flatten()
    });
    let window_title = if app_bundle_id == NOTES_BUNDLE_ID {
        // Notes window titles can contain a preview of the active editor line.
        // Keep that line out of every model-facing context field.
        NOTES_APP_NAME.to_string()
    } else {
        focused_window
            .as_ref()
            .and_then(|window| window.string_attribute(AX_TITLE_ATTRIBUTE).ok().flatten())
            .unwrap_or_default()
    };
    let editor_fallback = || ax_context_text(&element).map(|text| normalize_context_segment(&text));
    let visible_text = if app_bundle_id == NOTES_BUNDLE_ID {
        notes_editor_context(&element)
    } else if app_bundle_id == TEXTEDIT_BUNDLE_ID {
        editor_fallback()
    } else {
        focused_window
            .as_ref()
            .map(|window| collect_ax_window_text(window, limit.max_chars_per_snippet))
            .filter(|text| !text.is_empty())
            .or_else(editor_fallback)
    }
    .filter(|text| !text.is_empty())?;
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
        capture_preflight, collect_context_snippets, collect_context_text,
        editability_from_fallback, is_supported_app, notes_context_excluding_current_line,
        resolve_contract, validate_element_contract, validate_focused_editable_element,
        AccessibilityError, AccessibilityPermissionStatus, CaptureResult, ContextSnippet,
        ContextSnippetLimit, ContractError, DestinationRegistry, DestinationSnapshot,
        ElementContract, FocusResolutionError, FocusSource, FocusedElementSnapshot, ProbeElement,
        AX_STATIC_TEXT_ROLE, AX_TEXT_AREA_ROLE, AX_TEXT_FIELD_ROLE,
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

    fn probe(app_bundle_id: &str, contract: ElementContract) -> ProbeElement {
        ProbeElement {
            app_bundle_id: app_bundle_id.to_string(),
            contract,
            children: Vec::new(),
        }
    }

    fn probe_with_children(
        app_bundle_id: &str,
        contract: ElementContract,
        children: Vec<ProbeElement>,
    ) -> ProbeElement {
        ProbeElement {
            app_bundle_id: app_bundle_id.to_string(),
            contract,
            children,
        }
    }

    fn resolve_probe(
        app_id: &str,
        system_focused: Option<ProbeElement>,
        app_focused: Option<ProbeElement>,
    ) -> Result<super::FocusResolution<ProbeElement>, FocusResolutionError> {
        resolve_contract(
            app_id,
            system_focused,
            app_focused,
            |source, depth, element| {
                super::evaluate_contract_candidate(source, depth, element, |element| {
                    Ok(element.contract.clone())
                })
            },
            |element| element.children.clone(),
        )
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
    fn resolver_falls_back_to_app_level_focused_element() {
        let system = probe(
            "com.apple.TextEdit",
            ElementContract::container("AXWebArea"),
        );
        let app = probe(
            "com.apple.TextEdit",
            ElementContract::editable_text(AX_TEXT_AREA_ROLE),
        );

        let resolved = resolve_probe("com.apple.TextEdit", Some(system), Some(app)).unwrap();

        assert_eq!(resolved.source, FocusSource::AppFocused);
        assert_eq!(resolved.reason, "direct");
    }

    #[test]
    fn resolver_rejects_container_only_web_area() {
        let system = probe(
            "com.vivaldi.Vivaldi",
            ElementContract::container("AXWebArea"),
        );

        let error = resolve_probe("com.vivaldi.Vivaldi", Some(system), None).unwrap_err();

        assert_eq!(error.reason, ContractError::NotTextRole);
        assert_eq!(error.diagnostics[0].reject_reason, Some("not_text_role"));
    }

    #[test]
    fn resolver_accepts_editable_descendant() {
        let child = probe(
            "com.vivaldi.Vivaldi",
            ElementContract::editable_text(AX_TEXT_FIELD_ROLE),
        );
        let system = probe_with_children(
            "com.vivaldi.Vivaldi",
            ElementContract::container("AXWebArea"),
            vec![child],
        );

        let resolved = resolve_probe("com.vivaldi.Vivaldi", Some(system), None).unwrap();

        assert_eq!(resolved.source, FocusSource::SystemFocused);
        assert_eq!(resolved.reason, "resolved_descendant");
        assert_eq!(resolved.element.contract.role, AX_TEXT_FIELD_ROLE);
    }

    #[test]
    fn resolver_preserves_secure_field_rejection() {
        let system = probe("com.apple.TextEdit", ElementContract::secure_text());

        let error = resolve_probe("com.apple.TextEdit", Some(system), None).unwrap_err();

        assert_eq!(error.reason, ContractError::SecureField);
    }

    #[test]
    fn resolver_keeps_unsupported_app_rejected_after_fallback() {
        let system = probe(
            "org.mozilla.firefox",
            ElementContract::container("AXWebArea"),
        );
        let app = probe(
            "org.mozilla.firefox",
            ElementContract::editable_text(AX_TEXT_FIELD_ROLE),
        );

        let error = resolve_probe("org.mozilla.firefox", Some(system), Some(app)).unwrap_err();

        assert_eq!(error.reason, ContractError::UnsupportedApp);
    }

    #[test]
    fn textedit_unknown_editable_text_area_passes_with_full_contract() {
        let contract = ElementContract {
            is_editable: None,
            ..ElementContract::editable_text(AX_TEXT_AREA_ROLE)
        };

        assert_eq!(
            validate_element_contract("com.apple.TextEdit", &contract),
            Ok(())
        );
    }

    #[test]
    fn text_area_with_missing_subrole_passes_with_full_contract() {
        let contract = ElementContract {
            subrole: None,
            ..ElementContract::editable_text(AX_TEXT_AREA_ROLE)
        };

        assert_eq!(
            validate_element_contract("com.apple.TextEdit", &contract),
            Ok(())
        );
    }

    #[test]
    fn unsupported_editable_and_unsettable_value_keeps_unknown_and_full_contract_passes() {
        let contract = ElementContract {
            is_editable: editability_from_fallback(Some(false)),
            ..ElementContract::editable_text(AX_TEXT_AREA_ROLE)
        };

        assert_eq!(contract.is_editable, None);
        assert_eq!(
            validate_element_contract("com.apple.TextEdit", &contract),
            Ok(())
        );
    }

    #[test]
    fn settable_value_promotes_unknown_editability_to_true() {
        assert_eq!(editability_from_fallback(Some(true)), Some(true));
        assert_eq!(editability_from_fallback(None), None);
    }

    #[test]
    fn explicit_not_editable_rejects_even_with_full_contract() {
        let contract = ElementContract {
            is_editable: Some(false),
            ..ElementContract::editable_text(AX_TEXT_AREA_ROLE)
        };

        assert_eq!(
            validate_element_contract("com.apple.TextEdit", &contract),
            Err(ContractError::ExplicitNotEditable)
        );
    }

    #[test]
    fn resolver_preserves_exact_reject_reason_when_contract_construction_fails() {
        let system = probe(
            "com.apple.TextEdit",
            ElementContract::editable_text(AX_TEXT_AREA_ROLE),
        );

        let error = resolve_contract(
            "com.apple.TextEdit",
            Some(system),
            None,
            |source, depth, element| {
                super::evaluate_contract_candidate(source, depth, element, |_| {
                    Err(ContractError::SubroleReadError)
                })
            },
            |element| element.children.clone(),
        )
        .unwrap_err();

        assert_eq!(error.reason, ContractError::SubroleReadError);
        assert_eq!(
            error.diagnostics[0].reject_reason,
            Some("subrole_read_error")
        );
    }

    #[test]
    fn text_area_without_selected_range_fails() {
        let contract = ElementContract {
            selected_range_available: false,
            ..ElementContract::editable_text(AX_TEXT_AREA_ROLE)
        };

        assert_eq!(
            validate_element_contract("com.apple.TextEdit", &contract),
            Err(ContractError::SelectedRangeUnavailable)
        );
    }

    #[test]
    fn secure_text_field_with_unknown_editable_still_fails_secure() {
        let contract = ElementContract {
            is_editable: None,
            ..ElementContract::secure_text()
        };

        assert_eq!(
            validate_element_contract("com.apple.TextEdit", &contract),
            Err(ContractError::SecureField)
        );
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

    #[derive(Clone)]
    struct ContextProbe {
        role: &'static str,
        editable: bool,
        text: Option<&'static str>,
        children: Vec<ContextProbe>,
    }

    #[test]
    fn window_context_collects_visible_messages_and_excludes_the_editor() {
        let tree = ContextProbe {
            role: "AXWindow",
            editable: false,
            text: None,
            children: vec![
                ContextProbe {
                    role: AX_STATIC_TEXT_ROLE,
                    editable: false,
                    text: Some("Mira: The launch is tomorrow."),
                    children: Vec::new(),
                },
                ContextProbe {
                    role: AX_STATIC_TEXT_ROLE,
                    editable: false,
                    text: Some("Dev: Meet at Union Station."),
                    children: Vec::new(),
                },
                ContextProbe {
                    role: AX_TEXT_AREA_ROLE,
                    editable: true,
                    text: Some("cnt cm tmrw"),
                    children: vec![ContextProbe {
                        role: AX_STATIC_TEXT_ROLE,
                        editable: false,
                        text: Some("cnt cm tmrw"),
                        children: Vec::new(),
                    }],
                },
            ],
        };

        let text = collect_context_text(
            vec![tree],
            240,
            |element| Some(element.role.to_string()),
            |element| element.editable,
            |element| element.text.map(str::to_string),
            |element| element.children.clone(),
        );

        assert_eq!(
            text,
            "Mira: The launch is tomorrow.\nDev: Meet at Union Station."
        );
        assert!(!text.contains("cnt cm tmrw"));
    }

    #[test]
    fn window_context_deduplicates_and_bounds_accessible_text() {
        let repeated = ContextProbe {
            role: AX_STATIC_TEXT_ROLE,
            editable: false,
            text: Some("same message"),
            children: Vec::new(),
        };
        let text = collect_context_text(
            vec![repeated.clone(), repeated],
            8,
            |element| Some(element.role.to_string()),
            |element| element.editable,
            |element| element.text.map(str::to_string),
            |element| element.children.clone(),
        );

        assert_eq!(text, "same mes");
    }

    #[test]
    fn notes_context_excludes_only_the_line_containing_the_caret() {
        let note = "Project kickoff is Monday.\nCnt cm tmrw\nMeet at Union Station.";
        let caret = "Project kickoff is Monday.\nCnt".encode_utf16().count();

        assert_eq!(
            notes_context_excluding_current_line(note, caret..caret),
            "Project kickoff is Monday.\nMeet at Union Station."
        );
    }

    #[test]
    fn notes_context_handles_utf16_and_notes_line_separators() {
        let note = "Above\u{2028}𝄞 current\u{2028}Below";
        let caret = "Above\u{2028}𝄞".encode_utf16().count();

        assert_eq!(
            notes_context_excluding_current_line(note, caret..caret),
            "Above\nBelow"
        );
    }

    #[test]
    fn notes_context_keeps_previous_lines_when_caret_is_on_empty_trailing_line() {
        let note = "Above\nCurrent line\n";
        let caret = note.encode_utf16().count();

        assert_eq!(
            notes_context_excluding_current_line(note, caret..caret),
            "Above\nCurrent line"
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
