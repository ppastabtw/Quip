//! Workstream 3: Accessibility permission detection, focused editable element
//! recognition, secure-field exclusion, burst capture, and bounded window text.
//!
//! Produces `quip_contracts::CaptureResult` for the composition layer. Element
//! handles, insertion markers, and restoration state stay internal here; only
//! the opaque `destination_id` crosses the boundary.
