//! Workstream 3: Accessibility permission detection, focused editable element
//! recognition, secure-field exclusion, burst capture, and bounded window text.
//!
//! Produces `quip_contracts::CaptureResult` for the composition layer. Element
//! handles, insertion markers, and restoration state stay internal here; only
//! the opaque `destination_id` crosses the boundary.

use std::collections::HashMap;
use std::ops::Range;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct ContextSnippetLimit {
    pub max_snippets: usize,
    pub max_chars_per_snippet: usize,
}

#[cfg(test)]
mod tests {
    use super::{
        AccessibilityError, AccessibilityPermissionStatus, ContextSnippetLimit,
        DestinationRegistry, DestinationSnapshot,
    };

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
}
