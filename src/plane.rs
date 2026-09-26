//! Data-plane resolution (BLUEPRINT §2.9).
//!
//! A request without `X-Testing-Environment-Key` is production. With it, the
//! header carries a **peek** test app secret; the plane is resolved only from
//! IAM's validation of that secret (never from a client-supplied UUID), must
//! have a Honeycomb participant binding, and mutations (except login and
//! refresh) must carry the binding's current
//! `X-Testing-Environment-Generation`. There is never a production fallback.

use std::sync::Arc;

use axum::{
    extract::FromRequestParts,
    http::{Method, StatusCode, request::Parts},
};
use silicon_peek_client::{
    ErrorCode,
    api::{TestingEnvironment, headers, routes},
    identity::{Context, DataContext, TestingSecret},
    timestamp::unix_now,
};
use tokio::sync::OwnedRwLockReadGuard;
use uuid::Uuid;

use crate::{
    db::Db,
    error::{ApiError, ApiResult},
    extract::single_header,
    iam::{IamPlane, api_code, upstream},
    state::AppState,
    store,
};

/// The testing environment a request selected.
#[derive(Clone, Debug)]
pub(crate) struct TestingInfo {
    pub(crate) environment_id: Uuid,
    pub(crate) name: String,
    pub(crate) generation: u64,
}

/// The resolved data plane of a request.
pub(crate) struct Plane {
    pub(crate) ctx: Context,
    pub(crate) db: Db,
    iam: Option<Arc<dyn IamPlane>>,
    pub(crate) testing: Option<TestingInfo>,
    /// Held for the whole request so a clean cannot interleave with it.
    _fence: Option<OwnedRwLockReadGuard<()>>,
}

impl Plane {
    /// IAM for this plane, or `503 iam_misconfigured` while production has
    /// no app secret.
    pub(crate) fn iam(&self) -> ApiResult<Arc<dyn IamPlane>> {
        self.iam.clone().ok_or_else(|| {
            ApiError::iam_misconfigured(
                "peek-server has no IAM app secret yet (PEEK_IAM_APP_SECRET is empty), so it cannot talk to IAM",
            )
        })
    }

    /// `production` or the environment UUID.
    pub(crate) fn ctx_string(&self) -> String {
        self.ctx.as_string()
    }

    /// Whether this is a testing plane.
    pub(crate) fn is_testing(&self) -> bool {
        self.testing.is_some()
    }

    /// The coarse context Ting payloads carry.
    pub(crate) fn data_context(&self) -> DataContext {
        self.ctx.data_context()
    }

    /// The environment as the API describes it.
    pub(crate) fn testing_environment(&self) -> Option<TestingEnvironment> {
        self.testing.as_ref().map(|t| TestingEnvironment {
            id: t.environment_id,
            name: t.name.clone(),
            generation: t.generation,
        })
    }
}

impl FromRequestParts<AppState> for Plane {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        resolve(parts, state).await
    }
}

fn is_mutation(parts: &Parts) -> bool {
    !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS)
        && !matches!(parts.uri.path(), routes::LOGIN | routes::REFRESH)
}

fn secret_rejected(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::TestingSecretInvalid,
        message,
    )
    .with_hint("get the current peek test app secret with `honeycomb --test <env> apps rotate-secret 'peek'` and pass it with --app-secret-file -")
}

fn not_prepared(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        ErrorCode::EnvironmentNotPrepared,
        message,
    )
    .with_hint("the Honeycomb operator must list peek in HONEYCOMB_LIFECYCLE_PARTICIPANTS, then import peek into the environment")
}

async fn resolve(parts: &Parts, state: &AppState) -> ApiResult<Plane> {
    let key = single_header(&parts.headers, headers::TESTING_KEY)?;
    let generation = match single_header(&parts.headers, headers::TESTING_GENERATION)? {
        None => None,
        Some(raw) => Some(raw.parse::<u64>().ok().filter(|g| *g > 0).ok_or_else(|| {
            ApiError::invalid_input(format!(
                "X-Testing-Environment-Generation must be a positive integer, got `{raw}`"
            ))
        })?),
    };
    match key {
        Some(key) => resolve_testing(parts, state, key, generation).await,
        None if generation.is_some() => Err(ApiError::invalid_input(
            "X-Testing-Environment-Generation was sent without X-Testing-Environment-Key; production requests carry neither",
        )),
        None => Ok(Plane {
            ctx: Context::Production,
            db: state.0.production_db.clone(),
            iam: state.0.iam.production(),
            testing: None,
            _fence: None,
        }),
    }
}

async fn resolve_testing(
    parts: &Parts,
    state: &AppState,
    key: &str,
    generation: Option<u64>,
) -> ApiResult<Plane> {
    let secret = TestingSecret::parse(key).map_err(ApiError::from_client)?;
    let (iam, context) = state.0.iam.testing(&secret).await.map_err(|e| {
        match api_code(&e) {
            Some((_, 400 | 401 | 403 | 404)) => secret_rejected(
                "IAM does not recognise this peek testing app secret (it is unknown, rotated, or its environment was cleaned or deleted)",
            ),
            _ => upstream(&e, "validate the testing app secret", true),
        }
    })?;
    let environment_id = context.environment_id;
    let Some(meta) = context
        .environment
        .as_ref()
        .filter(|m| m.environment_id == environment_id && m.version >= 1)
        .filter(|_| {
            context.application.app_id == state.0.config.iam.app_id && !environment_id.is_nil()
        })
    else {
        return Err(secret_rejected(format!(
            "the testing app secret belongs to application `{}`, not a peek testing environment",
            context.application.app_id
        )));
    };
    let (name, env_org) = (meta.name.clone(), meta.org_id.clone());

    let fence = state.env_lock(environment_id).read_owned().await;
    let env = environment_id.hyphenated().to_string();
    let binding = {
        let env = env.clone();
        state
            .0
            .production_db
            .call(move |conn| Ok(store::bindings::get(conn, &env)?))
            .await?
    };
    let current = check_binding(binding.as_ref(), &env, &env_org)?;
    check_generation(parts, &env, current, generation)?;
    {
        let env = env.clone();
        if let Err(e) = state
            .0
            .production_db
            .call(move |conn| Ok(store::bindings::touch_activity(conn, &env, unix_now())?))
            .await
        {
            tracing::warn!(code = %e.code(), "could not record testing-environment activity");
        }
    }
    Ok(Plane {
        ctx: Context::Testing(environment_id),
        db: state.0.testing_db.clone(),
        iam: Some(iam),
        testing: Some(TestingInfo {
            environment_id,
            name,
            generation: current,
        }),
        _fence: Some(fence),
    })
}

/// The binding must exist, be active and belong to the environment's org;
/// returns its generation.
fn check_binding(
    binding: Option<&store::bindings::Binding>,
    env: &str,
    env_org: &str,
) -> ApiResult<u64> {
    let Some(binding) = binding else {
        return Err(not_prepared(format!(
            "Honeycomb has not prepared peek in testing environment {env}"
        )));
    };
    match binding.state.as_str() {
        "active" => {}
        "pending" => {
            return Err(not_prepared(format!(
                "a Honeycomb lifecycle operation is still running for testing environment {env}"
            ))
            .with_retry_after(5));
        }
        "disabled" => {
            return Err(not_prepared(format!(
                "testing environment {env} is disabled; restore it in Honeycomb first"
            )));
        }
        other => {
            return Err(secret_rejected(format!(
                "testing environment {env} is {other} for peek"
            )));
        }
    }
    if !env_org.is_empty() && env_org != binding.org_id {
        return Err(not_prepared(format!(
            "testing environment {env} belongs to org `{env_org}`, but peek was prepared for org `{}`",
            binding.org_id
        )));
    }
    Ok(u64::try_from(binding.generation).unwrap_or(0))
}

/// Mutations must carry the current generation; any request carrying a
/// generation must carry the current one.
fn check_generation(
    parts: &Parts,
    env: &str,
    current: u64,
    generation: Option<u64>,
) -> ApiResult<()> {
    let outdated = match generation {
        Some(g) => g != current,
        None => is_mutation(parts),
    };
    if !outdated {
        return Ok(());
    }
    let message = match generation {
        Some(g) => format!(
            "testing environment {env} is at generation {current}, but the request was bound to generation {g} (the environment was cleaned)"
        ),
        None => format!(
            "this change must carry X-Testing-Environment-Generation (testing environment {env} is at generation {current})"
        ),
    };
    Err(ApiError::new(
        StatusCode::CONFLICT,
        ErrorCode::TestingGenerationChanged,
        message,
    )
    .with_hint("read GET /api/v1/iam with the testing key again (peek status does this), log in again if needed, then retry")
    .with_details(serde_json::json!({"generation": current})))
}
