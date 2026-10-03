//! Dedicated Ting authorization. Ordinary login never conveys provider authority.
//! Encrypted root families and retry identities survive restart and logout. SQLite
//! leases serialize each account's exchanges across server processes; a timed-out
//! operation retains its exact payload/key for recovery after the lease expires.
use axum::http::StatusCode;
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use silicon_iam_client::{IdempotencyKey, Mutation, models};
use silicon_peek_client::{
    ErrorCode,
    identity::{ActorId, OrgId},
    timestamp::unix_now,
};
use uuid::Uuid;

use crate::{
    auth::Principal,
    db::Db,
    error::{ApiError, ApiResult},
    iam::{api_code, upstream},
    plane::Plane,
    state::AppState,
};

pub(crate) const ENDPOINTS: [&str; 3] = [
    "subscriptions.register",
    "subscriptions.revoke",
    "tings.send",
];

pub(crate) fn required() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, ErrorCode::ReconsentRequired, "Ting permission is required for this feature.")
        .with_hint("Run `peek ting authorize`, review the request in IAM, then run `peek ting complete-authorization --code-file -` with the displayed code. Your Peek login and pending action are preserved.")
        .with_details(json!({"feature":"ting","permission_required":true}))
}
fn invalid_authority() -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        ErrorCode::IamUnavailable,
        "IAM returned incomplete or mismatched Ting authority.",
    )
}
fn busy() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        ErrorCode::IdempotencyInProgress,
        "This account's Ting authorization is being updated. Retry the same action.",
    )
    .with_retry_after(2)
}
fn failure(error: &silicon_iam_client::Error, plane: &Plane) -> ApiError {
    match api_code(error) {
        Some((_, 412)) => ApiError::new(StatusCode::PRECONDITION_FAILED, ErrorCode::ReconsentRequired, "The Ting permission graph changed. Review a new request in IAM; your action is preserved.").with_details(json!({"feature":"ting","graph_changed":true})),
        Some((_, 400 | 401 | 403 | 404 | 409 | 410)) => required(),
        _ => upstream(error, "complete Ting permission", plane.is_testing()),
    }
}
fn mutation(key: &str) -> ApiResult<Mutation> {
    IdempotencyKey::parse(key)
        .map(Mutation::with_key)
        .map_err(|_| ApiError::invalid_input("Invalid feature retry key"))
}
fn serialize(value: &impl Serialize) -> ApiResult<Vec<u8>> {
    serde_json::to_vec(value)
        .map_err(|_| ApiError::internal("Could not serialize feature authority"))
}
fn deserialize<T: for<'a> Deserialize<'a>>(value: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(value)
        .map_err(|_| ApiError::internal("Saved feature authority is unreadable"))
}
fn generation(plane: &Plane) -> u64 {
    plane.testing.as_ref().map_or(0, |t| t.generation)
}
fn aad(plane: &Plane, principal: &Principal, kind: &str, id: &str) -> String {
    json!([
        "peek-obo-v1",
        plane.ctx_string(),
        generation(plane),
        principal.org,
        principal.actor,
        kind,
        id
    ])
    .to_string()
}

struct Lease {
    db: Db,
    ctx: String,
    org: String,
    actor: String,
    owner: String,
}
impl Lease {
    async fn acquire(plane: &Plane, principal: &Principal) -> ApiResult<Self> {
        let lease = Self {
            db: plane.db.clone(),
            ctx: plane.ctx_string(),
            org: principal.org.to_string(),
            actor: principal.actor.to_string(),
            owner: Uuid::new_v4().to_string(),
        };
        let (ctx, org, actor, owner) = (
            lease.ctx.clone(),
            lease.org.clone(),
            lease.actor.clone(),
            lease.owner.clone(),
        );
        let count = lease.db.call(move |c| Ok(c.execute("INSERT INTO obo_locks(ctx,org_id,actor_id,owner,expires_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(ctx,org_id,actor_id) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at WHERE obo_locks.expires_at < ?6",params![ctx,org,actor,owner,unix_now()+120,unix_now()])?)).await?;
        if count != 1 {
            return Err(busy());
        }
        Ok(lease)
    }
    async fn call<T, F>(&self, action: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> ApiResult<T> + Send + 'static,
    {
        let (ctx, org, actor, owner) = (
            self.ctx.clone(),
            self.org.clone(),
            self.actor.clone(),
            self.owner.clone(),
        );
        self.db.call(move |c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let held: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM obo_locks WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND owner=?4 AND expires_at>=?5)", params![ctx,org,actor,owner,unix_now()], |r| r.get(0))?;
            if !held { return Err(busy()); }
            let result = action(&tx)?;
            tx.commit()?;
            Ok(result)
        }).await
    }
    async fn release(self) {
        let _ = self
            .db
            .call(move |c| {
                c.execute(
                    "DELETE FROM obo_locks WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND owner=?4",
                    params![self.ctx, self.org, self.actor, self.owner],
                )?;
                Ok(())
            })
            .await;
    }
}

#[derive(Clone)]
struct Request {
    id: String,
    payload: Vec<u8>,
    authorization_id: Option<String>,
    consent: Option<String>,
    code_hash: Option<String>,
    completed: bool,
}
fn request(c: &Connection, ctx: &str, org: &str, actor: &str, id: &str) -> ApiResult<Request> {
    c.query_row("SELECT id,payload,authorization_id,consent,code_hash,completed FROM obo_requests WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND id=?4", params![ctx,org,actor,id], |r| Ok(Request{id:r.get(0)?,payload:r.get(1)?,authorization_id:r.get(2)?,consent:r.get(3)?,code_hash:r.get(4)?,completed:r.get(5)?})).optional()?.ok_or_else(|| ApiError::not_found("This Ting permission request is not available in this account and organization"))
}
fn check_consent(
    detail: &models::OboConsentDetail,
    principal: &Principal,
    app_id: &str,
) -> ApiResult<()> {
    if detail.id.is_nil()
        || detail.app_id != app_id
        || detail.org_id != principal.org.as_str()
        || detail.actor.public_id != principal.actor.as_str()
        || detail.redirect_uri.is_some()
        || detail.state.is_some()
        || serde_json::to_value(&detail.actor.type_field).ok()
            != serde_json::to_value(principal.actor.actor_type()).ok()
    {
        return Err(invalid_authority());
    }
    if let Some(url) = &detail.authorization_url {
        let parsed = url::Url::parse(url).map_err(|_| invalid_authority())?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(invalid_authority());
        }
    }
    Ok(())
}
fn safe(pair: &models::OboTokenPair) -> Value {
    json!({"audience":pair.audience,"endpoint_id":pair.endpoint_id,"grant_id":pair.grant_id,"actor":pair.actor,"org_id":pair.org_id,"expires_at":pair.expires_at.unix_timestamp()})
}
fn request_view(row: &Request, roots: &[Value]) -> ApiResult<Value> {
    let consent: Option<Value> = row
        .consent
        .as_ref()
        .map(|s| serde_json::from_str(s))
        .transpose()
        .map_err(|_| invalid_authority())?;
    Ok(json!({"request_id":row.id,"authorization":consent,"completed":row.completed,"roots":roots}))
}

pub(crate) async fn start(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    key: &str,
) -> ApiResult<Value> {
    let lease = Lease::acquire(plane, principal).await?;
    let result = async {
        let (ctx, org, actor, key) = (plane.ctx_string(), principal.org.to_string(), principal.actor.to_string(), key.to_owned());
        let id = Uuid::new_v4().to_string();
        let body = models::OboAuthorizationRequest { redirect_uri:None, state:None, subject_token:principal.access_token.expose().to_owned(), org_id:principal.org.to_string(), endpoints:ENDPOINTS.iter().map(|id| models::OboAuthorizationEndpoint{audience:"ting".into(),endpoint_id:(*id).into()}).collect() };
        let payload = state.0.sealer.seal(&aad(plane, principal, "request", &id), &serialize(&body)?)?;
        let gen_id = generation(plane);
        let row = lease.call(move |c| {
            c.execute("INSERT OR IGNORE INTO obo_requests(ctx,org_id,actor_id,id,request_key,generation,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![ctx,org,actor,id,key,gen_id,payload,unix_now()])?;
            let id:String=c.query_row("SELECT id FROM obo_requests WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND request_key=?4 AND generation=?5",params![ctx,org,actor,key,gen_id],|r|r.get(0))?;
            request(c,&ctx,&org,&actor,&id)
        }).await?;
        if row.authorization_id.is_some() { return request_view(&row, &[]); }
        let body: models::OboAuthorizationRequest = deserialize(&state.0.sealer.open(&aad(plane,principal,"request",&row.id),&row.payload)?)?;
        let detail = plane.iam()?.obo_authorize(&body,&mutation(&format!("peek-obo-start-{}",row.id))?).await.map_err(|e|failure(&e,plane))?;
        check_consent(&detail,principal,&state.0.config.iam.app_id)?;
        let (ctx,org,actor,id,auth,consent)=(plane.ctx_string(),principal.org.to_string(),principal.actor.to_string(),row.id.clone(),detail.id.to_string(),String::from_utf8(serialize(&detail)?).map_err(|_|invalid_authority())?);
        let row=lease.call(move|c|{c.execute("UPDATE obo_requests SET authorization_id=?5,consent=?6,payload=x'' WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND id=?4",params![ctx,org,actor,id,auth,consent])?;request(c,&ctx,&org,&actor,&id)}).await?;
        request_view(&row,&[])
    }.await;
    lease.release().await;
    result
}

pub(crate) async fn status(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    id: Uuid,
) -> ApiResult<Value> {
    let lease = Lease::acquire(plane, principal).await?;
    let result = async {
        let (ctx, org, actor, id) = (
            plane.ctx_string(),
            principal.org.to_string(),
            principal.actor.to_string(),
            id.to_string(),
        );
        let mut row = lease
            .call(move |c| request(c, &ctx, &org, &actor, &id))
            .await?;
        let auth = Uuid::parse_str(row.authorization_id.as_deref().ok_or_else(busy)?)
            .map_err(|_| invalid_authority())?;
        let detail = plane
            .iam()?
            .obo_authorization(auth)
            .await
            .map_err(|e| failure(&e, plane))?;
        check_consent(&detail, principal, &state.0.config.iam.app_id)?;
        if detail.id != auth {
            return Err(invalid_authority());
        }
        row.consent =
            Some(String::from_utf8(serialize(&detail)?).map_err(|_| invalid_authority())?);
        request_view(&row, &[])
    }
    .await;
    lease.release().await;
    result
}

#[derive(Serialize, Deserialize)]
struct Credentials {
    pair: models::OboTokenPair,
    refresh_key: Option<String>,
}

async fn validate_pair(plane: &Plane, pair: &models::OboTokenPair) -> ApiResult<()> {
    let actor = pair.actor.as_ref().ok_or_else(invalid_authority)?;
    let actor_id = ActorId::parse(&actor.public_id).map_err(|_| invalid_authority())?;
    if pair.audience != "ting"
        || !ENDPOINTS.contains(&pair.endpoint_id.as_str())
        || pair.grant_id.is_nil()
        || !pair.access_token.starts_with("oba_")
        || !pair.refresh_token.starts_with("obr_")
        || pair.token_type != models::OboTokenPairTokenType::Bearer
        || pair.expires_in <= 0
        || pair.expires_at.unix_timestamp() <= unix_now()
        || OrgId::parse(&pair.org_id).is_err()
        || serde_json::to_value(&actor.type_field).ok()
            != serde_json::to_value(actor_id.actor_type()).ok()
        || !pair
            .scope
            .split_whitespace()
            .any(|s| s == format!("obo:ting:{}", pair.endpoint_id))
    {
        return Err(invalid_authority());
    }
    match (&plane.testing, &pair.testing_context) {
        (None, None) => {}
        (Some(expected), Some(context)) if context.app_id == "ting" => {
            let selected = plane
                .iam()?
                .audience_testing_context("ting", &context.app_secret, &context.iam_test_key)
                .await
                .map_err(|e| failure(&e, plane))?;
            if selected.environment_id != expected.environment_id
                || selected.application.app_id != "ting"
            {
                return Err(invalid_authority());
            }
        }
        _ => return Err(invalid_authority()),
    }
    Ok(())
}

pub(crate) async fn complete(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    id: Uuid,
    code: &str,
) -> ApiResult<Value> {
    if code.is_empty() || code.len() > 1024 || !code.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ApiError::invalid_input(
            "Paste the authorization code shown by IAM",
        ));
    }
    let lease = Lease::acquire(plane, principal).await?;
    let result=async {
        let (ctx,org,actor,id)=(plane.ctx_string(),principal.org.to_string(),principal.actor.to_string(),id.to_string());
        let row=lease.call(move|c|request(c,&ctx,&org,&actor,&id)).await?;
        let hash=blake3::hash(code.as_bytes()).to_hex().to_string();
        if row.code_hash.as_ref().is_some_and(|h|h!=&hash) {return Err(ApiError::new(StatusCode::CONFLICT,ErrorCode::IdempotencyConflict,"Retry this permission request with the same code"));}
        if row.completed {return request_view(&row,&[]);}
        let auth=Uuid::parse_str(row.authorization_id.as_deref().ok_or_else(busy)?).map_err(|_|invalid_authority())?;
        let detail=plane.iam()?.obo_authorization(auth).await.map_err(|e|failure(&e,plane))?;
        check_consent(&detail,principal,&state.0.config.iam.app_id)?;
        if detail.id!=auth || !matches!(detail.status, models::OboConsentDetailStatus::Approved | models::OboConsentDetailStatus::Exchanged) {return Err(required());}
        let (ctx,org,actor,id)=(plane.ctx_string(),principal.org.to_string(),principal.actor.to_string(),row.id.clone());
        lease.call(move|c|{c.execute("UPDATE obo_requests SET code_hash=?5 WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND id=?4",params![ctx,org,actor,id,hash])?;Ok(())}).await?;
        let response=plane.iam()?.obo_code(auth,code,&mutation(&format!("peek-obo-code-{}",row.id))?).await.map_err(|e|failure(&e,plane))?;
        if response.items.len()!=ENDPOINTS.len() {return Err(invalid_authority());}
        let mut seen=std::collections::BTreeSet::new();
        let mut destination=None;
        let mut sealed=vec![];
        let mut roots=vec![];
        for pair in response.items {
            validate_pair(plane,&pair).await?;
            if !seen.insert(pair.endpoint_id.clone()) {return Err(invalid_authority());}
            let selected=(pair.org_id.clone(),pair.actor.as_ref().map(|a|a.public_id.clone()));
            if destination.as_ref().is_some_and(|d|d!=&selected) {return Err(invalid_authority());}
            destination=Some(selected);
            let endpoint=pair.endpoint_id.clone();roots.push(safe(&pair));
            let bytes=state.0.sealer.seal(&aad(plane,principal,"root",&endpoint),&serialize(&Credentials{pair,refresh_key:None})?)?;
            sealed.push((endpoint,bytes));
        }
        let consent=String::from_utf8(serialize(&detail)?).map_err(|_|invalid_authority())?;
        let (ctx,org,actor,id,gen_id)=(plane.ctx_string(),principal.org.to_string(),principal.actor.to_string(),row.id.clone(),generation(plane));
        let row=lease.call(move|c|{
            for (endpoint,bytes) in sealed {c.execute("INSERT INTO obo_roots(ctx,org_id,actor_id,endpoint,generation,request_id,credentials) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(ctx,org_id,actor_id,endpoint) DO UPDATE SET generation=excluded.generation,request_id=excluded.request_id,credentials=excluded.credentials",params![ctx,org,actor,endpoint,gen_id,id,bytes])?;}
            c.execute("UPDATE obo_requests SET completed=1,consent=?5 WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND id=?4",params![ctx,org,actor,id,consent])?;
            request(c,&ctx,&org,&actor,&id)
        }).await?;
        request_view(&row,&roots)
    }.await;
    lease.release().await;
    result
}

/// Read/refresh the dedicated root and pin a provider destination before I/O.
/// A later approval for another provider account cannot move an existing action.
pub(crate) async fn access(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    endpoint: &str,
    operation: &str,
    body: &[u8],
    force_refresh: bool,
) -> ApiResult<models::OboTokenPair> {
    let lease = Lease::acquire(plane, principal).await?;
    let result=async {
        let (ctx,org,actor,endpoint_owned,gen_id)=(plane.ctx_string(),principal.org.to_string(),principal.actor.to_string(),endpoint.to_owned(),generation(plane));
        let bytes=lease.call(move|c|Ok(c.query_row("SELECT credentials FROM obo_roots WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND endpoint=?4 AND generation=?5",params![ctx,org,actor,endpoint_owned,gen_id],|r|r.get::<_,Vec<u8>>(0)).optional()?)).await?.ok_or_else(required)?;
        let mut credentials:Credentials=deserialize(&state.0.sealer.open(&aad(plane,principal,"root",endpoint),&bytes)?)?;
        if force_refresh || credentials.pair.expires_at.unix_timestamp()<=unix_now()+60 || credentials.refresh_key.is_some() {
            let retry=credentials.refresh_key.get_or_insert_with(||format!("peek-obo-refresh-{}",Uuid::new_v4())).clone();
            save_root(state,plane,principal,&lease,endpoint,&credentials).await?;
            let mut response=plane.iam()?.obo_refresh(&credentials.pair.refresh_token,&mutation(&retry)?).await.map_err(|e|failure(&e,plane))?;
            if response.items.len()!=1 {return Err(invalid_authority());}
            let next=response.items.remove(0);
            validate_pair(plane,&next).await?;
            if next.grant_id!=credentials.pair.grant_id || next.endpoint_id!=endpoint || next.org_id!=credentials.pair.org_id || serde_json::to_value(&next.actor).ok()!=serde_json::to_value(&credentials.pair.actor).ok() {return Err(invalid_authority());}
            credentials=Credentials{pair:next,refresh_key:None};
            save_root(state,plane,principal,&lease,endpoint,&credentials).await?;
        }
        validate_pair(plane,&credentials.pair).await?;
        let (ctx,org,actor,endpoint,operation,hash,provider_org,provider_actor)=(plane.ctx_string(),principal.org.to_string(),principal.actor.to_string(),endpoint.to_owned(),operation.to_owned(),blake3::hash(body).to_hex().to_string(),credentials.pair.org_id.clone(),credentials.pair.actor.as_ref().ok_or_else(invalid_authority)?.public_id.clone());
        lease.call(move|c|{
            c.execute("INSERT OR IGNORE INTO obo_operations(ctx,org_id,actor_id,endpoint,operation_key,request_hash,provider_org,provider_actor) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![ctx,org,actor,endpoint,operation,hash,provider_org,provider_actor])?;
            let matches:bool=c.query_row("SELECT request_hash=?6 AND provider_org=?7 AND provider_actor=?8 FROM obo_operations WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND endpoint=?4 AND operation_key=?5",params![ctx,org,actor,endpoint,operation,hash,provider_org,provider_actor],|r|r.get(0))?;
            if !matches {return Err(ApiError::new(StatusCode::CONFLICT,ErrorCode::IdempotencyConflict,"This pending action belongs to its original Ting account, organization and payload. Restore that permission or create a new action."));}
            Ok(())
        }).await?;
        Ok(credentials.pair)
    }.await;
    lease.release().await;
    result
}
async fn save_root(
    state: &AppState,
    plane: &Plane,
    principal: &Principal,
    lease: &Lease,
    endpoint: &str,
    credentials: &Credentials,
) -> ApiResult<()> {
    let bytes = state.0.sealer.seal(
        &aad(plane, principal, "root", endpoint),
        &serialize(credentials)?,
    )?;
    let (ctx, org, actor, endpoint) = (
        plane.ctx_string(),
        principal.org.to_string(),
        principal.actor.to_string(),
        endpoint.to_owned(),
    );
    lease.call(move|c|{c.execute("UPDATE obo_roots SET credentials=?5 WHERE ctx=?1 AND org_id=?2 AND actor_id=?3 AND endpoint=?4",params![ctx,org,actor,endpoint,bytes])?;Ok(())}).await
}
