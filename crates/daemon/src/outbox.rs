//! The delivery outbox (BLUEPRINT §3.3–§3.6, gap-token-custody §4).
//!
//! Each row's `request` bytes are fixed when the event is recorded and are
//! sent unchanged on every attempt with `Idempotency-Key:
//! peek-delivery-<event_id>`. The queue key is `(api_url, context, org,
//! actor)`: rows are only ever sent with that Silicon's own session (any home
//! holding it), never another actor's. peekd never re-registers a Ting
//! recipient (D7): `recipient_not_registered` parks the row in
//! `authority_required` until `peek ting enroll` / a new login.

use std::{sync::Arc, time::Duration};

use rusqlite::{Connection, OptionalExtension as _, params};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    config::DEFAULT_DELIVERY_MAX_AGE_HOURS,
    http::Client,
    identity::{ActorId, ApiUrl, Context, OrgId},
    ids::EventId,
    runtime::{DELIVERY_MARGIN, RefreshPolicy, SessionSlot, Store, force_refresh},
};
use tokio::task::JoinSet;

use crate::{
    bubbles::now_ms,
    db::SqlResult as _,
    net::{HomeRef, needs_authority},
    state::{ActorKey, Shared, SharedRef},
    telemetry::Record,
};

/// What a row delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `POST /api/v1/deliveries`.
    Ting,
    /// `PUT /api/v1/drawings/current`.
    DrawingPut,
    /// `DELETE /api/v1/drawings/current`.
    DrawingDelete,
}

impl Kind {
    /// The stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ting => "ting",
            Self::DrawingPut => "drawing.put",
            Self::DrawingDelete => "drawing.delete",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "ting" => Some(Self::Ting),
            "drawing.put" => Some(Self::DrawingPut),
            "drawing.delete" => Some(Self::DrawingDelete),
            _ => None,
        }
    }
}

/// A row to insert.
#[derive(Clone, Debug)]
pub struct NewRow {
    /// `evt_…` (the delivery request's own id for tings).
    pub event_id: String,
    /// The Silicon.
    pub key: ActorKey,
    /// Its home and backend.
    pub home: HomeRef,
    /// What to do.
    pub kind: Kind,
    /// The exact body.
    pub request: Vec<u8>,
    /// The ask, send or message it reports on.
    pub subject_id: Option<String>,
}

/// Inserts a pending row, due now.
///
/// # Errors
/// Database failures.
pub fn insert(c: &Connection, row: &NewRow) -> Result<()> {
    let now = now_ms();
    c.execute(
        "INSERT INTO outbox (event_id, context, home_path, api_url, org_id, actor_id, kind, request,
                             created_at, next_attempt_at, attempts, status, subject_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 0, 'pending', ?10)",
        params![
            row.event_id,
            row.key.context_str(),
            row.home.home_path,
            row.home.api_url.as_str(),
            row.key.org.as_str(),
            row.key.actor.as_str(),
            row.kind.as_str(),
            row.request,
            now,
            row.subject_id
        ],
    )
    .sql()?;
    Ok(())
}

/// A row due for an attempt.
#[derive(Clone, Debug)]
struct DueRow {
    event_id: String,
    key: ActorKey,
    home: HomeRef,
    kind: Kind,
    request: Vec<u8>,
    created_at: i64,
    attempts: u32,
    subject_id: Option<String>,
}

/// The result of one attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Accepted {
        ting_id: Option<String>,
        silent: Option<bool>,
        request_id: Option<String>,
    },
    Retry {
        code: String,
        request_id: Option<String>,
        after: Option<Duration>,
    },
    TypeMissing {
        code: String,
        request_id: Option<String>,
        after: Option<Duration>,
    },
    AuthorityRequired {
        code: String,
        request_id: Option<String>,
    },
    Failed {
        code: String,
        request_id: Option<String>,
    },
    Cancelled {
        code: String,
    },
    Expired,
}

impl Outcome {
    fn status(&self) -> &'static str {
        match self {
            Self::Accepted { .. } => "accepted",
            Self::Retry { .. } | Self::TypeMissing { .. } => "pending",
            Self::AuthorityRequired { .. } => "authority_required",
            Self::Failed { .. } => "failed",
            Self::Cancelled { .. } => "cancelled",
            Self::Expired => "expired",
        }
    }

    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Accepted { .. } | Self::Failed { .. } | Self::Cancelled { .. } | Self::Expired
        )
    }

    fn code(&self) -> Option<&str> {
        match self {
            Self::Retry { code, .. }
            | Self::TypeMissing { code, .. }
            | Self::AuthorityRequired { code, .. }
            | Self::Failed { code, .. }
            | Self::Cancelled { code } => Some(code),
            Self::Accepted { .. } => None,
            Self::Expired => Some("expired"),
        }
    }
}

fn code_of(e: &Error) -> String {
    e.code().as_str().to_owned()
}

/// Classifies a failed delivery (§3.6 matrix).
fn classify(e: &Error) -> Outcome {
    let code = code_of(e);
    let request_id = e.request_id().map(str::to_owned);
    if e.is_transport() {
        return Outcome::Retry {
            code,
            request_id,
            after: None,
        };
    }
    match e.code() {
        ErrorCode::RecipientNotRegistered
        | ErrorCode::ReconsentRequired
        | ErrorCode::TestingGenerationChanged
        | ErrorCode::TestingSecretInvalid
        | ErrorCode::Unauthenticated
        | ErrorCode::SessionRejected
        | ErrorCode::NotLoggedIn
        | ErrorCode::PrivateApplicationOrganizationRequired
        | ErrorCode::EnvironmentNotPrepared => Outcome::AuthorityRequired { code, request_id },
        ErrorCode::TingTypeMissing => Outcome::TypeMissing {
            code,
            request_id,
            after: e.retry_after(),
        },
        ErrorCode::TingRejected
        | ErrorCode::TingKeyConflict
        | ErrorCode::InvalidInput
        | ErrorCode::InvalidJson
        | ErrorCode::PayloadTooLarge
        | ErrorCode::IdempotencyConflict
        | ErrorCode::IdempotencyKeyRequired
        | ErrorCode::DrawingTooLarge
        | ErrorCode::UnexpectedResponse => Outcome::Failed { code, request_id },
        ErrorCode::TingUnavailable
        | ErrorCode::BackendUnavailable
        | ErrorCode::IamUnavailable
        | ErrorCode::IamMisconfigured
        | ErrorCode::RateLimited
        | ErrorCode::IdempotencyInProgress => Outcome::Retry {
            code,
            request_id,
            after: e.retry_after(),
        },
        _ => match e.status() {
            Some(401 | 403) => Outcome::AuthorityRequired { code, request_id },
            Some(s) if (400..500).contains(&s) && s != 408 && s != 429 => {
                Outcome::Failed { code, request_id }
            }
            _ => Outcome::Retry {
                code,
                request_id,
                after: e.retry_after(),
            },
        },
    }
}

fn parse_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<DueRow>> {
    let event_id: String = r.get(0)?;
    let context: String = r.get(1)?;
    let home_path: String = r.get(2)?;
    let api_url: String = r.get(3)?;
    let org: String = r.get(4)?;
    let actor: String = r.get(5)?;
    let kind: String = r.get(6)?;
    let request: Vec<u8> = r.get(7)?;
    let created_at: i64 = r.get(8)?;
    let attempts: i64 = r.get(9)?;
    let subject_id: Option<String> = r.get(10)?;
    let (Ok(context), Ok(api_url), Ok(org), Ok(actor), Some(kind)) = (
        Context::parse(&context),
        ApiUrl::parse(&api_url),
        OrgId::parse(&org),
        ActorId::parse(&actor),
        Kind::parse(&kind),
    ) else {
        return Ok(None);
    };
    Ok(Some(DueRow {
        event_id,
        key: ActorKey {
            context,
            org,
            actor,
        },
        home: HomeRef {
            home_path,
            api_url,
            context,
        },
        kind,
        request,
        created_at,
        attempts: u32::try_from(attempts).unwrap_or(u32::MAX),
        subject_id,
    }))
}

impl Shared {
    /// The backoff before attempt `attempts + 1` (§3.6).
    fn backoff(&self, attempts: u32) -> Duration {
        let t = &self.cfg.timings;
        let i = usize::try_from(attempts.saturating_sub(1)).unwrap_or(usize::MAX);
        t.outbox_backoff.get(i).copied().unwrap_or(t.outbox_steady)
    }

    /// Runs the outbox until shutdown.
    pub async fn run_outbox(self: SharedRef) {
        let mut shutdown = self.shutdown.subscribe();
        loop {
            if *shutdown.borrow() {
                return;
            }
            match self.outbox_pass().await {
                Ok(n) if n > 0 => continue,
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "the outbox pass failed"),
            }
            let wait = self.next_due_in().await;
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = self.outbox_wake.notified() => {}
                _ = shutdown.changed() => {}
            }
        }
    }

    async fn next_due_in(&self) -> Duration {
        let next: Option<i64> = self
            .db
            .call(|c| {
                c.query_row(
                    "SELECT min(next_attempt_at) FROM outbox WHERE status IN ('pending','authority_required')",
                    [],
                    |r| r.get(0),
                )
                .sql()
            })
            .await
            .ok()
            .flatten();
        let idle = self.cfg.timings.outbox_idle;
        next.map_or(idle, |n| {
            let ms = u64::try_from(n.saturating_sub(now_ms()).max(0)).unwrap_or(0);
            Duration::from_millis(ms)
                .min(idle)
                .max(Duration::from_millis(5))
        })
    }

    /// One pass over due rows (up to 32, four at a time). Returns how many
    /// were attempted.
    ///
    /// # Errors
    /// Database failures.
    pub async fn outbox_pass(self: &SharedRef) -> Result<usize> {
        let now = now_ms();
        let due: Vec<DueRow> = self
            .db
            .call(move |c| {
                let mut st = c
                    .prepare(
                        "SELECT event_id, context, home_path, api_url, org_id, actor_id, kind, request, created_at, attempts, subject_id
                         FROM outbox WHERE status IN ('pending','authority_required') AND next_attempt_at <= ?1
                         ORDER BY next_attempt_at, created_at LIMIT 32",
                    )
                    .sql()?;
                let rows = st
                    .query_map([now], parse_row)
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok(rows.into_iter().flatten().collect())
            })
            .await?;
        let n = due.len();
        let mut set = JoinSet::new();
        let limit = Arc::new(tokio::sync::Semaphore::new(4));
        for row in due {
            let this = Arc::clone(self);
            let limit = Arc::clone(&limit);
            set.spawn(async move {
                let _permit = limit.acquire_owned().await;
                let outcome = this.attempt(&row).await;
                if let Err(e) = this.finish(&row, &outcome).await {
                    tracing::error!(event = %row.event_id, error = %e, "recording a delivery outcome failed");
                }
            });
        }
        while set.join_next().await.is_some() {}
        Ok(n)
    }

    fn max_age_ms(home: &HomeRef) -> i64 {
        let hours = Store::open_existing(std::path::Path::new(&home.home_path))
            .and_then(|s| s.read_config())
            .map_or(DEFAULT_DELIVERY_MAX_AGE_HOURS, |c| c.delivery_max_age_hours)
            .clamp(1, DEFAULT_DELIVERY_MAX_AGE_HOURS);
        i64::from(hours) * 3_600_000
    }

    /// Homes that may deliver for `key`: the row's own first, then any other
    /// attached home holding the same Silicon on the same backend.
    async fn candidate_homes(&self, row: &DueRow) -> Vec<HomeRef> {
        let mut out = vec![row.home.clone()];
        let key = row.key.clone();
        let api = row.home.api_url.as_str().to_owned();
        let own = row.home.home_path.clone();
        let others: Vec<String> = self
            .db
            .call(move |c| {
                let mut st = c
                    .prepare(
                        "SELECT home_path FROM homes WHERE actor_id = ?1 AND org_id = ?2 AND api_url = ?3 AND context = ?4 AND home_path != ?5
                         ORDER BY last_seen_at DESC",
                    )
                    .sql()?;
                let v = st
                    .query_map(
                        params![key.actor.as_str(), key.org.as_str(), api, key.context_str(), own],
                        |r| r.get(0),
                    )
                    .sql()?
                    .collect::<rusqlite::Result<Vec<String>>>()
                    .sql()?;
                Ok(v)
            })
            .await
            .unwrap_or_default();
        out.extend(others.into_iter().map(|home_path| HomeRef {
            home_path,
            api_url: row.home.api_url.clone(),
            context: row.home.context,
        }));
        out
    }

    /// A session of exactly this Silicon, from any candidate home.
    async fn session_for(
        &self,
        row: &DueRow,
    ) -> std::result::Result<(Client, SessionSlot, Store, HomeRef), Outcome> {
        let policy = RefreshPolicy::with_delays(Vec::new());
        let mut last: Option<Outcome> = None;
        for home in self.candidate_homes(row).await {
            match self.net.session(&home, DELIVERY_MARGIN, &policy).await {
                Ok((client, slot, store)) => {
                    if slot.actor.public_id == row.key.actor && slot.org_id == row.key.org {
                        return Ok((client, slot, store, home));
                    }
                    last.get_or_insert(Outcome::AuthorityRequired {
                        code: "authority_required".to_owned(),
                        request_id: None,
                    });
                }
                Err(e) if needs_authority(&e) => {
                    // An explicit logout of this actor cancels its rows.
                    if *e.code() == ErrorCode::NotLoggedIn
                        && let Ok(store) =
                            Store::open_existing(std::path::Path::new(&home.home_path))
                        && let Ok(file) = store.read_session()
                        && file
                            .logged_out
                            .as_ref()
                            .is_some_and(|l| l.actor == row.key.actor.as_str())
                        && home.home_path == row.home.home_path
                    {
                        return Err(Outcome::Cancelled {
                            code: "logged_out".to_owned(),
                        });
                    }
                    last.get_or_insert(Outcome::AuthorityRequired {
                        code: code_of(&e),
                        request_id: e.request_id().map(str::to_owned),
                    });
                }
                Err(e) => {
                    return Err(Outcome::Retry {
                        code: code_of(&e),
                        request_id: e.request_id().map(str::to_owned),
                        after: e.retry_after(),
                    });
                }
            }
        }
        Err(last.unwrap_or(Outcome::AuthorityRequired {
            code: "authority_required".to_owned(),
            request_id: None,
        }))
    }

    async fn attempt(&self, row: &DueRow) -> Outcome {
        let (outcome, home) = self.attempt_once(row).await;
        // A testing environment that was cleaned or re-imported has a new
        // generation: re-read it (GET /api/v1/iam) and try once more.
        match (&outcome, home) {
            (Outcome::AuthorityRequired { code, .. }, Some(home))
                if code == ErrorCode::TestingGenerationChanged.as_str()
                    && home.context.is_testing() =>
            {
                match self.net.refresh_generation(&home).await {
                    Ok(_) => self.attempt_once(row).await.0,
                    Err(e) => {
                        tracing::warn!(code = %e.code(), "refreshing the testing generation failed");
                        outcome
                    }
                }
            }
            _ => outcome,
        }
    }

    async fn attempt_once(&self, row: &DueRow) -> (Outcome, Option<HomeRef>) {
        if now_ms().saturating_sub(row.created_at) > Self::max_age_ms(&row.home) {
            return (Outcome::Expired, None);
        }
        let (client, slot, store, home) = match self.session_for(row).await {
            Ok(s) => s,
            Err(o) => return (o, None),
        };
        // Getting a session can take a while (a refresh): a row another path
        // cancelled meanwhile (a newer drawing, an unregister, a logout) is
        // not sent.
        if !self.still_due(&row.event_id).await {
            return (
                Outcome::Cancelled {
                    code: "superseded".to_owned(),
                },
                None,
            );
        }
        let first = self.call_backend(row, &client).await;
        let outcome = match first {
            Err(e) if e.status() == Some(401) && *e.code() != ErrorCode::SessionRejected => {
                // §3.6: IAM 401 on introspect → force one refresh, then retry.
                let (_, base) = match self.net.client(&home) {
                    Ok(c) => c,
                    Err(e) => return (classify(&e), Some(home)),
                };
                let policy = RefreshPolicy::with_delays(Vec::new());
                match force_refresh(&store, &base, home.context, &slot.access_token, &policy).await
                {
                    Ok(fresh) => {
                        let client =
                            base.with_session(fresh.access_token.clone(), fresh.org_id.clone());
                        match self.call_backend(row, &client).await {
                            Ok(o) => o,
                            Err(e) => classify(&e),
                        }
                    }
                    Err(e) => classify(&e),
                }
            }
            Ok(o) => o,
            Err(e) => classify(&e),
        };
        (outcome, Some(home))
    }

    async fn call_backend(&self, row: &DueRow, client: &Client) -> Result<Outcome> {
        match row.kind {
            Kind::Ting => {
                let event_id = EventId::parse(&row.event_id)?;
                let resp = client.deliver(&event_id, &row.request).await?;
                Ok(Outcome::Accepted {
                    ting_id: Some(resp.ting_id),
                    silent: Some(resp.silent),
                    request_id: None,
                })
            }
            Kind::DrawingPut => {
                let stored = client.put_drawing(&row.request).await?;
                let sha = crate::paths::sha256_hex(&row.request);
                if !stored.sha256.eq_ignore_ascii_case(&sha) {
                    tracing::warn!(sent = %sha, echoed = %stored.sha256, "peek-server echoed a different drawing hash");
                }
                let key = row.key.clone();
                self.db
                    .call(move |c| {
                        c.execute(
                            "UPDATE drawings SET server_sync = 'synced' WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3 AND sha256 = ?4",
                            params![key.context_str(), key.org.as_str(), key.actor.as_str(), sha],
                        )
                        .sql()
                    })
                    .await?;
                Ok(Outcome::Accepted {
                    ting_id: None,
                    silent: None,
                    request_id: None,
                })
            }
            Kind::DrawingDelete => match client.delete_drawing().await {
                Ok(()) => Ok(Outcome::Accepted {
                    ting_id: None,
                    silent: None,
                    request_id: None,
                }),
                Err(e) if matches!(e.code(), ErrorCode::DrawingNotFound | ErrorCode::NotFound) => {
                    Ok(Outcome::Accepted {
                        ting_id: None,
                        silent: None,
                        request_id: e.request_id().map(str::to_owned),
                    })
                }
                Err(e) => Err(e),
            },
        }
    }

    /// Whether a row is still waiting to be delivered (not cancelled or
    /// finished by another path since it was picked).
    async fn still_due(&self, event_id: &str) -> bool {
        let id = event_id.to_owned();
        self.db
            .call(move |c| {
                c.query_row(
                    "SELECT status IN ('pending','authority_required') FROM outbox WHERE event_id = ?1",
                    [id],
                    |r| r.get::<_, bool>(0),
                )
                .optional()
                .sql()
            })
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    async fn finish(&self, row: &DueRow, outcome: &Outcome) -> Result<()> {
        let now = now_ms();
        let attempts = if matches!(outcome, Outcome::Expired | Outcome::Cancelled { .. }) {
            row.attempts
        } else {
            row.attempts.saturating_add(1)
        };
        let next = match outcome {
            Outcome::Retry { after, .. } => {
                let b = self.backoff(attempts);
                let d = after.map_or(b, |a| a.max(b));
                now.saturating_add(i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            }
            // Keep pending; the backend's Retry-After (900 s) wins when sent.
            Outcome::TypeMissing { after, .. } => now.saturating_add(
                i64::try_from(
                    after
                        .unwrap_or(self.cfg.timings.ting_type_missing_retry)
                        .as_millis(),
                )
                .unwrap_or(i64::MAX),
            ),
            Outcome::AuthorityRequired { .. } => now.saturating_add(
                i64::try_from(self.cfg.timings.authority_sweep.as_millis()).unwrap_or(i64::MAX),
            ),
            _ => now,
        };
        let status = outcome.status();
        let code = outcome.code().map(str::to_owned);
        let (ting_id, silent, request_id) = match outcome {
            Outcome::Accepted {
                ting_id,
                silent,
                request_id,
            } => (ting_id.clone(), *silent, request_id.clone()),
            Outcome::Retry { request_id, .. }
            | Outcome::TypeMissing { request_id, .. }
            | Outcome::AuthorityRequired { request_id, .. }
            | Outcome::Failed { request_id, .. } => (None, None, request_id.clone()),
            _ => (None, None, None),
        };
        let accepted_at = matches!(outcome, Outcome::Accepted { .. }).then_some(now);
        let event_id = row.event_id.clone();
        let kind = row.kind;
        let key = row.key.clone();
        let accepted = matches!(outcome, Outcome::Accepted { .. });
        let changed = self
            .db
            .call(move |c| {
                // A row another path cancelled while this attempt was in
                // flight (a newer drawing, an unregister, a logout) stays
                // cancelled: an attempt that failed must not bring it back
                // to `pending` behind the newer row. An attempt that was
                // accepted did happen, so it is still recorded.
                let changed = c
                    .execute(
                        "UPDATE outbox SET status = ?2, attempts = ?3, next_attempt_at = ?4, last_error_code = ?5,
                                last_request_id = coalesce(?6, last_request_id), ting_id = coalesce(?7, ting_id),
                                silent = coalesce(?8, silent), accepted_at = coalesce(?9, accepted_at)
                         WHERE event_id = ?1 AND (?10 OR status IN ('pending','authority_required'))",
                        params![event_id, status, attempts, next, code, request_id, ting_id, silent, accepted_at, accepted],
                    )
                    .sql()?;
                if changed == 0 {
                    return Ok(0);
                }
                if kind == Kind::DrawingPut && matches!(status, "failed" | "expired" | "cancelled") {
                    c.execute(
                        "UPDATE drawings SET server_sync = 'failed' WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3 AND server_sync = 'pending'",
                        params![key.context_str(), key.org.as_str(), key.actor.as_str()],
                    )
                    .sql()?;
                }
                Ok(changed)
            })
            .await?;
        if changed == 0 {
            tracing::info!(event = %row.event_id, status, "a delivery was cancelled while its attempt ran; it stays cancelled");
            return Ok(());
        }
        if outcome.terminal()
            && let Some(subject) = &row.subject_id
        {
            let _ = std::fs::remove_file(self.paths.recording(subject));
        }
        if !matches!(outcome, Outcome::Accepted { .. }) {
            let code = outcome.code().unwrap_or("none");
            tracing::info!(event = %row.event_id, status, code, "delivery attempt did not complete");
        }
        let mut rec = Record::new(
            "delivery.attempt",
            if matches!(outcome, Outcome::Accepted { .. }) {
                "ok"
            } else {
                "error"
            },
        )
        .with("ting_status", status)
        .with("attempt", attempts)
        .with("method", row.kind.as_str());
        rec.error_code = outcome.code().map(str::to_owned);
        rec.actor = Some((row.key.org.clone(), row.key.actor.clone()));
        rec.testing = row.key.context.is_testing();
        self.record(rec);
        Ok(())
    }

    /// Cancels a Silicon's undelivered rows (`detach{reason:logout}`).
    ///
    /// # Errors
    /// Database failures.
    pub async fn cancel_undelivered(&self, key: &ActorKey) -> Result<u64> {
        let k = key.clone();
        let n = self
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE outbox SET status = 'cancelled', last_error_code = 'logged_out'
                     WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3 AND status IN ('pending','authority_required')",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                )
                .sql()
            })
            .await?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Makes a Silicon's `authority_required` rows due again (after a login
    /// or any authenticated CLI call); `now` forces an immediate retry,
    /// otherwise within a minute.
    ///
    /// # Errors
    /// Database failures.
    pub async fn wake_authority_rows(
        &self,
        key: &ActorKey,
        api: &ApiUrl,
        immediately: bool,
    ) -> Result<()> {
        let k = key.clone();
        let api = api.as_str().to_owned();
        let at = if immediately {
            now_ms()
        } else {
            now_ms() + 60_000
        };
        let n = self
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE outbox SET next_attempt_at = min(next_attempt_at, ?5)
                     WHERE status = 'authority_required' AND context = ?1 AND org_id = ?2 AND actor_id = ?3 AND api_url = ?4",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str(), api, at],
                )
                .sql()
            })
            .await?;
        if n > 0 {
            self.outbox_wake.notify_one();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(code: ErrorCode, status: u16) -> Error {
        Error::new(code, "x").with_status(status)
    }

    #[test]
    fn classification_matrix() {
        let t = |e: &Error| classify(e).status();
        assert_eq!(
            t(&err(ErrorCode::RecipientNotRegistered, 409)),
            "authority_required"
        );
        assert_eq!(
            t(&err(ErrorCode::ReconsentRequired, 403)),
            "authority_required"
        );
        assert_eq!(t(&err(ErrorCode::TingRejected, 502)), "failed");
        assert_eq!(t(&err(ErrorCode::TingKeyConflict, 409)), "failed");
        assert_eq!(t(&err(ErrorCode::TingUnavailable, 503)), "pending");
        assert!(matches!(
            classify(&err(ErrorCode::TingTypeMissing, 502)),
            Outcome::TypeMissing { .. }
        ));
        assert_eq!(t(&err(ErrorCode::Other("weird".into()), 418)), "failed");
        assert_eq!(t(&err(ErrorCode::Other("weird".into()), 500)), "pending");
        let transport = Error::new(ErrorCode::BackendUnavailable, "x")
            .with_origin(silicon_peek_client::error::Origin::Transport);
        assert_eq!(t(&transport), "pending");
        let after = Error::new(ErrorCode::TingUnavailable, "x")
            .with_status(503)
            .with_retry_after(Some(Duration::from_secs(7)));
        assert_eq!(
            classify(&after),
            Outcome::Retry {
                code: "ting_unavailable".into(),
                request_id: None,
                after: Some(Duration::from_secs(7))
            }
        );
    }
}
