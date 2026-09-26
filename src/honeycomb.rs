//! Honeycomb lifecycle participant (BLUEPRINT §2.9) and testing-environment
//! activity reports.
//!
//! `PUT …/operations/{op}` applies one instruction in three durable steps:
//!
//! 1. commit a **pending barrier** (binding state `pending`, operation
//!    `pending`) in the production database — from here on requests with the
//!    environment's test key are refused;
//! 2. take the environment's write fence (waits for in-flight requests), then
//!    wipe the environment's rows from the testing database when the action
//!    is `clean`, `purge` or a `retire-applications` that retires peek;
//! 3. write the final binding state and the `completed` receipt atomically.
//!
//! Replaying the same operation resumes a pending or failed one and returns
//! a completed one's receipt. Receipts never contain keys, snapshots or error
//! details. The root key is kept sealed (it authenticates activity reports
//! and routes test webhooks) and erased on purge.

use axum::http::{HeaderMap, StatusCode, header};
use serde_json::json;
use sha2::{Digest, Sha256};
use silicon_peek_client::{
    ErrorCode, Secret,
    api::participant::{Action, OperationRequest, Receipt, ReceiptState},
    constant_time_eq,
    identity::OrgId,
    timestamp::unix_now,
};
use uuid::Uuid;

use crate::{
    crypto::{Sealer, root_key_aad},
    error::{ApiError, ApiResult},
    extract::single_header,
    idempotency::request_hash,
    state::AppState,
    store::{self, bindings},
};

fn conflict(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, ErrorCode::LifecycleConflict, message)
        .with_hint("retry the original operation with the same ID, or send a newer revision")
}

/// Checks `Authorization: Bearer <PEEK_HONEYCOMB_SERVICE_TOKEN>`.
pub(crate) fn authenticate(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let Some(expected) = &state.0.config.honeycomb.service_token else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::BackendUnavailable,
            "this peek-server is not configured as a Honeycomb lifecycle participant (PEEK_HONEYCOMB_SERVICE_TOKEN is empty)",
        )
        .with_retryable(false));
    };
    let presented = single_header(headers, header::AUTHORIZATION.as_str())?
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            ApiError::unauthenticated("this route needs Authorization: Bearer <service token>")
        })?;
    let a = blake3::hash(presented.as_bytes());
    let b = blake3::hash(expected.expose().as_bytes());
    if constant_time_eq(a.as_bytes(), b.as_bytes()) {
        Ok(())
    } else {
        Err(ApiError::unauthenticated(
            "the Honeycomb service token does not match PEEK_HONEYCOMB_SERVICE_TOKEN",
        ))
    }
}

fn valid_root_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn key_sha(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

fn action_name(action: Action) -> &'static str {
    match action {
        Action::Prepare => "prepare",
        Action::Import => "import",
        Action::RefreshImport => "refresh-import",
        Action::RotateKey => "rotate-key",
        Action::Clean => "clean",
        Action::Disable => "disable",
        Action::Restore => "restore",
        Action::Purge => "purge",
        Action::RetireApplications => "retire-applications",
    }
}

fn retires_peek(op: &OperationRequest, app_id: &str) -> bool {
    op.action == Action::RetireApplications
        && op
            .retired_apps
            .as_ref()
            .is_some_and(|apps| apps.iter().any(|a| a == app_id))
}

fn receipt(op: &OperationRequest, state: ReceiptState) -> Receipt {
    Receipt {
        state,
        operation_id: op.operation_id,
        environment_id: op.environment_id,
        app_id: op.app_id.clone(),
        environment_revision: op.environment_revision,
        generation: op.generation,
        key_version: op.key_version,
        retired_apps: op.retired_apps.clone(),
    }
}

fn i64_of(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Validates the instruction against the route and peek's identity.
pub(crate) fn validate(
    op: &OperationRequest,
    org: &str,
    environment: Uuid,
    operation: Uuid,
    app_id: &str,
) -> ApiResult<()> {
    if op.org_id != org || op.environment_id != environment || op.operation_id != operation {
        return Err(ApiError::invalid_input(
            "the lifecycle instruction's org_id, environment_id and operation_id must equal the route's",
        ));
    }
    if op.app_id != app_id {
        return Err(ApiError::invalid_input(format!(
            "this participant is `{app_id}`, the instruction targets `{}`",
            op.app_id
        )));
    }
    OrgId::parse(org).map_err(ApiError::from_client)?;
    if environment.is_nil() || operation.is_nil() {
        return Err(ApiError::invalid_input(
            "lifecycle identities must not be the nil UUID",
        ));
    }
    if op.environment_revision == 0 || op.generation == 0 || op.key_version == 0 {
        return Err(ApiError::invalid_input(
            "environment_revision, generation and key_version must be positive",
        ));
    }
    match &op.testing_key {
        Some(key) if !valid_root_key(key.expose()) => Err(ApiError::invalid_input(
            "testing_key must be the environment root key: 32 ASCII letters and digits",
        )),
        None if matches!(
            op.action,
            Action::Prepare
                | Action::Import
                | Action::RefreshImport
                | Action::RotateKey
                | Action::Clean
                | Action::Restore
        ) =>
        {
            Err(ApiError::invalid_input(format!(
                "`{}` needs the environment root key in testing_key",
                action_name(op.action)
            )))
        }
        _ => Ok(()),
    }
}

/// What the barrier decided.
enum Plan {
    Done(Vec<u8>),
    Run { target_state: String, wipe: bool },
}

#[allow(clippy::too_many_lines)] // the transition table reads best as one unit
fn barrier(
    conn: &mut rusqlite::Connection,
    op: &OperationRequest,
    hash: &str,
    sealer: &Sealer,
    app_id: &str,
) -> ApiResult<Plan> {
    let tx = conn.transaction()?;
    let now = unix_now();
    let op_id = op.operation_id.hyphenated().to_string();
    let env = op.environment_id.hyphenated().to_string();
    let replay = bindings::get_operation(&tx, &op_id)?;
    if let Some(prior_op) = &replay {
        if prior_op.request_sha256 != hash || prior_op.environment_id != env {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::IdempotencyConflict,
                "this lifecycle operation ID was already used with a different instruction",
            ));
        }
        if prior_op.state == "completed" {
            return Ok(Plan::Done(prior_op.receipt.clone()));
        }
    }
    let prior = bindings::get(&tx, &env)?;
    let retry = prior.as_ref().is_some_and(|p| p.operation_id == op_id);
    let new_key = op.testing_key.as_ref().map(|k| key_sha(k.expose()));
    if let Some(p) = &prior {
        let reimport =
            p.state == "retired" && matches!(op.action, Action::Prepare | Action::Import);
        let (revision, generation, key_version) = (
            i64_of(op.environment_revision),
            i64_of(op.generation),
            i64_of(op.key_version),
        );
        let stale = p.org_id != op.org_id
            || (!retry && p.environment_revision >= revision)
            || generation < p.generation
            || key_version < p.key_version
            || (!retry && generation != p.generation && op.action != Action::Clean && !reimport)
            || (!retry
                && key_version != p.key_version
                && op.action != Action::RotateKey
                && !reimport)
            || (!retry && op.action == Action::Clean && generation <= p.generation)
            || (!retry && op.action == Action::RotateKey && key_version <= p.key_version);
        if stale {
            return Err(conflict(format!(
                "stale or incompatible lifecycle instruction for {env}: peek holds revision {}, generation {}, key version {} ({})",
                p.environment_revision, p.generation, p.key_version, p.state
            )));
        }
        if p.state == "purged" {
            return Err(conflict(format!(
                "testing environment {env} was purged; purge is irreversible"
            )));
        }
        if !retry && p.state == "pending" {
            return Err(conflict(format!(
                "operation {} is still pending for {env}; finish it before sending another",
                p.operation_id
            )));
        }
        if p.state == "retired" && !reimport && op.action != Action::Purge {
            return Err(conflict(format!(
                "peek was retired from {env}; only a new import or a purge is accepted"
            )));
        }
        if p.state == "disabled"
            && !matches!(
                op.action,
                Action::Restore
                    | Action::Purge
                    | Action::Disable
                    | Action::Clean
                    | Action::RotateKey
            )
        {
            return Err(conflict(format!(
                "testing environment {env} is disabled; restore it first"
            )));
        }
        if let Some(sha) = &new_key {
            let same = constant_time_eq(sha.as_bytes(), p.testing_key_sha256.as_bytes());
            if key_version == p.key_version && !same {
                return Err(conflict("the root key changed without a new key_version"));
            }
            if !retry && op.action == Action::RotateKey && same {
                return Err(conflict("rotate-key must deliver a new root key"));
            }
        }
    } else if !matches!(op.action, Action::Prepare | Action::Import) {
        return Err(conflict(format!(
            "Honeycomb must prepare peek in {env} before `{}`",
            action_name(op.action)
        )));
    }

    let target_state = if let Some(prior_op) = &replay {
        prior_op.target_state.clone()
    } else if op.action == Action::Purge {
        "purged".to_owned()
    } else if retires_peek(op, app_id) {
        "retired".to_owned()
    } else if op.action == Action::Disable
        || (prior.as_ref().is_some_and(|p| p.state == "disabled") && op.action != Action::Restore)
    {
        "disabled".to_owned()
    } else {
        "active".to_owned()
    };
    let (sealed_key, sha) = match &op.testing_key {
        Some(key) => (
            sealer.seal(&root_key_aad(&env), key.expose().as_bytes())?,
            key_sha(key.expose()),
        ),
        None => match &prior {
            Some(p) => (p.sealed_root_key.clone(), p.testing_key_sha256.clone()),
            None => {
                return Err(ApiError::invalid_input(
                    "testing_key is required to prepare an environment",
                ));
            }
        },
    };
    let wipe = matches!(op.action, Action::Clean | Action::Purge) || retires_peek(op, app_id);
    bindings::upsert_pending(
        &tx,
        &bindings::PendingBinding {
            environment_id: &env,
            org_id: &op.org_id,
            environment_revision: i64_of(op.environment_revision),
            generation: i64_of(op.generation),
            key_version: i64_of(op.key_version),
            testing_key_sha256: &sha,
            sealed_root_key: &sealed_key,
            operation_id: &op_id,
            reset_activity: wipe,
        },
        now,
    )?;
    let pending = serde_json::to_vec(&receipt(op, ReceiptState::Pending))
        .map_err(|e| ApiError::internal(format!("serializing a receipt failed: {e}")))?;
    if replay.is_some() {
        bindings::update_operation(&tx, &op_id, "pending", &pending, now)?;
    } else {
        bindings::insert_operation(
            &tx,
            &bindings::NewOperation {
                operation_id: &op_id,
                environment_id: &env,
                org_id: &op.org_id,
                action: action_name(op.action),
                request_sha256: hash,
                target_state: &target_state,
                receipt: &pending,
            },
            now,
        )?;
    }
    tx.commit()?;
    Ok(Plan::Run { target_state, wipe })
}

/// Applies (or replays) one lifecycle instruction.
pub(crate) async fn apply(state: &AppState, op: OperationRequest) -> ApiResult<Receipt> {
    let app_id = state.0.config.iam.app_id.clone();
    let canonical = serde_json::to_vec(&op)
        .map_err(|e| ApiError::internal(format!("serializing an instruction failed: {e}")))?;
    let hash = request_hash(&[&canonical]);
    let plan = {
        let (op, sealer, app_id) = (op.clone(), state.0.sealer.clone(), app_id.clone());
        state
            .0
            .production_db
            .call(move |conn| barrier(conn, &op, &hash, &sealer, &app_id))
            .await?
    };
    let (target_state, wipe) = match plan {
        Plan::Done(bytes) => {
            return serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::internal("a stored lifecycle receipt is unreadable"));
        }
        Plan::Run { target_state, wipe } => (target_state, wipe),
    };
    let env = op.environment_id.hyphenated().to_string();
    let op_id = op.operation_id.hyphenated().to_string();
    let result = async {
        let _fence = state.env_lock(op.environment_id).write_owned().await;
        if wipe {
            let ctx = env.clone();
            let removed = state
                .0
                .testing_db
                .call(move |conn| Ok(store::wipe_context(conn, &ctx)?))
                .await?;
            tracing::info!(environment = %env, removed, action = action_name(op.action), "wiped testing-environment data");
        }
        let completed = serde_json::to_vec(&receipt(&op, ReceiptState::Completed))
            .map_err(|e| ApiError::internal(format!("serializing a receipt failed: {e}")))?;
        let (env, op_id, target) = (env.clone(), op_id.clone(), target_state.clone());
        state
            .0
            .production_db
            .call(move |conn| {
                let tx = conn.transaction()?;
                let now = unix_now();
                bindings::finish(&tx, &env, &target, now)?;
                bindings::update_operation(&tx, &op_id, "completed", &completed, now)?;
                tx.commit()?;
                Ok(())
            })
            .await
    }
    .await;
    if let Err(error) = result {
        let failed = serde_json::to_vec(&receipt(&op, ReceiptState::Failed)).unwrap_or_default();
        let op_id = op_id.clone();
        if let Err(e) = state
            .0
            .production_db
            .call(move |conn| {
                Ok(bindings::update_operation(
                    conn,
                    &op_id,
                    "failed",
                    &failed,
                    unix_now(),
                )?)
            })
            .await
        {
            tracing::error!(code = %e.code(), "could not record a failed lifecycle operation");
        }
        return Err(error);
    }
    Ok(receipt(&op, ReceiptState::Completed))
}

/// The stored receipt of an operation (lost-response recovery).
pub(crate) async fn stored_receipt(
    state: &AppState,
    org: String,
    environment: Uuid,
    operation: Uuid,
) -> ApiResult<Receipt> {
    let op_id = operation.hyphenated().to_string();
    let env = environment.hyphenated().to_string();
    let found = state
        .0
        .production_db
        .call(move |conn| Ok(bindings::get_operation(conn, &op_id)?))
        .await?;
    match found {
        Some(op) if op.environment_id == env && op.org_id == org => {
            serde_json::from_slice(&op.receipt)
                .map_err(|_| ApiError::internal("a stored lifecycle receipt is unreadable"))
        }
        _ => Err(ApiError::not_found(format!(
            "no lifecycle operation {operation} for environment {environment} in org {org}"
        ))),
    }
}

/// Reports testing-environment activity to Honeycomb (`POST
/// /api/v1/environments/{id}/apps/peek/activity`, authenticated with the
/// environment root key). Returns how many environments were acknowledged.
pub async fn report_activity(state: &AppState) -> usize {
    let due = match state
        .0
        .production_db
        .call(|conn| Ok(bindings::activity_due(conn)?))
        .await
    {
        Ok(due) => due,
        Err(e) => {
            tracing::warn!(code = %e.code(), "cannot read testing environments with unreported activity");
            return 0;
        }
    };
    let mut reported = 0;
    for binding in due {
        let Some(at) = binding.last_activity_at else {
            continue;
        };
        let root = match state.0.sealer.open_string(
            &root_key_aad(&binding.environment_id),
            &binding.sealed_root_key,
        ) {
            Ok(root) => Secret::new(root),
            Err(e) => {
                tracing::warn!(environment = %binding.environment_id, code = %e.code(), "cannot open a testing-environment root key");
                continue;
            }
        };
        let url = format!(
            "{}/api/v1/environments/{}/apps/{}/activity",
            state.0.config.honeycomb.base_url, binding.environment_id, state.0.config.iam.app_id
        );
        let response = state
            .0
            .http
            .post(url)
            .timeout(std::time::Duration::from_secs(5))
            .header("X-Testing-Environment-Key", root.expose())
            .header(
                "Idempotency-Key",
                format!(
                    "peek:{}:{}:{}:{at}",
                    binding.environment_id, binding.generation, binding.key_version
                ),
            )
            .json(&json!({"generation": binding.generation, "key_version": binding.key_version}))
            .send()
            .await;
        match response {
            Ok(r) if r.status().is_success() => {
                let b = binding.clone();
                match state
                    .0
                    .production_db
                    .call(move |conn| Ok(bindings::mark_activity_reported(conn, &b, at)?))
                    .await
                {
                    Ok(()) => reported += 1,
                    Err(e) => tracing::warn!(code = %e.code(), "cannot record reported activity"),
                }
            }
            Ok(r) => {
                tracing::warn!(environment = %binding.environment_id, status = r.status().as_u16(), "Honeycomb refused an activity report; it will be retried");
            }
            Err(_) => {
                tracing::warn!(environment = %binding.environment_id, "Honeycomb could not be reached for an activity report; it will be retried");
            }
        }
    }
    reported
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(
        action: &str,
        revision: u64,
        generation: u64,
        key: Option<&str>,
    ) -> Result<OperationRequest, serde_json::Error> {
        let mut v = json!({
            "operation_id": Uuid::now_v7(), "environment_id": Uuid::from_u128(7), "org_id": "tos", "app_id": "peek",
            "environment_revision": revision, "generation": generation, "key_version": 1, "action": action,
        });
        if let Some(key) = key {
            v["testing_key"] = json!(key);
        }
        serde_json::from_value(v)
    }

    #[test]
    fn validation_rules() -> Result<(), serde_json::Error> {
        let good = op("prepare", 1, 1, Some(&"a".repeat(32)))?;
        let (env, id) = (good.environment_id, good.operation_id);
        assert!(validate(&good, "tos", env, id, "peek").is_ok());
        assert!(validate(&good, "other", env, id, "peek").is_err());
        assert!(validate(&good, "tos", Uuid::from_u128(8), id, "peek").is_err());
        assert!(validate(&good, "tos", env, id, "dm").is_err());
        let short = op("prepare", 1, 1, Some("abc"))?;
        assert!(validate(&short, "tos", env, short.operation_id, "peek").is_err());
        let keyless = op("clean", 2, 2, None)?;
        assert!(validate(&keyless, "tos", env, keyless.operation_id, "peek").is_err());
        let purge = op("purge", 2, 1, None)?;
        assert!(validate(&purge, "tos", env, purge.operation_id, "peek").is_ok());
        let zero = op("prepare", 0, 1, Some(&"a".repeat(32)))?;
        assert!(validate(&zero, "tos", env, zero.operation_id, "peek").is_err());
        Ok(())
    }

    #[test]
    fn barrier_transitions() -> Result<(), Box<dyn std::error::Error>> {
        let mut conn = crate::store::testing::memory()?;
        let sealer = Sealer::new(&[1; 32]);
        let key = "a".repeat(32);
        let prepare = op("prepare", 1, 1, Some(&key))?;
        let hash = |o: &OperationRequest| -> Result<String, serde_json::Error> {
            Ok(request_hash(&[&serde_json::to_vec(o)?]))
        };
        let plan = barrier(&mut conn, &prepare, &hash(&prepare)?, &sealer, "peek")
            .map_err(|e| e.to_string())?;
        assert!(
            matches!(plan, Plan::Run { ref target_state, wipe: false } if target_state == "active")
        );
        // A second, different operation while the first is pending is refused.
        let other = op("import", 2, 1, Some(&key))?;
        assert!(barrier(&mut conn, &other, &hash(&other)?, &sealer, "peek").is_err());
        // The same operation resumes.
        assert!(barrier(&mut conn, &prepare, &hash(&prepare)?, &sealer, "peek").is_ok());
        // Same ID, different body.
        let mut altered = prepare.clone();
        altered.environment_revision = 9;
        assert!(barrier(&mut conn, &altered, &hash(&altered)?, &sealer, "peek").is_err());
        let env = prepare.environment_id.hyphenated().to_string();
        bindings::finish(&conn, &env, "active", 1)?;
        // Clean must advance the generation.
        let clean_same = op("clean", 2, 1, Some(&key))?;
        assert!(barrier(&mut conn, &clean_same, &hash(&clean_same)?, &sealer, "peek").is_err());
        let clean = op("clean", 2, 2, Some(&key))?;
        let plan = barrier(&mut conn, &clean, &hash(&clean)?, &sealer, "peek")
            .map_err(|e| e.to_string())?;
        assert!(matches!(plan, Plan::Run { wipe: true, .. }));
        bindings::finish(&conn, &env, "active", 2)?;
        // A stale revision is refused.
        let stale = op("import", 2, 2, Some(&key))?;
        assert!(barrier(&mut conn, &stale, &hash(&stale)?, &sealer, "peek").is_err());
        Ok(())
    }
}
