//! `GET /api/v1/iam`: public discovery. With a peek test app secret it also
//! names the testing environment and its generation (§2.9).

use axum::{Json, extract::State};
use silicon_peek_client::api::{Compatibility, IamDiscovery};

use crate::{plane::Plane, state::AppState};

/// The discovery document.
#[allow(clippy::unused_async)] // axum handlers are async by contract
pub(crate) async fn iam(State(state): State<AppState>, plane: Plane) -> Json<IamDiscovery> {
    let config = &state.0.config;
    let testing = plane.testing_environment();
    Json(IamDiscovery {
        app_id: config.iam.app_id.clone(),
        api_version: silicon_peek_client::API_VERSION.to_owned(),
        api_base_url: config.public_origin.clone(),
        iam_base_url: config.iam.base_url.clone(),
        testing_environment_id: testing.as_ref().map(|t| t.id),
        testing_generation: testing.as_ref().map(|t| t.generation),
        testing_environment: testing,
        compatibility: Compatibility {
            cli: ">=0.1.0, <1.0.0".to_owned(),
            ipc_protocols: vec![1],
        },
    })
}
