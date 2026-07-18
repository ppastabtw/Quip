use quip_contracts::FixtureFile;
use quip_inference_sidecar::FixtureBackend;

fn fixtures() -> FixtureFile {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/fixtures/phase-0-examples.json"
    )))
    .unwrap()
}

#[test]
fn producer_replays_every_shared_prediction_fixture() {
    let backend = FixtureBackend::embedded().unwrap();

    for exchange in fixtures().prediction_exchanges {
        let produced = backend.predict(&exchange.request);
        produced
            .validate()
            .unwrap_or_else(|error| panic!("case {}: {error}", exchange.case_id));
        assert_eq!(
            produced, exchange.result,
            "case {} must pass through the real producer",
            exchange.case_id
        );
        if let quip_contracts::PredictionResult::Ok { candidates, .. } = &produced {
            assert!(
                !candidates.contains(&exchange.request.draft),
                "case {} returned the exact draft",
                exchange.case_id
            );
        }
    }
}

#[test]
fn producer_replays_every_shared_health_fixture() {
    let backend = FixtureBackend::embedded().unwrap();

    for case in fixtures().health_cases {
        assert_eq!(
            backend.health(Some(&case.case_id)),
            case.health,
            "{}",
            case.case_id
        );
    }
}
