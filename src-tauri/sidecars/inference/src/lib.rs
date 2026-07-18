//! Deterministic local inference sidecar used before live model artifacts are
//! installed. The fixture backend and the later live backend share the Phase 0
//! prediction and health contracts.

mod fixture;
mod live;
mod protocol;

pub use fixture::{FixtureBackend, FixtureBackendError};
pub use live::{LiveBackend, LiveBackendError};
pub use protocol::serve;

use quip_contracts::{PredictionRequest, PredictionResult, SidecarHealth};

pub trait InferenceBackend {
    fn health(&self, case_id: Option<&str>) -> SidecarHealth;
    fn predict(&self, request: &PredictionRequest) -> PredictionResult;
}
