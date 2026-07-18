//! Quip: a local-first macOS composition layer. Tray-only Tauri shell.
//!
//! IME model: the user types in their own textbox; the `suggestions` window
//! is a small non-focusable candidate bar anchored above the caret. The
//! webviews are pure renderers: every mutation goes through a command into
//! the [`composition::Engine`], and every state change is broadcast as an
//! event. Dev/validation hooks: `QUIP_DATA_DIR` overrides the data dir,
//! `QUIP_SHOW=demo,settings` shows windows at startup, `QUIP_SELFTEST=1`
//! drives the full fixture flow headlessly and exits.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accessibility;
mod commit;
mod composition;
mod debug_events;
mod inference;
mod learning;
mod settings;

use commit::CommitOutcome;
use composition::{BurstInput, Engine, Snapshot};
use debug_events::{DebugEventView, DebugSink};
use inference::{DemoCase, FixtureLookupDebug, Metrics, SideSpec};
use quip_contracts::{
    CaptureResult, PredictionRequest, PredictionResult, Rect, SidecarHealth, Trigger,
};
use serde::Serialize;
use serde_json::{json, Value};
use settings::{AppSettings, BackendMode};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WindowEvent, Wry,
};

const BAR_HEIGHT: f64 = 44.0;
const BAR_GAP: f64 = 10.0;

struct EngineState(Mutex<Engine>);

struct DebugState(Mutex<DebugSink>);

struct TrayHandles {
    enabled: CheckMenuItem<Wry>,
    window_context: CheckMenuItem<Wry>,
    pause_learning: CheckMenuItem<Wry>,
    profiles: Vec<(String, CheckMenuItem<Wry>)>,
}

impl TrayHandles {
    fn sync(&self, settings: &AppSettings) {
        let _ = self.enabled.set_checked(settings.enabled);
        let _ = self.window_context.set_checked(settings.window_context);
        let _ = self.pause_learning.set_checked(settings.learning_paused);
        for (profile_id, item) in &self.profiles {
            let _ = item.set_checked(*profile_id == settings.active_profile);
        }
    }
}

struct TrayState(Mutex<Option<TrayHandles>>);

/// Estimates the bar's logical width from its chip contents.
fn bar_width(candidates: &[String], has_error: bool) -> f64 {
    if has_error {
        return 300.0;
    }
    let chips: f64 = candidates
        .iter()
        .map(|c| c.chars().count() as f64 * 7.2 + 38.0)
        .sum();
    (chips + 20.0).clamp(180.0, 780.0)
}

/// Positions, sizes, and shows/hides the candidate bar to match the
/// snapshot. The bar is `focusable: false`, so showing it never steals key
/// focus from the textbox the user is typing in.
fn sync_bar(app: &AppHandle, snapshot: &Snapshot) {
    let Some(bar) = app.get_webview_window("suggestions") else {
        record_debug(
            app,
            "bar_hidden",
            "suggestion window unavailable",
            json!({ "phase": "missing_window" }),
        );
        return;
    };
    match snapshot {
        Snapshot::Suggesting {
            caret,
            candidates,
            error,
            ..
        } => {
            let width = bar_width(candidates, error.is_some());
            let _ = bar.set_size(LogicalSize::new(width, BAR_HEIGHT));
            let _ = bar.set_position(LogicalPosition::new(
                (caret.x - 8.0).max(0.0),
                (caret.y - BAR_HEIGHT - BAR_GAP).max(0.0),
            ));
            let _ = bar.show();
            record_debug(
                app,
                "bar_shown",
                format!("shown with {} candidates", candidates.len()),
                json!({
                    "phase": "suggesting",
                    "candidate_count": candidates.len(),
                    "x": (caret.x - 8.0).max(0.0),
                    "y": (caret.y - BAR_HEIGHT - BAR_GAP).max(0.0),
                    "width": width,
                    "height": BAR_HEIGHT,
                }),
            );
        }
        Snapshot::Predicting { .. } => {} // nothing shown until there is something to say
        _ => {
            let _ = bar.hide();
            record_debug(
                app,
                "bar_hidden",
                "bar hidden",
                json!({ "phase": snapshot_phase(snapshot) }),
            );
        }
    }
}

fn emit_snapshot(app: &AppHandle, snapshot: &Snapshot) {
    sync_bar(app, snapshot);
    let _ = app.emit("composition://state", snapshot);
}

fn emit_settings(app: &AppHandle) {
    let engine = app.state::<EngineState>();
    let engine = engine.0.lock().unwrap();
    let settings = engine.settings.clone();
    drop(engine);
    if let Some(handles) = app.state::<TrayState>().0.lock().unwrap().as_ref() {
        handles.sync(&settings);
    }
    let _ = app.emit("settings://changed", &settings);
}

fn emit_metrics(app: &AppHandle) {
    let engine = app.state::<EngineState>();
    let metrics = engine.0.lock().unwrap().metrics.clone();
    let _ = app.emit("metrics://changed", &metrics);
}

fn show_window(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn record_debug(app: &AppHandle, event: &str, summary: impl Into<String>, payload: Value) {
    let debug = app.state::<DebugState>();
    if let Ok(mut sink) = debug.0.lock() {
        sink.record(event, summary, payload);
    };
}

fn record_resolver_candidates(
    app: &AppHandle,
    parent_event: &'static str,
    focused: &accessibility::FocusedElementDiagnostic,
) {
    for candidate in &focused.resolver_candidates {
        record_debug(
            app,
            "capture_resolver_candidate",
            format!(
                "{} {} depth {} -> {}",
                parent_event,
                candidate.source,
                candidate.depth,
                candidate.reject_reason.unwrap_or("accepted")
            ),
            json!({
                "parent_event": parent_event,
                "candidate": candidate,
            }),
        );
    }
}

fn snapshot_phase(snapshot: &Snapshot) -> &'static str {
    match snapshot {
        Snapshot::Idle => "idle",
        Snapshot::Predicting { .. } => "predicting",
        Snapshot::Suggesting { .. } => "suggesting",
        Snapshot::Applied { .. } => "applied",
        Snapshot::Unavailable { .. } => "unavailable",
    }
}

fn record_prediction_result(
    app: &AppHandle,
    burst_id: &str,
    request: &PredictionRequest,
    lookup_debug: Option<&FixtureLookupDebug>,
    result: &PredictionResult,
) {
    let (has_context, context_count, fallback_used, lookup_variant) = lookup_debug
        .map(|debug| {
            (
                debug.has_context,
                debug.context_count,
                debug.fallback_used,
                Some(debug.lookup_variant),
            )
        })
        .unwrap_or((
            !request.context_snippets.is_empty(),
            request.context_snippets.len(),
            false,
            None,
        ));
    match result {
        PredictionResult::Ok {
            request_id,
            model_variant,
            backend,
            candidates,
            latency_ms,
        } => record_debug(
            app,
            "prediction_result",
            format!("prediction returned {} candidates", candidates.len()),
            json!({
                "status": "ok",
                "request_id": request_id,
                "burst_id": burst_id,
                "model_variant": model_variant,
                "backend": backend,
                "candidate_count": candidates.len(),
                "candidates": candidates,
                "latency_ms": latency_ms,
                "has_context": has_context,
                "context_count": context_count,
                "fallback_used": fallback_used,
                "lookup_variant": lookup_variant,
            }),
        ),
        PredictionResult::Error {
            request_id,
            model_variant,
            error,
        } => record_debug(
            app,
            "prediction_result",
            format!("prediction error: {}", error.code),
            json!({
                "status": "error",
                "request_id": request_id,
                "burst_id": burst_id,
                "model_variant": model_variant,
                "candidate_count": 0,
                "error_code": error.code,
                "error_message": error.message,
                "retryable": error.retryable,
                "has_context": has_context,
                "context_count": context_count,
                "fallback_used": fallback_used,
                "lookup_variant": lookup_variant,
            }),
        ),
    }
}

fn prediction_status_and_count(result: &PredictionResult) -> (&'static str, usize, Option<String>) {
    match result {
        PredictionResult::Ok { candidates, .. } => ("ok", candidates.len(), None),
        PredictionResult::Error { error, .. } => ("error", 0, Some(error.code.clone())),
    }
}

#[tauri::command]
fn capture_focused_destination(profile_id: String, trigger: Trigger) -> CaptureResult {
    accessibility::capture_focused_destination(&profile_id, trigger)
}

#[tauri::command]
fn commit_confirmed_text(
    destination_id: String,
    confirmed_text: String,
) -> Result<commit::CommitReport, commit::CommitError> {
    commit::commit_confirmed_text(&destination_id, &confirmed_text)
}

#[tauri::command]
fn cancel_destination(destination_id: String) -> Result<commit::CommitReport, commit::CommitError> {
    commit::cancel_destination(&destination_id)
}

#[tauri::command]
async fn capture_active_destination(app: AppHandle, trigger: Trigger) {
    let (profile_id, include_context) = {
        let engine = app.state::<EngineState>();
        let engine = engine.0.lock().unwrap();
        (
            engine.settings.active_profile.clone(),
            engine.settings.window_context,
        )
    };
    let focused = accessibility::focused_element_diagnostic();
    record_resolver_candidates(&app, "capture_requested", &focused);
    record_debug(
        &app,
        "capture_requested",
        "manual focused capture requested",
        json!({
            "source": "manual_focused_capture",
            "trigger": trigger,
            "profile_id": profile_id,
            "include_context": include_context,
            "focused": focused,
        }),
    );
    let result = accessibility::capture_focused_destination(&profile_id, trigger);
    run_capture_result(app, result, include_context, "manual_focused_capture").await;
}

/// One full burst: begin → inference → suggest. Fixture latency is replayed;
/// live results have already incurred their measured latency. The engine lock
/// is never held across the optional sleep, and stale results are dropped by
/// `apply_result` if the burst was dismissed meanwhile.
async fn run_burst_flow(app: AppHandle, input: BurstInput) {
    let begun = {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        engine.begin_burst(input)
    };
    let (snapshot, request, mode) = match begun {
        Ok(v) => v,
        Err(snapshot) => {
            emit_snapshot(&app, &snapshot);
            return;
        }
    };
    record_debug(
        &app,
        "prediction_started",
        format!("prediction started for {}", request.request_id),
        json!({
            "request_id": request.request_id,
            "burst_id": request.request_id.strip_prefix("req_").unwrap_or(&request.request_id),
            "mode": mode,
            "model_variant": request.model_variant,
            "context_count": request.context_snippets.len(),
            "personal_pattern_count": request.personal_patterns.len(),
            "draft_chars": request.draft.chars().count(),
            "draft_text": request.draft,
        }),
    );
    emit_snapshot(&app, &snapshot);
    let burst_id = request
        .request_id
        .strip_prefix("req_")
        .unwrap_or(&request.request_id)
        .to_string();

    let (result, lookup_debug) = {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        let lookup_debug = if mode == BackendMode::Fixture {
            Some(engine.backend.lookup_debug(&request))
        } else {
            None
        };
        (engine.predict(&request, mode), lookup_debug)
    };
    record_prediction_result(&app, &burst_id, &request, lookup_debug.as_ref(), &result);
    emit_metrics(&app);

    // Fixture latencies are replayed in real time so the bar's arrival is
    // honest about what live inference will feel like.
    let delay_ms = match (&result, mode) {
        (PredictionResult::Ok { latency_ms, .. }, BackendMode::Fixture) => (*latency_ms).min(900),
        (PredictionResult::Error { .. }, BackendMode::Fixture) => 250,
        _ => 0,
    };
    if delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }

    let applied = {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        engine.apply_result(&burst_id, result)
    };
    if let Some(snapshot) = applied {
        emit_snapshot(&app, &snapshot);
    }
}

async fn run_capture_result(
    app: AppHandle,
    result: CaptureResult,
    include_context: bool,
    source: &'static str,
) {
    match result {
        CaptureResult::Ready {
            burst_id,
            destination_id,
            profile_id,
            draft,
            trigger,
            caret,
        } => {
            let context_snippets = if include_context {
                accessibility::collect_context_snippets(accessibility::DEFAULT_CONTEXT_LIMIT)
            } else {
                Vec::new()
            };
            record_debug(
                &app,
                "capture_ready",
                format!("capture ready with {} chars", draft.chars().count()),
                json!({
                    "source": source,
                    "trigger": trigger,
                    "burst_id": burst_id,
                    "destination_id": destination_id,
                    "profile_id": profile_id,
                    "draft_chars": draft.chars().count(),
                    "draft_text": draft,
                    "context_count": context_snippets.len(),
                    "caret": caret,
                }),
            );
            run_burst_flow(
                app,
                BurstInput {
                    draft,
                    trigger,
                    caret,
                    context_snippets,
                    burst_id: Some(burst_id),
                    destination_id: Some(destination_id),
                    profile_id: Some(profile_id),
                },
            )
            .await;
        }
        CaptureResult::Unavailable { reason } => {
            let focused = accessibility::focused_element_diagnostic();
            record_resolver_candidates(&app, "capture_unavailable", &focused);
            record_debug(
                &app,
                "capture_unavailable",
                format!("capture unavailable: {reason}"),
                json!({
                    "source": source,
                    "reason": reason,
                    "focused": focused,
                }),
            );
            emit_snapshot(&app, &Snapshot::Unavailable { reason });
        }
    }
}

/// Demo entry point: accepts a caller-provided `capture_result` fixture and
/// feeds it through the same burst flow as real Accessibility capture.
#[tauri::command]
async fn inject_capture(app: AppHandle, result: CaptureResult) {
    run_capture_result(app, result, false, "capture_fixture").await;
}

#[tauri::command]
fn select_candidate(app: AppHandle, index: usize) -> Result<CommitOutcome, String> {
    record_debug(
        &app,
        "candidate_selected",
        format!("candidate {index} selected"),
        json!({ "selected_index": index }),
    );
    let selected = {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        engine.select(index)
    };
    let (snapshot, outcome) = match selected {
        Ok(selected) => selected,
        Err(error) => {
            record_debug(
                &app,
                "commit_failed",
                format!("commit failed: {error}"),
                json!({
                    "selected_index": index,
                    "success": false,
                    "error": error,
                }),
            );
            emit_snapshot(
                &app,
                &Snapshot::Unavailable {
                    reason: error.clone(),
                },
            );
            return Err(error);
        }
    };
    emit_snapshot(&app, &snapshot);
    let _ = app.emit("composition://committed", &outcome);
    record_debug(
        &app,
        "commit_succeeded",
        format!("committed to {}", outcome.destination_id),
        json!({
            "destination_id": outcome.destination_id,
            "selected_index": index,
            "success": true,
            "committed_chars": outcome.text.chars().count(),
            "committed_text": outcome.text,
        }),
    );
    emit_snapshot(&app, &Snapshot::Idle);
    Ok(outcome)
}

#[tauri::command]
fn move_selection(app: AppHandle, delta: i64) {
    let snapshot = {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        engine.move_selection(delta)
    };
    if let Some(snapshot) = snapshot {
        emit_snapshot(&app, &snapshot);
    }
}

#[tauri::command]
fn dismiss_suggestions(app: AppHandle) {
    let snapshot = {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        engine.dismiss()
    };
    emit_snapshot(&app, &snapshot);
}

#[tauri::command]
fn get_composition_state(app: AppHandle) -> Snapshot {
    let engine = app.state::<EngineState>();
    let engine = engine.0.lock().unwrap();
    engine.current_snapshot()
}

#[tauri::command]
fn get_settings(app: AppHandle) -> AppSettings {
    app.state::<EngineState>()
        .0
        .lock()
        .unwrap()
        .settings
        .clone()
}

#[tauri::command]
fn update_settings(app: AppHandle, settings: AppSettings) {
    {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        engine.settings = settings;
        engine.save_settings();
    }
    emit_settings(&app);
}

#[tauri::command]
fn list_profiles(app: AppHandle) -> Vec<String> {
    app.state::<EngineState>()
        .0
        .lock()
        .unwrap()
        .learning
        .list_profiles()
}

#[derive(Serialize)]
struct PatternView {
    shorthand: String,
    expansion: String,
    count: u32,
}

#[tauri::command]
fn get_patterns(app: AppHandle, profile_id: String) -> Vec<PatternView> {
    let engine = app.state::<EngineState>();
    let engine = engine.0.lock().unwrap();
    engine
        .learning
        .load_patterns(&profile_id)
        .into_iter()
        .map(|(shorthand, e)| PatternView {
            shorthand,
            expansion: e.expansion,
            count: e.count,
        })
        .collect()
}

#[tauri::command]
fn reset_profile(app: AppHandle, profile_id: String) {
    let engine = app.state::<EngineState>();
    engine.0.lock().unwrap().learning.reset_profile(&profile_id);
}

#[tauri::command]
fn get_health(app: AppHandle) -> SidecarHealth {
    app.state::<EngineState>().0.lock().unwrap().health()
}

#[tauri::command]
fn get_metrics(app: AppHandle) -> Metrics {
    app.state::<EngineState>().0.lock().unwrap().metrics.clone()
}

#[tauri::command]
fn get_debug_events(app: AppHandle, limit: usize) -> Vec<DebugEventView> {
    app.state::<DebugState>()
        .0
        .lock()
        .map(|sink| sink.recent(limit))
        .unwrap_or_default()
}

#[tauri::command]
fn record_debug_event(app: AppHandle, event: String, summary: String, payload: Value) {
    record_debug(&app, &event, summary, payload);
}

#[tauri::command]
fn set_simulate_failure(app: AppHandle, on: bool) {
    let engine = app.state::<EngineState>();
    engine.0.lock().unwrap().backend.simulate_failure = on;
}

#[tauri::command]
fn list_corpus(app: AppHandle) -> Vec<DemoCase> {
    app.state::<EngineState>()
        .0
        .lock()
        .unwrap()
        .backend
        .cases
        .clone()
}

#[derive(Serialize, Clone)]
struct ComparisonSide {
    spec: SideSpec,
    request: PredictionRequest,
    result: PredictionResult,
}

#[derive(Serialize, Clone)]
struct ComparisonReport {
    case_id: String,
    title: String,
    description: String,
    draft: String,
    left: ComparisonSide,
    right: ComparisonSide,
}

/// Runs one deterministic corpus case against both configured sides. Always
/// uses the fixture backend: this is the judged fallback path and must not
/// depend on the live sidecar.
#[tauri::command]
fn run_comparison(app: AppHandle, case_id: String) -> Result<ComparisonReport, String> {
    record_debug(
        &app,
        "comparison_requested",
        format!("comparison requested: {case_id}"),
        json!({ "case_id": case_id.clone() }),
    );
    let report = {
        let engine = app.state::<EngineState>();
        let mut engine = engine.0.lock().unwrap();
        let Some(case) = engine.backend.case(&case_id).cloned() else {
            let error = format!("unknown corpus case {case_id}");
            drop(engine);
            record_debug(
                &app,
                "comparison_failed",
                format!("comparison failed: {error}"),
                json!({
                    "case_id": case_id,
                    "error": error,
                }),
            );
            return Err(error);
        };
        let run_side = |engine: &mut Engine, side: &SideSpec, tag: &str| {
            let request = PredictionRequest {
                request_id: format!("req_cmp_{}_{}", case.case_id, tag),
                profile_id: side.profile_id.clone(),
                model_variant: side.model_variant,
                draft: case.draft.clone(),
                context_snippets: if side.use_context {
                    case.context_snippets.clone()
                } else {
                    Vec::new()
                },
                personal_patterns: engine.learning.patterns_for_request(&side.profile_id),
            };
            let result = engine.predict(&request, BackendMode::Fixture);
            ComparisonSide {
                spec: side.clone(),
                request,
                result,
            }
        };
        ComparisonReport {
            left: run_side(&mut engine, &case.left.clone(), "left"),
            right: run_side(&mut engine, &case.right.clone(), "right"),
            case_id: case.case_id,
            title: case.title,
            description: case.description,
            draft: case.draft,
        }
    };
    let (left_status, left_candidate_count, left_error_code) =
        prediction_status_and_count(&report.left.result);
    let (right_status, right_candidate_count, right_error_code) =
        prediction_status_and_count(&report.right.result);
    record_debug(
        &app,
        "comparison_result",
        format!(
            "{}: left {left_candidate_count} candidates, right {right_candidate_count} candidates",
            report.case_id
        ),
        json!({
            "case_id": report.case_id.clone(),
            "left_status": left_status,
            "right_status": right_status,
            "left_candidate_count": left_candidate_count,
            "right_candidate_count": right_candidate_count,
            "left_error_code": left_error_code,
            "right_error_code": right_error_code,
            "draft_chars": report.draft.chars().count(),
            "draft_text": report.draft.clone(),
        }),
    );
    emit_metrics(&app);
    Ok(report)
}

fn build_tray(
    app: &tauri::App,
    settings: &AppSettings,
    profiles: &[String],
) -> tauri::Result<TrayHandles> {
    let enabled = CheckMenuItem::with_id(
        app,
        "toggle_enabled",
        "Enabled",
        true,
        settings.enabled,
        None::<&str>,
    )?;
    let window_context = CheckMenuItem::with_id(
        app,
        "toggle_context",
        "Window context",
        true,
        settings.window_context,
        None::<&str>,
    )?;
    let pause_learning = CheckMenuItem::with_id(
        app,
        "toggle_learning",
        "Pause learning",
        true,
        settings.learning_paused,
        None::<&str>,
    )?;
    let mut profile_items = Vec::new();
    for profile_id in profiles {
        profile_items.push((
            profile_id.clone(),
            CheckMenuItem::with_id(
                app,
                format!("profile:{profile_id}"),
                profile_id,
                true,
                *profile_id == settings.active_profile,
                None::<&str>,
            )?,
        ));
    }
    let profile_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = profile_items
        .iter()
        .map(|(_, item)| item as &dyn tauri::menu::IsMenuItem<Wry>)
        .collect();
    let profile_menu =
        Submenu::with_id_and_items(app, "profile_menu", "Profile", true, &profile_refs)?;

    let open_settings = MenuItem::with_id(app, "open_settings", "Settings…", true, None::<&str>)?;
    let open_demo = MenuItem::with_id(app, "open_demo", "Demo & Playground…", true, None::<&str>)?;
    let capture_focused = MenuItem::with_id(
        app,
        "capture_focused",
        "Manual focused capture",
        true,
        Some("CmdOrCtrl+Shift+Space"),
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Quip", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &enabled,
            &window_context,
            &pause_learning,
            &profile_menu,
            &PredefinedMenuItem::separator(app)?,
            &capture_focused,
            &PredefinedMenuItem::separator(app)?,
            &open_settings,
            &open_demo,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    TrayIconBuilder::with_id("quip-tray")
        .icon(icon)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_tray_menu(app, event.id().as_ref()))
        .build(app)?;

    Ok(TrayHandles {
        enabled,
        window_context,
        pause_learning,
        profiles: profile_items,
    })
}

fn on_tray_menu(app: &AppHandle, id: &str) {
    match id {
        "open_settings" => show_window(app, "settings"),
        "open_demo" => show_window(app, "demo"),
        "capture_focused" => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                capture_active_destination(handle, Trigger::Shortcut).await;
            });
        }
        "quit" => app.exit(0),
        "toggle_enabled" | "toggle_context" | "toggle_learning" => {
            {
                let engine = app.state::<EngineState>();
                let mut engine = engine.0.lock().unwrap();
                match id {
                    "toggle_enabled" => engine.settings.enabled = !engine.settings.enabled,
                    "toggle_context" => {
                        engine.settings.window_context = !engine.settings.window_context
                    }
                    _ => engine.settings.learning_paused = !engine.settings.learning_paused,
                }
                engine.save_settings();
            }
            emit_settings(app);
        }
        id if id.starts_with("profile:") => {
            let profile_id = id.trim_start_matches("profile:").to_string();
            {
                let engine = app.state::<EngineState>();
                let mut engine = engine.0.lock().unwrap();
                engine.settings.active_profile = profile_id;
                engine.save_settings();
            }
            emit_settings(app);
        }
        _ => {}
    }
}

fn init_logging(log_dir: &PathBuf) {
    let _ = std::fs::create_dir_all(log_dir);
    let file_appender = tracing_appender::rolling::daily(log_dir, "quip.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    // The guard must outlive the process; leaking it is the intended pattern
    // for process-lifetime logging.
    Box::leak(Box::new(guard));
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(file_writer),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .init();
}

fn resolve_debug_dir(data_dir: &std::path::Path) -> PathBuf {
    if let Ok(dir) = std::env::var("QUIP_DEBUG_DIR") {
        return PathBuf::from(dir);
    }

    if cfg!(debug_assertions) {
        if let Ok(current_dir) = std::env::current_dir() {
            for dir in current_dir.ancestors() {
                let workspace = dir.join(".workspace");
                if workspace.exists() {
                    return workspace.join("quip-debug");
                }
            }
        }
    }

    data_dir.join("debug")
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let data_dir = match std::env::var("QUIP_DATA_DIR") {
                Ok(dir) => PathBuf::from(dir),
                Err(_) => app.path().app_data_dir()?,
            };
            init_logging(&data_dir.join("logs"));
            tracing::info!(data_dir = %data_dir.display(), "quip starting");

            let debug_dir = resolve_debug_dir(&data_dir);
            let include_debug_text = std::env::var("QUIP_DEBUG_TEXT").as_deref() == Ok("1");
            app.manage(DebugState(Mutex::new(DebugSink::new(
                debug_dir.clone(),
                include_debug_text,
            ))));
            tracing::info!(
                debug_dir = %debug_dir.display(),
                include_debug_text,
                "debug sink initialized"
            );

            let engine = Engine::new(&data_dir);
            let settings = engine.settings.clone();
            let profiles = engine.learning.list_profiles();
            app.manage(EngineState(Mutex::new(engine)));

            let handles = build_tray(app, &settings, &profiles)?;
            app.manage(TrayState(Mutex::new(Some(handles))));

            if let Ok(show) = std::env::var("QUIP_SHOW") {
                for label in show.split(',').map(str::trim).filter(|l| !l.is_empty()) {
                    show_window(app.handle(), label);
                }
            }

            // Dev/validation hook: fire the TextEdit capture fixture shortly
            // after startup so the bar can be inspected without a typist.
            if std::env::var("QUIP_DEMO_CAPTURE").as_deref() == Ok("1") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                    inject_capture(
                        handle,
                        CaptureResult::Ready {
                            burst_id: "burst_demo_env".into(),
                            destination_id: "destination_textedit".into(),
                            profile_id: "profile_default".into(),
                            draft: "cnt cm tmrw".into(),
                            trigger: Trigger::Idle,
                            caret: Rect {
                                x: 512.0,
                                y: 384.0,
                                width: 2.0,
                                height: 18.0,
                            },
                        },
                    )
                    .await;
                });
            }

            if std::env::var("QUIP_SELFTEST_LIVE").as_deref() == Ok("1") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let code = match live_selftest::run(handle.clone()).await {
                        Ok(()) => {
                            println!("LIVE SELFTEST PASS");
                            0
                        }
                        Err(error) => {
                            println!("LIVE SELFTEST FAIL: {error}");
                            1
                        }
                    };
                    handle.exit(code);
                });
            } else if std::env::var("QUIP_SELFTEST").as_deref() == Ok("1") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let code = match selftest::run(handle.clone()).await {
                        Ok(()) => {
                            println!("SELFTEST PASS");
                            0
                        }
                        Err(e) => {
                            println!("SELFTEST FAIL: {e}");
                            1
                        }
                    };
                    handle.exit(code);
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            capture_focused_destination,
            capture_active_destination,
            commit_confirmed_text,
            cancel_destination,
            inject_capture,
            select_candidate,
            move_selection,
            dismiss_suggestions,
            get_composition_state,
            get_settings,
            update_settings,
            list_profiles,
            get_patterns,
            reset_profile,
            get_health,
            get_metrics,
            get_debug_events,
            record_debug_event,
            set_simulate_failure,
            list_corpus,
            run_comparison
        ])
        .run(tauri::generate_context!())
        .expect("error while running Quip");
}

/// Headless validation of the full IME flow through the real app runtime:
/// capture -> engine -> fixture backend -> bar state, selection with in-place
/// replacement semantics, learning records, failure path, metrics.
mod selftest {
    use super::*;

    fn test_caret() -> Rect {
        Rect {
            x: 512.0,
            y: 384.0,
            width: 2.0,
            height: 18.0,
        }
    }

    fn capture(burst_id: &str, profile_id: &str, draft: &str) -> CaptureResult {
        CaptureResult::Ready {
            burst_id: burst_id.to_string(),
            destination_id: "destination_selftest".to_string(),
            profile_id: profile_id.to_string(),
            draft: draft.to_string(),
            trigger: Trigger::Idle,
            caret: test_caret(),
        }
    }

    fn state(app: &AppHandle) -> Snapshot {
        let engine = app.state::<EngineState>();
        let engine = engine.0.lock().unwrap();
        engine.current_snapshot()
    }

    fn suggesting(app: &AppHandle) -> Result<(Vec<String>, Option<String>), String> {
        match state(app) {
            Snapshot::Suggesting {
                candidates, error, ..
            } => Ok((candidates, error.map(|e| e.code))),
            other => Err(format!("expected suggesting state, got {other:?}")),
        }
    }

    fn check(name: &str, ok: bool, detail: String) -> Result<(), String> {
        if ok {
            println!("SELFTEST ok: {name}");
            Ok(())
        } else {
            Err(format!("{name}: {detail}"))
        }
    }

    pub async fn run(app: AppHandle) -> Result<(), String> {
        // 1. Shorthand burst: candidates appear. No exact-draft option is
        //    needed because the typed text is already in the destination.
        inject_capture(
            app.clone(),
            capture("st_1", "profile_default", "cnt cm tmrw"),
        )
        .await;
        let (candidates, error) = suggesting(&app)?;
        check(
            "shorthand suggests multiple candidates, best first",
            candidates.first().map(String::as_str) == Some("Can't come tomorrow.")
                && candidates.len() == 5
                && error.is_none(),
            format!("{candidates:?} {error:?}"),
        )?;

        // 2. Selecting replaces the burst in place and records learning.
        let outcome = select_candidate(app.clone(), 0)?;
        check(
            "selection replaces the burst",
            outcome.text == "Can't come tomorrow."
                && outcome.destination_id == "destination_selftest",
            format!("{outcome:?}"),
        )?;

        // 3. Personal patterns from the seeded profile personalize a burst,
        //    then a dismissal records a keep example.
        inject_capture(app.clone(), capture("st_2", "profile_a", "ship spec tn")).await;
        let (candidates, _) = suggesting(&app)?;
        check(
            "profile_a personalizes tn -> tonight",
            candidates.first().map(String::as_str) == Some("Ship spec tonight."),
            format!("{candidates:?}"),
        )?;
        dismiss_suggestions(app.clone());

        // 4. A zero-candidate result shows no bar at all.
        inject_capture(
            app.clone(),
            capture("st_3", "profile_default", "open usr/bin and q3_finl_v2.pdf"),
        )
        .await;
        check(
            "zero candidates show nothing",
            state(&app) == Snapshot::Idle,
            format!("{:?}", state(&app)),
        )?;

        // 5. Simulated adapter failure: explicit error bar, no candidates.
        set_simulate_failure(app.clone(), true);
        inject_capture(
            app.clone(),
            capture("st_4", "profile_default", "cnt cm tmrw"),
        )
        .await;
        let (candidates, error) = suggesting(&app)?;
        check(
            "failure shows explicit error with no candidates",
            candidates.is_empty() && error.as_deref() == Some("adapter_not_loaded"),
            format!("{candidates:?} {error:?}"),
        )?;
        set_simulate_failure(app.clone(), false);
        dismiss_suggestions(app.clone());

        // 6. The two demo profiles produce different candidates.
        let report = run_comparison(app.clone(), "personal".into())?;
        let texts = |side: &ComparisonSide| match &side.result {
            PredictionResult::Ok { candidates, .. } => candidates.clone(),
            PredictionResult::Error { error, .. } => vec![format!("error:{}", error.code)],
        };
        check(
            "two profiles diverge on the same shorthand",
            texts(&report.left) != texts(&report.right),
            format!("{:?} vs {:?}", texts(&report.left), texts(&report.right)),
        )?;

        // 7. Metrics counted every prediction, none schema-invalid.
        let metrics = get_metrics(app.clone());
        check(
            "metrics counted all predictions",
            metrics.requests >= 6 && metrics.schema_invalid == 0,
            format!("{metrics:?}"),
        )?;
        Ok(())
    }
}

/// Headless validation of the pushed app client against the real local
/// sidecar and Qwen server. This deliberately checks the process boundary and
/// UI state conversion without selecting or persisting a model suggestion.
mod live_selftest {
    use super::*;

    pub async fn run(app: AppHandle) -> Result<(), String> {
        let health = get_health(app.clone());
        if health.status != quip_contracts::HealthStatus::Ready || !health.loaded.base {
            return Err(format!("sidecar health was not live-ready: {health:?}"));
        }
        println!("LIVE SELFTEST ok: sidecar health is ready with base Qwen loaded");

        inject_capture(
            app.clone(),
            CaptureResult::Ready {
                burst_id: "live_selftest".to_owned(),
                destination_id: "destination_live_selftest".to_owned(),
                profile_id: "profile_default".to_owned(),
                draft: "see you tomorow".to_owned(),
                trigger: Trigger::Idle,
                caret: Rect {
                    x: 512.0,
                    y: 384.0,
                    width: 2.0,
                    height: 18.0,
                },
            },
        )
        .await;

        let snapshot = {
            let engine = app.state::<EngineState>();
            let engine = engine.0.lock().unwrap();
            engine.current_snapshot()
        };
        match snapshot {
            Snapshot::Suggesting {
                candidates,
                backend: Some(quip_contracts::Backend::Live),
                latency_ms: Some(latency_ms),
                error: None,
                ..
            } if !candidates.is_empty() && latency_ms > 0 => {
                println!(
                    "LIVE SELFTEST ok: app rendered live candidates in {latency_ms} ms: {candidates:?}"
                );
            }
            other => return Err(format!("unexpected live composition state: {other:?}")),
        }

        let metrics = get_metrics(app);
        if metrics.requests != 1 || metrics.ok != 1 || metrics.schema_invalid != 0 {
            return Err(format!("unexpected live metrics: {metrics:?}"));
        }
        println!("LIVE SELFTEST ok: app metrics recorded one schema-valid live result");
        Ok(())
    }
}
