//! Validates this crate as a consumer of the shared Phase 0 fixtures, per the
//! acceptance rule in `docs/phase-0-contracts.md`: a boundary is accepted only
//! after one producer and one consumer validate the same fixture.

use quip_contracts::{CaptureResult, FixtureFile, PredictionResult, Rect, Trigger};

fn load_raw() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/fixtures/phase-0-examples.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// serde_json::Value treats 512 and 512.0 as unequal; the wire format does
/// not. Normalize every number to f64 before comparing.
fn normalize(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(n) => serde_json::json!(n.as_f64().unwrap()),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}

fn capture_case(case_id: &str) -> CaptureResult {
    let typed: FixtureFile = serde_json::from_str(&load_raw()).unwrap();
    typed
        .capture_results
        .into_iter()
        .find(|case| case.case_id == case_id)
        .unwrap_or_else(|| panic!("missing capture case {case_id}"))
        .result
}

fn unavailable_capture_reason(case_id: &str) -> String {
    match capture_case(case_id) {
        CaptureResult::Unavailable { reason } => reason,
        CaptureResult::Ready { .. } => panic!("capture case {case_id} should be unavailable"),
    }
}

#[test]
fn fixtures_round_trip_exactly() {
    let raw = load_raw();
    let typed: FixtureFile = serde_json::from_str(&raw).expect("fixtures must parse into types");
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&typed).unwrap();
    assert_eq!(
        normalize(original),
        normalize(reserialized),
        "typed contracts must not drop, rename, or invent fields"
    );
}

#[test]
fn fixture_results_satisfy_invariants() {
    let typed: FixtureFile = serde_json::from_str(&load_raw()).unwrap();
    assert_eq!(typed.version, 0);
    assert!(!typed.prediction_exchanges.is_empty());
    assert!(!typed.capture_results.is_empty());
    assert!(!typed.health_cases.is_empty());

    for exchange in &typed.prediction_exchanges {
        exchange
            .result
            .validate()
            .unwrap_or_else(|e| panic!("case {}: {e}", exchange.case_id));
        assert_eq!(
            exchange.request.request_id,
            exchange.result.request_id(),
            "case {}: result must echo the request id",
            exchange.case_id
        );
        let (PredictionResult::Ok { model_variant, .. }
        | PredictionResult::Error { model_variant, .. }) = &exchange.result;
        assert_eq!(
            *model_variant, exchange.request.model_variant,
            "case {}: result must echo the request model variant",
            exchange.case_id
        );
    }

    let candidate_counts: Vec<usize> = typed
        .prediction_exchanges
        .iter()
        .filter_map(|exchange| match &exchange.result {
            PredictionResult::Ok { candidates, .. } => Some(candidates.len()),
            PredictionResult::Error { .. } => None,
        })
        .collect();
    assert!(
        candidate_counts.contains(&0),
        "fixtures must prove zero candidates"
    );
    assert!(
        candidate_counts.contains(&5),
        "fixtures must prove five candidates"
    );
}

#[test]
fn textedit_ready_capture_fixture_matches_shared_rust_contract() {
    let fixture = capture_case("textedit_ready");

    assert_eq!(
        fixture,
        CaptureResult::Ready {
            burst_id: "burst_textedit".to_string(),
            destination_id: "destination_textedit".to_string(),
            profile_id: "profile_default".to_string(),
            draft: "cnt cm tmrw".to_string(),
            trigger: Trigger::Idle,
            caret: Rect {
                x: 512.0,
                y: 384.0,
                width: 2.0,
                height: 18.0,
            },
        }
    );
}

#[test]
fn secure_field_capture_fixture_matches_shared_rust_contract() {
    let reason = unavailable_capture_reason("secure_field");

    assert_eq!(reason, "secure_field");
}
