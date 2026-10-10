//! Persistent ACCOUNTS sessions for browser, CLI and daemon clients.
use crate::{
    accounts::{api_code, token_shape_ok, upstream},
    auth::{self, Bearer},
    error::{ApiError, ApiResult},
    extract::JsonBody,
    plane::Plane,
    state::AppState,
    store,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use silicon_accounts_client::{Error as AccountsError, TokenResponse};
use silicon_peek_client::{
    ErrorCode, Secret,
    api::{LoginRequest, LogoutRequest, Me, RefreshRequest, SessionResponse, TingEnrollment},
    identity::{AccountId, Actor, ActorId},
    timestamp::{Timestamp, unix_now},
};
use std::{future::Future, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeExchange {
    code: String,
    redirect_uri: String,
    code_verifier: String,
}

fn rejected() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        ErrorCode::SessionRejected,
        "This session expired or was signed out",
    )
    .with_hint("Sign in again through ACCOUNTS")
}
fn token_error(error: &AccountsError) -> ApiError {
    match api_code(error) {
        Some(("invalid_grant" | "invalid_token" | "access_denied" | "expired_token", _)) => {
            rejected()
        }
        _ => upstream(error, "exchange this token", false),
    }
}

#[derive(Serialize, Deserialize)]
struct CachedTokens {
    tokens: TokenResponse,
    issued_at: i64,
}

// Accounts refresh tokens are single-use. Persist both the claim and the encrypted
// result so concurrent callers and lost client responses never rotate one twice.
async fn exchange_once<F>(
    state: &AppState,
    plane: &Plane,
    kind: &str,
    credential: &str,
    exchange: F,
) -> ApiResult<TokenResponse>
where
    F: Future<Output = Result<TokenResponse, AccountsError>>,
{
    let fingerprint = hex::encode(Sha256::digest(format!("{kind}:{credential}")));
    let db = plane.db.clone();
    let fingerprint_db = fingerprint.clone();
    let now = unix_now();
    let (inserted,existing)=db.call(move|conn| {
        let inserted=conn.execute("INSERT OR IGNORE INTO account_token_exchanges(fingerprint,state,created_at) VALUES(?1,'pending',?2)",params![fingerprint_db,now])?;
        let row=conn.query_row("SELECT state,response,created_at FROM account_token_exchanges WHERE fingerprint=?1",[fingerprint_db],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<Vec<u8>>>(1)?,r.get::<_,i64>(2)?)))?;
        Ok((inserted==1,row))
    }).await?;
    if !inserted {
        let (status, bytes, created_at) = existing;
        if status == "complete" {
            let bytes = bytes.ok_or_else(|| ApiError::internal("Token replay is incomplete"))?;
            let plain = state.0.sealer.open(&fingerprint, &bytes)?;
            let mut cached: CachedTokens = serde_json::from_slice(&plain)
                .map_err(|_| ApiError::internal("Token replay is unreadable"))?;
            cached.tokens.expires_in = cached
                .tokens
                .expires_in
                .saturating_sub(u64::try_from(now - cached.issued_at).unwrap_or(0));
            return Ok(cached.tokens);
        }
        if status == "pending" && now - created_at < 90 {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                ErrorCode::IdempotencyInProgress,
                "This sign-in or refresh is still running",
            )
            .with_retry_after(2));
        }
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::IdempotencyResponseExpired,
            "The token exchange result could not be recovered safely",
        )
        .with_hint("Sign in again; the previous refresh token will not be reused"));
    }
    // This task survives client disconnects; the token result reaches durable storage.
    let result = exchange.await;
    match result {
        Ok(tokens) => {
            let expires_at = tokens
                .refresh_token_expires_at
                .map(|t| t.unix_timestamp())
                .unwrap_or(now);
            let cached = CachedTokens {
                tokens,
                issued_at: now,
            };
            let bytes = serde_json::to_vec(&cached)
                .map_err(|_| ApiError::internal("Could not encode token response"))?;
            let sealed = state.0.sealer.seal(&fingerprint, &bytes)?;
            db.call(move|conn| { conn.execute("UPDATE account_token_exchanges SET state='complete',response=?2,completed_at=?3,expires_at=?4 WHERE fingerprint=?1",params![fingerprint,sealed,unix_now(),expires_at])?;Ok(()) }).await?;
            Ok(cached.tokens)
        }
        Err(error) => {
            // An explicit app-credential or rate-limit refusal happens before token
            // consumption. Only these refusals may release the rotation claim.
            let safe_retry = api_code(&error).is_some_and(|(code, status)| {
                status == 429 || matches!(code, "invalid_client" | "unauthorized_client")
            });
            db.call(move |conn| {
                let sql = if safe_retry {
                    "DELETE FROM account_token_exchanges WHERE fingerprint=?1"
                } else {
                    "UPDATE account_token_exchanges SET state='uncertain' WHERE fingerprint=?1"
                };
                conn.execute(sql, [fingerprint])?;
                Ok(())
            })
            .await?;
            Err(token_error(&error))
        }
    }
}

async fn session_response(
    state: &AppState,
    plane: &Plane,
    mut tokens: TokenResponse,
) -> ApiResult<SessionResponse> {
    // A client may recover a lost response after its access token expired. Walk
    // the durable rotation chain using only replacement tokens, never reusing
    // a refresh token that Accounts already consumed.
    while tokens.expires_in == 0 {
        if tokens
            .refresh_token_expires_at
            .is_none_or(|t| t.unix_timestamp() <= unix_now())
        {
            return Err(rejected());
        }
        let refresh = tokens
            .refresh_token
            .as_ref()
            .ok_or_else(|| ApiError::internal("Accounts omitted the refresh token"))?;
        let accounts = plane.accounts()?;
        let app = accounts.app();
        tokens = exchange_once(
            state,
            plane,
            "refresh",
            refresh.expose(),
            app.refresh(refresh.expose()),
        )
        .await?;
    }
    let account = tokens
        .account
        .as_ref()
        .ok_or_else(|| ApiError::internal("Accounts omitted the account"))?;
    let account_id = AccountId::parse(&account.uuid).map_err(ApiError::from_client)?;
    let actor_id = ActorId::parse(&account.id).map_err(ApiError::from_client)?;
    let bearer = Bearer {
        token: Secret::new(tokens.access_token.expose()),
    };
    let principal = auth::authenticate(plane, bearer, "peek").await?;
    if principal.account != account_id || principal.actor.actor_type() != actor_id.actor_type() {
        return Err(ApiError::unauthenticated(
            "Accounts returned mismatched account information",
        ));
    }
    let actor_id = principal.actor.clone();
    let refresh = tokens
        .refresh_token
        .ok_or_else(|| ApiError::internal("Accounts omitted the refresh token"))?;
    let expires = tokens
        .refresh_token_expires_at
        .ok_or_else(|| ApiError::internal("Accounts omitted the session expiry"))?;
    let response = SessionResponse {
        access_token: Secret::new(tokens.access_token.expose()),
        refresh_token: Secret::new(refresh.expose()),
        token_type: tokens.token_type,
        expires_in: tokens.expires_in,
        refresh_token_expires_at: Some(Timestamp::from_unix(expires.unix_timestamp())),
        scope: tokens.scope.unwrap_or_else(|| "profile".to_owned()),
        actor: Actor {
            actor_type: actor_id.actor_type(),
            public_id: actor_id,
        },
        account_id: account_id.clone(),
        account_ids: vec![account_id],
        membership_id: principal.membership_id,
        display_name: Some(account.display_name.clone()),
        reconsent_required: false,
        ting: None,
    };
    response.validate().map_err(ApiError::from_client)?;
    Ok(response)
}

pub(crate) async fn login(
    State(state): State<AppState>,
    plane: Plane,
    body: JsonBody<LoginRequest>,
) -> ApiResult<Json<SessionResponse>> {
    if !token_shape_ok(body.value.slt.expose(), "slt_") {
        return Err(ApiError::invalid_input(
            "Pass the short-lived token from `silicon-accounts login --app peek --json`, not a public account ID",
        ));
    }
    tokio::spawn(async move {
        let accounts = plane.accounts()?;
        let app = accounts.app();
        let tokens = exchange_once(
            &state,
            &plane,
            "slt",
            body.value.slt.expose(),
            app.exchange_slt(body.value.slt.expose()),
        )
        .await?;
        session_response(&state, &plane, tokens).await.map(Json)
    })
    .await
    .map_err(|_| ApiError::internal("Sign-in task interrupted"))?
}

pub(crate) async fn exchange(
    State(state): State<AppState>,
    plane: Plane,
    body: JsonBody<CodeExchange>,
) -> ApiResult<Json<SessionResponse>> {
    let request = body.value;
    let redirect = url::Url::parse(&request.redirect_uri)
        .map_err(|_| ApiError::invalid_input("Invalid redirect URI"))?;
    if !state
        .0
        .config
        .web_origins
        .contains(&redirect.origin().ascii_serialization())
        || redirect.query().is_some()
        || redirect.fragment().is_some()
        || !(43..=128).contains(&request.code_verifier.len())
        || !request
            .code_verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
    {
        return Err(ApiError::invalid_input(
            "The callback and PKCE verifier do not match Peek's sign-in configuration",
        ));
    }
    tokio::spawn(async move {
        let accounts = plane.accounts()?;
        let app = accounts.app();
        // Include the verifier and redirect in the cache key so possession of a code alone cannot replay tokens.
        let credential =
            serde_json::to_string(&(&request.code, &request.redirect_uri, &request.code_verifier))
                .map_err(|_| ApiError::internal("Could not bind callback credentials"))?;
        let tokens = exchange_once(
            &state,
            &plane,
            "code",
            &credential,
            app.exchange_code(
                &request.code,
                &request.redirect_uri,
                Some(&request.code_verifier),
            ),
        )
        .await?;
        session_response(&state, &plane, tokens).await.map(Json)
    })
    .await
    .map_err(|_| ApiError::internal("Sign-in task interrupted"))?
}

pub(crate) async fn refresh(
    State(state): State<AppState>,
    plane: Plane,
    body: JsonBody<RefreshRequest>,
) -> ApiResult<Json<SessionResponse>> {
    if !token_shape_ok(body.value.refresh_token.expose(), "sar_") {
        return Err(rejected());
    }
    tokio::spawn(async move {
        let accounts = plane.accounts()?;
        let app = accounts.app();
        let tokens = exchange_once(
            &state,
            &plane,
            "refresh",
            body.value.refresh_token.expose(),
            app.refresh(body.value.refresh_token.expose()),
        )
        .await?;
        session_response(&state, &plane, tokens).await.map(Json)
    })
    .await
    .map_err(|_| ApiError::internal("Refresh task interrupted"))?
}

pub(crate) async fn logout(
    State(state): State<AppState>,
    plane: Plane,
    headers: HeaderMap,
    body: JsonBody<LogoutRequest>,
) -> ApiResult<StatusCode> {
    let token = body.value.token.expose();
    if !token_shape_ok(token, "sar_") && !token_shape_ok(token, "jwt") {
        return Err(ApiError::invalid_input("A session token is required"));
    }
    let accounts = plane.accounts()?;
    if body.value.revoke_ting
        && let Ok(bearer) = Bearer::required(&headers)
        && let Ok(principal) =
            auth::authenticate(&plane, bearer, &state.0.config.accounts.app_id).await
    {
        let (ctx, account, actor) = (
            plane.ctx_string(),
            principal.account.to_string(),
            principal.actor.to_string(),
        );
        let lookup = (ctx.clone(), account.clone(), actor.clone());
        if let Some(enrollment) = plane
            .db
            .call(move |c| Ok(store::enrollments::get(c, &lookup.0, &lookup.1, &lookup.2)?))
            .await?
        {
            crate::ting::revoke(&state, &plane, &principal, &enrollment.subscription_id).await?;
            plane
                .db
                .call(move |c| {
                    Ok(store::enrollments::mark_revoked(
                        c,
                        &ctx,
                        &account,
                        &actor,
                        unix_now(),
                    )?)
                })
                .await?;
        }
    }
    accounts
        .app()
        .revoke(token)
        .await
        .map_err(|e| upstream(&e, "sign out this session", false))?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn me(
    State(state): State<AppState>,
    plane: Plane,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let principal = auth::authenticate(
        &plane,
        Bearer::required(&headers)?,
        &state.0.config.accounts.app_id,
    )
    .await?;
    let accounts = plane.accounts()?;
    let name = tokio::time::timeout(
        Duration::from_secs(3),
        accounts.app().userinfo(principal.access_token.expose()),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .map(|u| u.account.display_name);
    let (ctx, account, actor) = (
        plane.ctx_string(),
        principal.account.to_string(),
        principal.actor.to_string(),
    );
    let enrollment = plane
        .db
        .call(move |c| Ok(store::enrollments::get(c, &ctx, &account, &actor)?))
        .await?;
    let ting = match enrollment {
        Some(e) if e.active() => TingEnrollment {
            subscribed: true,
            subscription_id: Some(e.subscription_id),
            error: None,
        },
        _ => TingEnrollment {
            subscribed: false,
            subscription_id: None,
            error: None,
        },
    };
    Ok(Json(Me {
        authenticated: true,
        actor: principal.actor_object(),
        display_name: name,
        account_id: principal.account.clone(),
        membership_id: principal.membership_id.clone(),
        scopes: principal.scope_list(),
        reconsent_required: principal.reconsent_required(),
        ting,
    })
    .into_response())
}
