//! Idempotent POSTs (BLUEPRINT §5.2 `idempotency` table).
//!
//! A key is bound to `(ctx, scope, key)` and to the SHA-256 of the request
//! it first arrived with:
//!
//! - same key, same request, finished → the stored response is replayed;
//! - same key, different request → `409 idempotency_conflict`;
//! - same key while the first execution still runs → `409
//!   idempotency_in_progress` (retryable); a reservation older than two
//!   minutes is treated as abandoned and taken over;
//! - only successful responses are stored, so a failed attempt never poisons
//!   its key and the client's retry with the same key runs again.
//!
//! The work runs in its own task, so a client that disconnects mid-request
//! still gets the stored response on its retry instead of a second execution.

use std::future::Future;

use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use silicon_peek_client::{ErrorCode, ids::IdempotencyKey, timestamp::unix_now};

use crate::{
    db::Db,
    error::{ApiError, ApiResult, REQUEST_ID, current_request_id},
    store,
};

/// Age after which an unfinished reservation is considered abandoned.
const ABANDONED_AFTER_SECS: i64 = 120;

/// Hex SHA-256 of request material.
pub(crate) fn request_hash(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

enum Reservation {
    Fresh,
    Replay { status: u16, body: Vec<u8> },
}

async fn reserve(
    db: &Db,
    ctx: &str,
    scope: &'static str,
    key: &str,
    hash: &str,
) -> ApiResult<Reservation> {
    let (ctx, key, hash) = (ctx.to_owned(), key.to_owned(), hash.to_owned());
    db.call(move |conn| {
        let tx = conn.transaction()?;
        let now = unix_now();
        let outcome = match store::idempotency::get(&tx, &ctx, scope, &key)? {
            None => {
                store::idempotency::insert_pending(&tx, &ctx, scope, &key, &hash, now)?;
                Ok(Reservation::Fresh)
            }
            Some(record) if record.request_sha256 != hash => Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::IdempotencyConflict,
                "this Idempotency-Key was already used for a different request",
            )
            .with_hint(
                "reuse a key only to retry the exact same request; use a new key for a new request",
            )),
            Some(store::idempotency::Record {
                status: Some(status),
                response: Some(body),
                ..
            }) => Ok(Reservation::Replay { status, body }),
            Some(record) if record.created_at > now - ABANDONED_AFTER_SECS => Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::IdempotencyInProgress,
                "the first request with this Idempotency-Key is still being processed",
            )
            .with_hint("retry the same request in a moment; it will return the original result")
            .with_retry_after(2)),
            Some(_) => {
                store::idempotency::renew_pending(&tx, &ctx, scope, &key, now)?;
                Ok(Reservation::Fresh)
            }
        };
        tx.commit()?;
        outcome
    })
    .await
}

fn json_response(status: StatusCode, body: Vec<u8>, replayed: bool) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if replayed {
        headers.insert("idempotent-replayed", HeaderValue::from_static("true"));
    }
    response
}

/// One idempotent execution.
pub(crate) struct Idempotent {
    pub(crate) db: Db,
    pub(crate) ctx: String,
    pub(crate) scope: &'static str,
    pub(crate) key: IdempotencyKey,
    pub(crate) request_sha256: String,
}

impl Idempotent {
    /// Runs `work` at most once per key and answers `success` with its JSON,
    /// or replays the stored answer.
    pub(crate) async fn run<T, F>(self, success: StatusCode, work: F) -> ApiResult<Response>
    where
        F: Future<Output = ApiResult<T>> + Send + 'static,
        T: Serialize + Send + 'static,
    {
        let Self {
            db,
            ctx,
            scope,
            key,
            request_sha256,
        } = self;
        match reserve(&db, &ctx, scope, key.as_str(), &request_sha256).await? {
            Reservation::Replay { status, body } => {
                let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
                return Ok(json_response(status, body, true));
            }
            Reservation::Fresh => {}
        }
        let task = async move {
            match work.await {
                Ok(value) => {
                    let body = serde_json::to_vec(&value).map_err(|e| {
                        ApiError::internal(format!("serializing a response failed: {e}"))
                    })?;
                    let (stored, k, c) = (body.clone(), key.as_str().to_owned(), ctx.clone());
                    if let Err(e) = db
                        .call(move |conn| {
                            store::idempotency::complete(
                                conn,
                                &c,
                                scope,
                                &k,
                                success.as_u16(),
                                &stored,
                            )?;
                            Ok(())
                        })
                        .await
                    {
                        tracing::warn!(scope, code = %e.code(), "could not store an idempotent response; a retry will run again");
                    }
                    Ok(body)
                }
                Err(error) => {
                    let (k, c) = (key.as_str().to_owned(), ctx.clone());
                    if let Err(e) = db
                        .call(move |conn| {
                            store::idempotency::release(conn, &c, scope, &k)?;
                            Ok(())
                        })
                        .await
                    {
                        tracing::warn!(scope, code = %e.code(), "could not release an idempotency reservation");
                    }
                    Err(error)
                }
            }
        };
        let handle = match current_request_id() {
            Some(id) => tokio::spawn(REQUEST_ID.scope(id, task)),
            None => tokio::spawn(task),
        };
        match handle.await {
            Ok(Ok(body)) => Ok(json_response(success, body, false)),
            Ok(Err(error)) => Err(error),
            Err(join) => {
                tracing::error!(error = %join, scope, "idempotent task failed");
                Err(ApiError::internal("the request's worker task failed"))
            }
        }
    }
}
