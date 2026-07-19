//! macOS InputMethodKit bridge for ordinary destination text fields.
//!
//! Printable events pass through, so the destination inserts literal text as
//! usual. Quip tracks the resulting UTF-16 range and, after explicit
//! confirmation, replaces it through the destination's IMK text client.

use crate::{run_capture_result, EngineState};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{msg_send, sel, AnyThread, MainThreadMarker};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSScreen};
use objc2_foundation::{NSDictionary, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_input_method_kit::IMKServer;
use quip_contracts::{CaptureResult, Rect, Trigger};
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const IDLE_DEBOUNCE_MS: u64 = 150;
const MAX_DRAFT_UTF16: usize = 80;

static APP: OnceLock<AppHandle> = OnceLock::new();

thread_local! {
    static SERVER: OnceCell<Retained<IMKServer>> = const { OnceCell::new() };
    static SESSIONS: RefCell<HashMap<usize, Session>> = RefCell::new(HashMap::new());
}

#[derive(Debug)]
struct Session {
    client: Retained<AnyObject>,
    draft: String,
    start_utf16: usize,
    end_utf16: usize,
    generation: u64,
    destination_id: Option<String>,
    burst_id: Option<String>,
    offered: bool,
    last_event: Option<EventFingerprint>,
    last_event_handled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EventFingerprint {
    event_number: isize,
    timestamp_bits: u64,
    key_code: u16,
}

impl Session {
    fn new(client: Retained<AnyObject>, start_utf16: usize) -> Self {
        Self {
            client,
            draft: String::new(),
            start_utf16,
            end_utf16: start_utf16,
            generation: 0,
            destination_id: None,
            burst_id: None,
            offered: false,
            last_event: None,
            last_event_handled: false,
        }
    }

    fn reset_at(&mut self, location: usize) {
        self.draft.clear();
        self.start_utf16 = location;
        self.end_utf16 = location;
        self.generation = self.generation.wrapping_add(1);
        self.destination_id = None;
        self.burst_id = None;
        self.offered = false;
    }
}

unsafe extern "C" {
    fn quip_imk_shim_force_link();
}

#[unsafe(no_mangle)]
extern "C" fn quip_imk_handle_event(
    controller_id: usize,
    event: *mut NSEvent,
    client: *mut AnyObject,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: InputMethodKit supplies live Objective-C objects for the
        // entire duration of this synchronous callback.
        let (Some(event), Some(client)) = (unsafe { event.as_ref() }, unsafe { client.as_ref() })
        else {
            return false;
        };
        handle_key_event(controller_id, event, client)
    }))
    .unwrap_or_else(|_| {
        // A Rust panic must never cross this Objective-C callback boundary;
        // doing so aborts the entire input-method process and strands every
        // connected text client. Passing the event through is the safe
        // failure mode.
        tracing::error!(controller_id, "panic contained in IMK key callback");
        false
    })
}

#[unsafe(no_mangle)]
extern "C" fn quip_imk_activate(controller_id: usize, client: *mut AnyObject) {
    // SAFETY: InputMethodKit supplies a live client object. Retaining it keeps
    // the text client alive for Quip's active input session.
    let Some(client) = (unsafe { client.as_ref() }) else {
        return;
    };
    let location = selected_range(client).map_or(0, |range| range.location);
    let Some(client) = (unsafe { Retained::retain(client as *const AnyObject as *mut AnyObject) })
    else {
        return;
    };
    SESSIONS.with(|sessions| {
        sessions
            .borrow_mut()
            .insert(controller_id, Session::new(client, location));
    });
    tracing::info!(controller_id, location, "InputMethodKit session activated");
}

#[unsafe(no_mangle)]
extern "C" fn quip_imk_deactivate(controller_id: usize) {
    SESSIONS.with(|sessions| {
        sessions.borrow_mut().remove(&controller_id);
    });
    if let Some(app) = APP.get() {
        crate::end_composition_session(app.clone());
    }
}

#[unsafe(no_mangle)]
extern "C" fn quip_imk_close(controller_id: usize) {
    SESSIONS.with(|sessions| {
        sessions.borrow_mut().remove(&controller_id);
    });
}

/// Starts one process-wide IMK server. Tauri already owns the NSApplication
/// event loop, so Quip can keep its existing webview popup in this process.
pub(crate) fn start(app: &AppHandle) -> Result<(), String> {
    APP.set(app.clone())
        .map_err(|_| "InputMethodKit app handle was already initialized".to_string())?;
    MainThreadMarker::new()
        .ok_or_else(|| "InputMethodKit must start on the main thread".to_string())?;
    let name = NSString::from_str("com.hackthe6ix.quip.inputmethod.v3_Connection");
    let bundle_identifier = NSString::from_str("com.hackthe6ix.quip.inputmethod.v3");
    // SAFETY: The native shim contains only Objective-C class metadata and
    // forwarding methods. Calling this no-op retains it in the final binary.
    unsafe { quip_imk_shim_force_link() };
    // SAFETY: Both strings are valid for this installed input-method bundle;
    // IMK resolves QuipInputController from InputMethodServerControllerClass.
    let server = unsafe {
        IMKServer::initWithName_bundleIdentifier(
            IMKServer::alloc(),
            Some(&name),
            Some(&bundle_identifier),
        )
    }
    .ok_or_else(|| "IMKServer initialization failed".to_string())?;
    SERVER.with(|slot| {
        slot.set(server)
            .map_err(|_| "IMKServer was already initialized".to_string())
    })?;
    tracing::info!("InputMethodKit server started");
    Ok(())
}

fn handle_key_event(controller_id: usize, event: &NSEvent, client: &AnyObject) -> bool {
    let fingerprint = EventFingerprint {
        event_number: event.eventNumber(),
        timestamp_bits: event.timestamp().to_bits(),
        key_code: event.keyCode(),
    };
    if let Some(handled) = SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        let session = sessions.get_mut(&controller_id)?;
        (session.last_event == Some(fingerprint)).then_some(session.last_event_handled)
    }) {
        return handled;
    }

    tracing::info!(
        controller_id,
        event_number = fingerprint.event_number,
        key_code = fingerprint.key_code,
        "InputMethodKit unique key event"
    );
    let handled = handle_key_event_once(controller_id, event, client);
    SESSIONS.with(|sessions| {
        if let Some(session) = sessions.borrow_mut().get_mut(&controller_id) {
            session.last_event = Some(fingerprint);
            session.last_event_handled = handled;
        }
    });
    handled
}

fn handle_key_event_once(controller_id: usize, event: &NSEvent, client: &AnyObject) -> bool {
    let modifiers = event.modifierFlags();
    if modifiers.intersects(
        NSEventModifierFlags::Command
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Function,
    ) {
        reset_session(controller_id, client);
        return false;
    }

    let key_code = event.keyCode();
    let characters = event
        .characters()
        .map(|value| value.to_string())
        .unwrap_or_default();

    if handle_candidate_key(controller_id, key_code, &characters) {
        return true;
    }

    // Navigation, deletion, and Return make a previously tracked range
    // ambiguous. Let the client handle them and begin fresh afterward.
    if matches!(key_code, 36 | 48 | 51 | 53 | 71 | 76 | 117 | 123..=126) {
        reset_session(controller_id, client);
        return false;
    }

    if characters.is_empty() || characters.chars().any(char::is_control) {
        return false;
    }

    let selected = selected_range(client).unwrap_or(NSRange::new(0, 0));
    let start = selected.location;
    // SAFETY: The client is live for the duration of this IMK callback.
    let Some(retained_client) =
        (unsafe { Retained::retain(client as *const AnyObject as *mut AnyObject) })
    else {
        return false;
    };

    let (generation, should_dismiss) = SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        let session = sessions
            .entry(controller_id)
            .or_insert_with(|| Session::new(retained_client.clone(), start));
        session.client = retained_client;
        if session.draft.is_empty() || selected.location != session.end_utf16 {
            session.reset_at(start);
        }
        let should_dismiss = session.offered;
        session.draft.push_str(&characters);
        session.end_utf16 = selected.location + characters.encode_utf16().count();
        session.generation = session.generation.wrapping_add(1);
        session.destination_id = None;
        session.burst_id = None;
        session.offered = false;
        (session.generation, should_dismiss)
    });

    // Dismissing calls back into `dismiss_active`, which borrows SESSIONS.
    // Keep it outside the RefCell borrow above to avoid a re-entrant panic.
    if should_dismiss {
        if let Some(app) = APP.get() {
            crate::dismiss_suggestions(app.clone());
        }
    }

    schedule_idle_capture(controller_id, generation);
    // Pass through so the destination performs its normal literal insertion.
    false
}

fn handle_candidate_key(controller_id: usize, key_code: u16, characters: &str) -> bool {
    let offered = SESSIONS.with(|sessions| {
        sessions
            .borrow()
            .get(&controller_id)
            .is_some_and(|session| session.offered)
    });
    if !offered {
        return false;
    }
    let Some(app) = APP.get().cloned() else {
        return false;
    };

    if key_code == 53 {
        crate::dismiss_suggestions(app);
        return true;
    }
    if matches!(key_code, 123 | 125) {
        crate::move_selection(app, -1);
        return true;
    }
    if matches!(key_code, 124 | 126) {
        crate::move_selection(app, 1);
        return true;
    }

    let index = if key_code == 48 {
        let engine = app.state::<EngineState>();
        let snapshot = engine.0.lock().unwrap().current_snapshot();
        match snapshot {
            crate::composition::Snapshot::Suggesting { selected, .. } => Some(selected),
            _ => None,
        }
    } else {
        characters
            .chars()
            .next()
            .and_then(|character| character.to_digit(10))
            .filter(|digit| (1..=5).contains(digit))
            .map(|digit| digit as usize - 1)
    };

    if let Some(index) = index {
        let _ = crate::select_candidate(app, index);
        return true;
    }
    false
}

fn reset_session(controller_id: usize, client: &AnyObject) {
    let location = selected_range(client).map_or(0, |range| range.location);
    SESSIONS.with(|sessions| {
        if let Some(session) = sessions.borrow_mut().get_mut(&controller_id) {
            session.reset_at(location);
        }
    });
}

fn schedule_idle_capture(controller_id: usize, generation: u64) {
    let Some(app) = APP.get().cloned() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(IDLE_DEBOUNCE_MS)).await;
        let app_for_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(capture) = build_capture(controller_id, generation, &app_for_main) else {
                return;
            };
            let handle = app_for_main.clone();
            tauri::async_runtime::spawn(async move {
                run_capture_result(handle, capture, false, "input_method", None).await;
            });
        });
    });
}

fn build_capture(controller_id: usize, generation: u64, app: &AppHandle) -> Option<CaptureResult> {
    let profile_id = {
        let engine = app.state::<EngineState>();
        let engine = engine.0.lock().ok()?;
        if !engine.settings.enabled {
            return None;
        }
        engine.settings.active_profile.clone()
    };

    SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        let Some(session) = sessions.get_mut(&controller_id) else {
            tracing::debug!(
                controller_id,
                generation,
                "IMK capture skipped: session missing"
            );
            return None;
        };
        if session.generation != generation {
            tracing::debug!(
                controller_id,
                generation,
                current_generation = session.generation,
                "IMK capture skipped: stale debounce"
            );
            return None;
        }
        if session.draft.trim().is_empty() {
            tracing::debug!(
                controller_id,
                generation,
                "IMK capture skipped: empty draft"
            );
            return None;
        }
        let Some(selected) = selected_range(&session.client) else {
            tracing::debug!(
                controller_id,
                generation,
                "IMK capture skipped: selection unavailable"
            );
            return None;
        };
        if selected.length != 0 || selected.location != session.end_utf16 {
            tracing::debug!(
                controller_id,
                generation,
                selected_location = selected.location,
                selected_length = selected.length,
                expected_location = session.end_utf16,
                "IMK capture skipped: selection moved"
            );
            session.reset_at(selected.location);
            return None;
        }

        trim_draft_to_limit(session);
        let Some(caret) = caret_rect(&session.client, selected.location) else {
            tracing::debug!(
                controller_id,
                generation,
                character_index = selected.location,
                "IMK capture skipped: caret rectangle unavailable"
            );
            return None;
        };
        let destination_id = format!("imk_destination_{controller_id:x}_{generation}");
        let burst_id = format!("burst_{destination_id}");
        session.destination_id = Some(destination_id.clone());
        session.burst_id = Some(burst_id.clone());
        Some(CaptureResult::Ready {
            burst_id,
            destination_id,
            profile_id,
            draft: session.draft.clone(),
            trigger: Trigger::Idle,
            caret,
            word_offset: None,
        })
    })
}

fn trim_draft_to_limit(session: &mut Session) {
    let units = session.draft.encode_utf16().count();
    if units <= MAX_DRAFT_UTF16 {
        return;
    }
    let mut kept_units = 0;
    let mut byte_start = session.draft.len();
    for (index, character) in session.draft.char_indices().rev() {
        let width = character.len_utf16();
        if kept_units + width > MAX_DRAFT_UTF16 {
            break;
        }
        kept_units += width;
        byte_start = index;
    }
    session.draft = session.draft[byte_start..].to_string();
    session.start_utf16 = session.end_utf16.saturating_sub(kept_units);
}

fn selected_range(client: &AnyObject) -> Option<NSRange> {
    // SAFETY: respondsToSelector: is part of NSObjectProtocol.
    let responds: Bool = unsafe { msg_send![client, respondsToSelector: sel!(selectedRange)] };
    if !responds.as_bool() {
        return None;
    }
    // SAFETY: IMK guarantees its client conforms to IMKTextInput.
    let range: NSRange = unsafe { msg_send![client, selectedRange] };
    // NSNotFound is NSIntegerMax on macOS, not NSUIntegerMax. Some web views
    // return it while unfocused; treating it as a real caret poisons the next
    // session with a huge UTF-16 offset.
    (range.location != isize::MAX as usize && range.location != usize::MAX).then_some(range)
}

fn caret_rect(client: &AnyObject, character_index: usize) -> Option<Rect> {
    let native = first_rect_for_range(client, character_index)
        .or_else(|| line_height_rect(client, character_index, false))
        .or_else(|| {
            character_index
                .checked_sub(1)
                .and_then(|index| line_height_rect(client, index, true))
        })?;

    let mtm = MainThreadMarker::new()?;
    let primary = NSScreen::screens(mtm).firstObject()?;
    let frame = primary.frame();
    let primary_top = frame.origin.y + frame.size.height;
    Some(appkit_to_tauri_rect(native, primary_top))
}

fn first_rect_for_range(client: &AnyObject, character_index: usize) -> Option<NSRect> {
    // NSTextInputClient's caret query accepts an empty range at the insertion
    // point and returns screen coordinates. TextEdit supports this path even
    // when IMK's attributesForCharacterIndex rejects the end-of-document
    // index.
    let responds: Bool = unsafe {
        msg_send![client, respondsToSelector: sel!(firstRectForCharacterRange:actualRange:)]
    };
    if !responds.as_bool() {
        return None;
    }
    let range = NSRange::new(character_index, 0);
    let mut actual = NSRange::new(0, 0);
    // SAFETY: The selector is part of NSTextInputClient and `actual` remains
    // valid for the duration of the synchronous Objective-C call.
    let native: NSRect =
        unsafe { msg_send![client, firstRectForCharacterRange: range, actualRange: &mut actual] };
    usable_native_rect(native).then_some(native)
}

fn line_height_rect(
    client: &AnyObject,
    character_index: usize,
    place_after_character: bool,
) -> Option<NSRect> {
    // SAFETY: respondsToSelector: is part of NSObjectProtocol.
    let responds: Bool = unsafe {
        msg_send![client, respondsToSelector: sel!(attributesForCharacterIndex:lineHeightRectangle:)]
    };
    if !responds.as_bool() {
        return None;
    }
    let mut native = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
    // SAFETY: IMKTextInput defines this selector with an output NSRect pointer.
    let _: Option<Retained<NSDictionary>> = unsafe {
        msg_send![client, attributesForCharacterIndex: character_index, lineHeightRectangle: &mut native]
    };
    if !usable_native_rect(native) {
        return None;
    }
    if place_after_character {
        native.origin.x += native.size.width;
        native.size.width = 0.0;
    }
    Some(native)
}

fn usable_native_rect(native: NSRect) -> bool {
    native.origin.x.is_finite()
        && native.origin.y.is_finite()
        && native.size.width.is_finite()
        && native.size.height.is_finite()
        && native.size.height > 0.0
}

fn appkit_to_tauri_rect(native: NSRect, primary_top: f64) -> Rect {
    Rect {
        x: native.origin.x,
        y: primary_top - (native.origin.y + native.size.height),
        width: native.size.width.max(1.0),
        height: native.size.height,
    }
}

pub(crate) fn prediction_settled(app: &AppHandle, burst_id: &str, offered: bool) {
    let burst_id = burst_id.to_string();
    let _ = app.run_on_main_thread(move || {
        SESSIONS.with(|sessions| {
            for session in sessions.borrow_mut().values_mut() {
                if session.burst_id.as_deref() == Some(&burst_id) {
                    session.offered = offered;
                    if !offered {
                        let end = session.end_utf16;
                        session.reset_at(end);
                    }
                    break;
                }
            }
        });
    });
}

pub(crate) fn commit_candidate(
    app: &AppHandle,
    destination_id: &str,
    text: &str,
) -> Result<(), String> {
    if crate::ime_bridge::is_native_destination(destination_id) {
        return crate::ime_bridge::commit_candidate(destination_id, text);
    }
    if !destination_id.starts_with("imk_destination_") {
        return Ok(());
    }
    let destination_id = destination_id.to_string();
    let text = text.to_string();
    if MainThreadMarker::new().is_some() {
        return commit_candidate_on_main(&destination_id, &text);
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let result = commit_candidate_on_main(&destination_id, &text);
        let _ = sender.send(result);
    })
    .map_err(|error| format!("could not schedule IMK commit: {error}"))?;
    receiver
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| "timed out waiting for IMK commit".to_string())?
}

fn commit_candidate_on_main(destination_id: &str, text: &str) -> Result<(), String> {
    SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        let Some(session) = sessions
            .values_mut()
            .find(|session| session.destination_id.as_deref() == Some(destination_id))
        else {
            return Err("the IMK destination is no longer active".to_string());
        };
        let replacement = NSRange::new(
            session.start_utf16,
            session.end_utf16.saturating_sub(session.start_utf16),
        );
        let value = NSString::from_str(text);
        // SAFETY: IMKTextInput defines insertText:replacementRange:.
        let _: () = unsafe {
            msg_send![&*session.client, insertText: &*value, replacementRange: replacement]
        };
        let new_location = session.start_utf16 + text.encode_utf16().count();
        session.reset_at(new_location);
        Ok(())
    })
}

pub(crate) fn dismiss_active(app: &AppHandle) {
    let _ = app.run_on_main_thread(|| {
        SESSIONS.with(|sessions| {
            for session in sessions.borrow_mut().values_mut() {
                if session.offered {
                    let end = session.end_utf16;
                    session.reset_at(end);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_at_a_utf16_boundary() {
        let client: Retained<AnyObject> = objc2_foundation::NSObject::new().into_super();
        let mut session = Session::new(client, 20);
        session.draft = format!("{}{}", "a".repeat(79), "🙂🙂");
        session.end_utf16 = 103;
        trim_draft_to_limit(&mut session);
        assert_eq!(session.draft.encode_utf16().count(), 80);
        assert!(session.draft.ends_with("🙂🙂"));
        assert_eq!(session.start_utf16, 23);
    }

    #[test]
    fn converts_appkit_bottom_left_to_tauri_top_left_coordinates() {
        let native = NSRect::new(NSPoint::new(320.0, 700.0), NSSize::new(0.0, 18.0));
        let converted = appkit_to_tauri_rect(native, 982.0);
        assert_eq!(converted.x, 320.0);
        assert_eq!(converted.y, 264.0);
        assert_eq!(converted.width, 1.0);
        assert_eq!(converted.height, 18.0);
    }
}
