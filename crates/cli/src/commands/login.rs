//! `peek login`, `peek login status`, `peek logout` (BLUEPRINT §2.4–§2.6,
//! the Stemcell contract) and the hidden `peek __after-login`.

use std::fmt::Write as _;
use std::time::Duration;

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result, Secret,
    api::{Me, TingEnrollment},
    identity::{ApiUrl, Context, OrgId, SlotKey},
    ipc::{
        AuthBlock,
        cli::{Attach, Detach, DetachReason, StatusOp},
    },
    runtime::{
        SessionSlot, Store, auth_block,
        daemon::REQUEST_TIMEOUT,
        force_refresh,
        login::{self, RemoteRevocation, default_login_delays},
        session::{RELOGIN_HINT, Rejection, SlotTing},
    },
    timestamp::{Timestamp, unix_now},
};

use super::{fresh, is_unauthenticated, next, sweep_revocations};
use crate::{
    cli::{AfterLoginArgs, LoginArgs, LoginCommand, LogoutArgs},
    context::{Globals, Session, cli_refresh_policy},
    input,
    output::Out,
    service, telemetry,
};

/// peekd's view of this home, for the `daemon` block.
#[derive(Clone, Copy, Debug, Default)]
struct DaemonInfo {
    attached: bool,
    queued_answers: u32,
    authority_required: u32,
}

impl DaemonInfo {
    fn value(self) -> Value {
        json!({"attached": self.attached, "queued_answers": self.queued_answers,
               "authority_required": self.authority_required})
    }
}

/// Asks a running peekd (never starts it) for this home's delivery counts.
async fn daemon_info(auth: &AuthBlock) -> DaemonInfo {
    let task = async {
        let mut s = service::connect_existing(Duration::from_millis(500))
            .await
            .ok()??;
        let (r, _) = s
            .call(&StatusOp {}, Some(auth), Vec::new(), Duration::from_secs(2))
            .await
            .ok()?;
        Some(DaemonInfo {
            attached: true,
            queued_answers: r.deliveries.pending,
            authority_required: r.deliveries.authority_required,
        })
    };
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Attaches this home to a running peekd (never starts it).
async fn attach_now(auth: &AuthBlock) -> bool {
    let task = async {
        let mut s = service::connect_existing(Duration::from_millis(500))
            .await
            .ok()??;
        s.call(&Attach {}, Some(auth), Vec::new(), Duration::from_secs(2))
            .await
            .ok()
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(3), task).await,
        Ok(Some(_))
    )
}

fn ting_value(ting: Option<&SlotTing>) -> Value {
    match ting {
        Some(t) => {
            let mut v = json!({"subscribed": t.subscribed, "subscription_id": t.subscription_id});
            if let Some(e) = &t.error {
                v["error"] = json!({"code": e.code, "message": e.message});
            }
            v
        }
        None => json!({"subscribed": false, "subscription_id": null}),
    }
}

fn relogin_next() -> &'static str {
    RELOGIN_HINT.trim_start_matches("log in again: ")
}

/// The authenticated `login status` shape (BLUEPRINT §2.6).
fn authenticated(session: &Session, slot: &SessionSlot, daemon: DaemonInfo) -> Value {
    let org_ids = if slot.org_ids.is_empty() {
        vec![slot.org_id.clone()]
    } else {
        slot.org_ids.clone()
    };
    let subscribed = slot.ting.as_ref().is_some_and(|t| t.subscribed);
    let mut v = json!({
        "authenticated": true,
        "id": slot.actor.public_id,
        "actor": slot.actor,
        "org_id": slot.org_id,
        "org_ids": org_ids,
        "membership_id": slot.membership_id,
        "authority": slot.actor.actor_type.as_str(),
        "custody": "client",
        "scopes": slot.scopes(),
        "reconsent_required": slot.reconsent_required,
        "access_expires_at": Timestamp::from_unix(slot.access_expires_at),
        "logged_in_at": Timestamp::from_unix(slot.logged_in_at),
        "family_expires_at_estimate": slot.family_expires_at_estimate(),
        "refresh_pending": slot.pending_refresh_key.is_some(),
        "ting": ting_value(slot.ting.as_ref()),
        "daemon": daemon.value(),
        "testing_environment_id": session.context.testing_id().map(|u| u.hyphenated().to_string()),
        "api_url": session.api,
        "store": session.store.dir().display().to_string(),
        "validated": true,
        "display_name": slot.display_name,
    });
    if slot.reconsent_required {
        v["next"] = json!(relogin_next());
    } else if !subscribed {
        v["next"] = json!("peek ting enroll");
    }
    v
}

fn signed_out(session_api: &ApiUrl, context: Context, reason: &str) -> Value {
    json!({
        "authenticated": false,
        "id": null,
        "reason": reason,
        "api_url": session_api,
        "testing_environment_id": context.testing_id().map(|u| u.hyphenated().to_string()),
    })
}

fn rejected(session: &Session, r: &Rejection) -> Value {
    let mut v = signed_out(&session.api, session.context, "rejected");
    v["rejection"] =
        json!({"code": r.code, "at": Timestamp::from_unix(r.at), "request_id": r.request_id});
    v["next"] = json!(relogin_next());
    v
}

fn human_status(v: &Value) -> String {
    if v["authenticated"] != true {
        let reason = v["reason"].as_str().unwrap_or("no_session");
        let mut s = match reason {
            "logged_out" => "not authenticated: this home logged out of peek".to_owned(),
            "rejected" => format!(
                "not authenticated: IAM rejected this home's session ({})",
                v["rejection"]["code"].as_str().unwrap_or("unknown")
            ),
            _ => "not authenticated: no peek session in this home".to_owned(),
        };
        let _ = write!(s, "\napi: {}", v["api_url"].as_str().unwrap_or_default());
        return s;
    }
    let daemon = &v["daemon"];
    let ting = &v["ting"];
    let mut s = format!(
        "authenticated as {} ({}) in org {}{}\n\
         membership: {}\n\
         scopes: {}\n\
         access expires: {}   logged in: {}   family expires (estimate): {}\n\
         ting: {}\n\
         peekd: {}\n\
         api: {}\n\
         store: {}",
        v["id"].as_str().unwrap_or_default(),
        v["authority"].as_str().unwrap_or_default(),
        v["org_id"].as_str().unwrap_or_default(),
        v["display_name"]
            .as_str()
            .map(|n| format!(", shown as \"{n}\""))
            .unwrap_or_default(),
        v["membership_id"].as_str().unwrap_or_default(),
        v["scopes"]
            .as_array()
            .map(|a| a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "))
            .unwrap_or_default(),
        v["access_expires_at"].as_str().unwrap_or_default(),
        v["logged_in_at"].as_str().unwrap_or_default(),
        v["family_expires_at_estimate"].as_str().unwrap_or_default(),
        if ting["subscribed"] == true {
            format!(
                "subscribed ({})",
                ting["subscription_id"].as_str().unwrap_or("?")
            )
        } else {
            "not subscribed".to_owned()
        },
        if daemon["attached"] == true {
            format!(
                "attached, {} queued answer(s), {} waiting for authority",
                daemon["queued_answers"], daemon["authority_required"]
            )
        } else {
            "not attached (peekd is not running, or this is not a Mac)".to_owned()
        },
        v["api_url"].as_str().unwrap_or_default(),
        v["store"].as_str().unwrap_or_default(),
    );
    if let Some(t) = v["testing_environment_id"].as_str() {
        let _ = write!(s, "\ntesting environment: {t}");
    }
    s
}

/// Maps a transport/5xx/IAM failure to the §2.6 exit-5 error.
fn unavailable(e: Error) -> Error {
    let iam = matches!(
        e.code(),
        ErrorCode::IamUnavailable | ErrorCode::IamMisconfigured
    );
    let transient = iam
        || e.is_transport()
        || e.status().is_some_and(|s| s >= 500 || s == 429)
        || matches!(
            e.code(),
            ErrorCode::BackendUnavailable
                | ErrorCode::RateLimited
                | ErrorCode::IdempotencyInProgress
        );
    if !transient {
        return e;
    }
    let code = if iam {
        ErrorCode::IamUnavailable
    } else {
        ErrorCode::BackendUnavailable
    };
    let hint = e.hint().map_or_else(
        || "retry; the session itself is unchanged".to_owned(),
        str::to_owned,
    );
    Error::new(code, e.message().to_owned())
        .with_hint(hint)
        .with_retryable(true)
        .with_request_id(e.request_id().map(str::to_owned))
        .with_retry_after(e.retry_after())
        .with_origin(e.origin())
}

pub async fn run(g: &Globals, out: Out, args: LoginArgs) -> Result<()> {
    if let Some(LoginCommand::Status) = args.command {
        return status(g, out).await;
    }
    g.check_stdin(&[("--token-file", args.token_file.as_deref() == Some("-"))])?;
    let slt = if args.recover {
        None
    } else if let Some(s) = args.slt {
        Some(Secret::new(s))
    } else if let Some(f) = &args.token_file {
        Some(input::read_secret("--token-file", f)?)
    } else {
        return Err(Error::invalid_input(
            "peek login needs an SLT: pass it as an argument, with --token-file <PATH|->, or use --recover",
        )
        .with_hint("mint one: iam silicon-login --app-id peek --grant-org <org> --approve-scopes; then peek login '<SLT>'")
        .with_details(json!({"missing_argument": "SLT"})));
    };
    let session = g.session(crate::context::store()?, true).await?;
    let org_hint = g.org(None)?;
    let delays = default_login_delays();
    let outcome = match &slt {
        Some(slt) => {
            login::login(
                &session.store,
                &session.client,
                session.context,
                slt,
                org_hint.as_ref(),
                &delays,
            )
            .await?
        }
        None => {
            login::recover_login(&session.store, &session.client, session.context, &delays).await?
        }
    };
    telemetry::note_actor(&outcome.slot.org_id, outcome.slot.actor_id());
    let auth = auth_block(&session.store, &outcome.slot_key)?;
    telemetry::note_auth(&auth);
    let attached = attach_now(&auth).await;
    let daemon = DaemonInfo {
        attached,
        ..DaemonInfo::default()
    };
    let value = authenticated(&session, &outcome.slot, daemon);
    out.value(&value, human_status);
    if let Some(TingEnrollment {
        subscribed: false,
        error,
        ..
    }) = &outcome.ting
    {
        out.hint(format!(
            "Ting enrollment did not complete{}; answers cannot reach you until `peek ting enroll` succeeds",
            error
                .as_ref()
                .map(|e| format!(" ({}: {})", e.code, e.message))
                .unwrap_or_default()
        ));
    }
    if outcome.slot.reconsent_required {
        out.hint(format!(
            "this session lacks scopes peek needs; {RELOGIN_HINT} with --approve-scopes"
        ));
    }
    next(
        out,
        &[
            "peek register side <1-8>",
            "peek register drawing ./logo.js",
            "peek send --speak \"…\"",
        ],
    );
    sweep_revocations(&session.store, session.telemetry).await;
    if cfg!(target_os = "macos") {
        // Best effort, detached, after the result is printed: its outcome
        // never changes login's output or exit code.
        let _ = service::spawn_after_login(session.store.dir(), &session.api, session.context);
    }
    Ok(())
}

/// The outcome of the live check behind `login status`.
enum Verified {
    /// The backend confirmed the (possibly refreshed) session; the org it
    /// was asked for (`--org` / `SILICON_ORG`) when the session does not
    /// cover that org.
    Live(Box<(SessionSlot, Me, Option<OrgId>)>),
    /// IAM rejected it; the slot is marked.
    Rejected,
}

/// The org `login status` verifies with. Status is not an org-specific
/// command: it must always answer in the §2.6 shape (Stemcell sets
/// `SILICON_ORG` on every call and reads status before it re-grants or
/// removes peek). So a requested org the session does not cover never fails
/// the check: the session's own org is verified instead and the requested
/// org is reported next to the result.
fn status_org(g: &Globals, slot: &SessionSlot) -> (OrgId, Option<OrgId>) {
    match g.session_org(slot) {
        Ok(org) => (org, None),
        Err(_) => (slot.org_id.clone(), g.org(None).ok().flatten()),
    }
}

/// `fresh_session(60 s)`, then `GET /api/v1/auth/me`; a 401 forces one
/// refresh and one retry, and a second 401 marks the slot rejected (§2.6).
async fn verify(g: &Globals, session: &Session) -> Result<Verified> {
    let slot = match fresh(session).await {
        Ok(s) => s,
        Err(e) if *e.code() == ErrorCode::SessionRejected => return Ok(Verified::Rejected),
        Err(e) => return Err(unavailable(e)),
    };
    let (org, requested) = status_org(g, &slot);
    let first = session
        .client
        .with_session(slot.access_token.clone(), org.clone())
        .me()
        .await;
    match first {
        Ok(me) => return Ok(Verified::Live(Box::new((slot, me, requested)))),
        Err(e) if is_unauthenticated(&e) => {}
        Err(e) => return Err(unavailable(e)),
    }
    let retried = match force_refresh(
        &session.store,
        &session.client,
        session.context,
        &slot.access_token,
        &cli_refresh_policy(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) if *e.code() == ErrorCode::SessionRejected => return Ok(Verified::Rejected),
        Err(e) => return Err(unavailable(e)),
    };
    match session
        .client
        .with_session(retried.access_token.clone(), org)
        .me()
        .await
    {
        Ok(me) => Ok(Verified::Live(Box::new((retried, me, requested)))),
        Err(e) if is_unauthenticated(&e) => {
            mark_rejected(session, &retried, &e).await?;
            Ok(Verified::Rejected)
        }
        Err(e) => Err(unavailable(e)),
    }
}

/// Stores what the backend just verified (unless the session changed).
async fn record_verification(
    session: &Session,
    slot: &SessionSlot,
    me: &Me,
) -> Result<SessionSlot> {
    let key = session.slot_key.as_string();
    let now = unix_now();
    let updated = session
        .store
        .update_session_async(|f| {
            let current = f
                .slots
                .get_mut(&key)
                .filter(|s| s.refresh_token == slot.refresh_token);
            Ok(current.map(|s| {
                s.verified_at = Some(now);
                if me.display_name.is_some() {
                    s.display_name.clone_from(&me.display_name);
                }
                s.reconsent_required = me.reconsent_required || !s.has_required_scopes();
                let registered_at = s.ting.as_ref().and_then(|t| t.registered_at);
                s.ting = Some(SlotTing {
                    subscribed: me.ting.subscribed,
                    subscription_id: me.ting.subscription_id.clone(),
                    registered_at: if me.ting.subscribed {
                        registered_at.or(Some(now))
                    } else {
                        None
                    },
                    error: me.ting.error.clone(),
                });
                s.clone()
            }))
        })
        .await?;
    Ok(updated.unwrap_or_else(|| slot.clone()))
}

pub async fn status(g: &Globals, out: Out) -> Result<()> {
    let Some(store) = crate::context::existing_store()? else {
        if g.is_testing()? {
            // A testing environment must have been saved in this home.
            let _ = g.session(crate::context::store()?, false).await?;
        }
        let api = g.explicit_api()?.unwrap_or_else(ApiUrl::production);
        out.value(
            &signed_out(&api, Context::Production, "no_session"),
            human_status,
        );
        return Ok(());
    };
    let mut session = g.session(store, false).await?;
    let file = session.store.read_session()?;
    let key = session.slot_key.as_string();
    match file.slot(&session.slot_key) {
        None => {
            let logged_out = file
                .logged_out
                .as_ref()
                .is_some_and(|l| l.slot.as_deref().is_none_or(|s| s == key));
            let reason = if logged_out {
                "logged_out"
            } else {
                "no_session"
            };
            out.value(
                &signed_out(&session.api, session.context, reason),
                human_status,
            );
            return Ok(());
        }
        Some(slot) => {
            if let Some(r) = &slot.rejected {
                out.value(&rejected(&session, r), human_status);
                next(out, &[relogin_next()]);
                return Ok(());
            }
            telemetry::note_actor(&slot.org_id, slot.actor_id());
        }
    }
    if session.testing.is_some() {
        // Re-read the environment's generation (it changes when cleaned).
        session = g.session(session.store.clone(), true).await?;
    }
    let Verified::Live(live) = verify(g, &session).await? else {
        return print_rejected(&session, out);
    };
    let (slot, me, requested) = *live;
    let updated = record_verification(&session, &slot, &me).await?;
    let auth = auth_block(&session.store, &session.slot_key).ok();
    let daemon = match &auth {
        Some(a) => {
            telemetry::note_auth(a);
            daemon_info(a).await
        }
        None => DaemonInfo::default(),
    };
    let mut value = authenticated(&session, &updated, daemon);
    if let Some(requested) = &requested {
        value["requested_org"] = json!(requested);
        value["requested_org_authorized"] = json!(false);
    }
    out.value(&value, human_status);
    if let Some(requested) = requested {
        out.hint(format!(
            "this session is authorized for org(s) {}, not `{requested}` (--org / SILICON_ORG); log in for it: iam silicon-login --app-id peek --grant-org {requested} --approve-scopes; peek --org {requested} login '<SLT>'",
            value["org_ids"]
                .as_array()
                .map(|a| a
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ));
    }
    sweep_revocations(&session.store, session.telemetry).await;
    Ok(())
}

async fn mark_rejected(session: &Session, slot: &SessionSlot, e: &Error) -> Result<()> {
    let key = session.slot_key.as_string();
    let code = e.code().as_str().to_owned();
    let request_id = e.request_id().map(str::to_owned);
    session
        .store
        .update_session_async(|f| {
            if let Some(s) = f.slots.get_mut(&key)
                && s.refresh_token == slot.refresh_token
            {
                s.rejected = Some(Rejection {
                    code,
                    at: unix_now(),
                    request_id,
                });
            }
            Ok(())
        })
        .await
}

fn print_rejected(session: &Session, out: Out) -> Result<()> {
    let file = session.store.read_session()?;
    let rejection = file
        .slot(&session.slot_key)
        .and_then(|s| s.rejected.clone())
        .unwrap_or(Rejection {
            code: "session_rejected".to_owned(),
            at: unix_now(),
            request_id: None,
        });
    out.value(&rejected(session, &rejection), human_status);
    next(out, &[relogin_next()]);
    Ok(())
}

pub async fn logout(g: &Globals, args: &LogoutArgs, out: Out) -> Result<()> {
    let human = |v: &Value| match v["remote_revocation"].as_str() {
        Some("pending") => {
            "logged out locally; the backend revocation is pending and will be retried".to_owned()
        }
        _ => "logged out of peek".to_owned(),
    };
    let Some(store) = crate::context::existing_store()? else {
        out.value(
            &json!({"authenticated": false, "remote_revocation": "confirmed"}),
            human,
        );
        return Ok(());
    };
    let session = g.session(store, false).await?;
    // Detach first: peekd authenticates the home from its session slot, so
    // it must still exist when peekd cancels this actor's undelivered rows.
    let file = session.store.read_session()?;
    if let Ok(slot) = file.usable_slot(&session.slot_key, session.store.dir()) {
        telemetry::note_actor(&slot.org_id, slot.actor_id());
        if let Ok(auth) = auth_block(&session.store, &session.slot_key) {
            detach_now(&auth).await;
        }
    }
    let outcome = login::logout(
        &session.store,
        &session.client,
        session.context,
        args.revoke_ting,
    )
    .await?;
    let value = json!({"authenticated": false, "remote_revocation": outcome.remote_revocation});
    out.value(&value, human);
    if args.revoke_ting && outcome.remote_revocation == RemoteRevocation::Pending {
        // A queued retry never carries the bearer, so it cannot revoke the grant.
        out.hint("the Ting grant was not revoked: the queued retry cannot carry it. Log in again and run `peek logout --revoke-ting` if you still want it gone");
    } else if !args.revoke_ting && outcome.actor.is_some() {
        out.hint("the Ting grant stays for other homes of this Silicon; `peek logout --revoke-ting` removes it too");
    }
    if outcome.remote_revocation == RemoteRevocation::Pending {
        out.hint(format!(
            "the backend could not confirm the revocation ({}); it stays queued and the next peek run retries it",
            outcome.error.as_deref().unwrap_or("unknown error")
        ));
    } else if outcome.actor.is_none() {
        out.hint("there was no peek session in this home; nothing needed revoking");
    }
    sweep_revocations(&session.store, session.telemetry).await;
    Ok(())
}

async fn detach_now(auth: &AuthBlock) {
    let task = async {
        let mut s = service::connect_existing(Duration::from_millis(500))
            .await
            .ok()??;
        s.call(
            &Detach {
                reason: DetachReason::Logout,
            },
            Some(auth),
            Vec::new(),
            Duration::from_secs(3),
        )
        .await
        .ok()
    };
    let _ = tokio::time::timeout(Duration::from_secs(4), task).await;
}

/// `peek __after-login`: installs and starts Peek.app, then attaches the
/// home. Runs detached with its output in the Peek support log.
pub async fn after_login(args: AfterLoginArgs) -> Result<()> {
    let store = Store::open_existing(&args.store)?;
    let key = SlotKey::new(
        ApiUrl::parse(&args.api_url)?,
        Context::parse(&args.context)?,
    );
    let auth = auth_block(&store, &key)?;
    let mut s = service::ensure_service().await?;
    s.call(&Attach {}, Some(&auth), Vec::new(), REQUEST_TIMEOUT)
        .await?;
    Ok(())
}
