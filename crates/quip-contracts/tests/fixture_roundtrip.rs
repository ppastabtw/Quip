//! Validates this crate as a consumer of the shared Phase 0 fixtures, per the
//! acceptance rule in `docs/phase-0-contracts.md`: a boundary is accepted only
//! after one producer and one consumer validate the same fixture.

use quip_contracts::{CaptureResult, FixtureFile, PredictionResult, Trigger};

fn load_raw() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/fixtures/phase-0-examples.json");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
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

#[test]
fn fixtures_round_trip_exactly() {
    let raw = load_raw();
    let typed: FixtureFile = serde_json::from_str(&raw).expect("fixtures must parse into types");
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&typed).unwrap();
    assert_eq!(
        original, reserialized,
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
        }
    );
}
