//! `GET /api/v1/accounts`: public discovery. With a peek test app secret it also
//! names the testing environment and its generation (§2.9).

use axum::{Json, extract::State};
use silicon_peek_client::api::{AccountsDiscovery, Compatibility};

use crate::{plane::Plane, state::AppState};

/// The discovery document.
#[allow(clippy::unused_async)] // axum handlers are async by contract
pub(crate) async fn accounts(
    State(state): State<AppState>,
    _plane: Plane,
) -> Json<AccountsDiscovery> {
    let config = &state.0.config;
    Json(AccountsDiscovery {
        app_id: config.accounts.app_id.clone(),
        api_version: silicon_peek_client::API_VERSION.to_owned(),
        api_base_url: config.public_origin.clone(),
        accounts_base_url: config.accounts.base_url.clone(),
        compatibility: Compatibility {
            cli: ">=0.1.0, <1.0.0".to_owned(),
            ipc_protocols: vec![1],
        },
    })
}
