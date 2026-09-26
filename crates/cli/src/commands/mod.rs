//! Command dispatch and the helpers commands share.

mod app;
mod ask;
pub(crate) mod config;
mod doctor;
mod iam;
mod login;
mod org;
mod register;
mod report;
mod send;
mod status;
mod ting;
mod update;

use std::future::Future;

use silicon_peek_client::{
    Error, ErrorCode, Result,
    error::Origin,
    http::Client,
    identity::{Context, OrgId, SlotKey},
    ipc::AuthBlock,
    runtime::{
        CLI_MARGIN, SessionSlot, Store, auth_block, force_refresh, fresh_session_with,
        login::revoke_pending,
    },
};

use crate::{
    cli::{Cli, Command, TingCommand},
    context::{Globals, Session, cli_refresh_policy},
    docs, experience,
    output::Out,
    telemetry,
};

/// Runs the parsed command.
pub async fn run(cli: Cli, path: &[String], g: &Globals) -> Result<()> {
    let out = g.out();
    let command_name = path.join(" ");
    if !matches!(
        cli.command,
        Command::Ting {
            command: TingCommand::Enroll
        } | Command::Report(_)
    ) {
        g.refuse_idempotency_key(&command_name)?;
    }
    match cli.command {
        Command::Iam => iam::run(g, out).await,
        Command::Login(args) => login::run(g, out, args).await,
        Command::Logout => login::logout(g, out).await,
        Command::Config { command } => config::run(g, out, command).await,
        Command::Ting {
            command: TingCommand::Enroll,
        } => ting::enroll(g, out).await,
        Command::Register { command } => register::run(g, out, command).await,
        Command::Unregister => register::unregister(g, out).await,
        Command::Send(args) => send::run(g, out, *args).await,
        Command::Ask { command } => ask::run(g, out, command).await,
        Command::History(args) => ask::history(g, out, args).await,
        Command::Status => status::run(g, out).await,
        Command::Org { command } => org::run(g, out, command).await,
        Command::App { command } => app::run(g, out, command).await,
        Command::Daemon { command } => app::daemon(g, out, command).await,
        Command::Docs(args) => {
            let value = if let Some(q) = &args.search {
                docs::search(q)?
            } else if args.all {
                docs::all()
            } else if let Some(t) = &args.topic {
                docs::topic(t)?
            } else {
                docs::index()
            };
            out.value(&value, docs::human);
            Ok(())
        }
        Command::Commands => {
            let value = experience::commands_value();
            out.value(&value, experience::commands_text);
            Ok(())
        }
        Command::Report(args) => report::run(g, out, args).await,
        Command::Update => update::run(g, out).await,
        Command::Doctor => doctor::run(g, out).await,
        Command::AfterLogin(args) => login::after_login(args).await,
    }
}

/// A session whose slot is usable, plus the peekd auth block of this home.
/// peekd derives the identity from the store; the CLI only checks locally
/// that there is something to derive it from, so the error is precise.
pub async fn mac_session(g: &Globals) -> Result<(Session, AuthBlock)> {
    let session = g.session(crate::context::store()?, false).await?;
    let file = session.store.read_session()?;
    let slot = file.usable_slot(&session.slot_key, session.store.dir())?;
    telemetry::note_actor(&slot.org_id, slot.actor_id());
    let auth = auth_block(&session.store, &session.slot_key)?;
    telemetry::note_auth(&auth);
    Ok((session, auth))
}

/// A fresh session slot (refreshed if it expires within 60 s).
pub async fn fresh(s: &Session) -> Result<SessionSlot> {
    fresh_session_with(
        &s.store,
        &s.client,
        s.context,
        CLI_MARGIN,
        &cli_refresh_policy(),
    )
    .await
}

/// Whether peek-server refused the bearer token (401).
pub fn is_unauthenticated(e: &Error) -> bool {
    e.status() == Some(401) && e.origin() == Origin::Server
}

/// Runs a bearer call; on a 401 it forces one refresh and retries once
/// (access tokens can be invalidated early). In a testing environment, a
/// `409 testing_generation_changed` re-reads the generation from
/// `GET /api/v1/iam` (saving it to testing.json) and retries once.
pub async fn bearer<T, F, Fut>(g: &Globals, s: &Session, f: F) -> Result<T>
where
    F: Fn(Client, OrgId) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    match bearer_once(g, s, &f).await {
        Err(e) if *e.code() == ErrorCode::TestingGenerationChanged && s.testing.is_some() => {
            let refreshed = crate::context::refresh_generation(s).await?;
            bearer_once(g, &refreshed, &f).await
        }
        r => r,
    }
}

async fn bearer_once<T, F, Fut>(g: &Globals, s: &Session, f: &F) -> Result<T>
where
    F: Fn(Client, OrgId) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let slot = fresh(s).await?;
    let org = g.session_org(&slot)?;
    telemetry::note_actor(&org, slot.actor_id());
    match f(
        s.client
            .with_session(slot.access_token.clone(), org.clone()),
        org.clone(),
    )
    .await
    {
        Err(e) if is_unauthenticated(&e) => {
            let slot = force_refresh(
                &s.store,
                &s.client,
                s.context,
                &slot.access_token,
                &cli_refresh_policy(),
            )
            .await?;
            f(
                s.client
                    .with_session(slot.access_token.clone(), org.clone()),
                org,
            )
            .await
        }
        r => r,
    }
}

/// Retries queued refresh-family revocations (best effort, bounded).
pub async fn sweep_revocations(store: &Store, telemetry_on: bool) {
    let testing = store.read_testing().ok();
    let client_for = move |key: &SlotKey| -> Option<Client> {
        let mut client = Client::builder(key.api_url())
            .component(concat!("peek-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(4))
            .build()
            .ok()?
            .with_telemetry(telemetry_on)
            .with_trace_id(telemetry::trace_id());
        if let Context::Testing(id) = key.context() {
            let env = testing.as_ref()?.environments.get(&id)?;
            client = client.with_testing(env.app_secret.clone(), Some(env.generation));
        }
        Some(client)
    };
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        revoke_pending(store, client_for),
    )
    .await;
}

/// The `Next:` hint block in human mode.
pub fn next(out: Out, lines: &[&str]) {
    if lines.is_empty() {
        return;
    }
    out.hint(format!("Next: {}", lines.join("  ·  ")));
}
