//! The telemetry relay (BLUEPRINT §6.3, D17): peekd records its own events
//! and relays the CLI's and Peek.app's through peek-server's gateway
//! `POST /api/web/telemetry`. No table key ever reaches this machine.
//!
//! The outbox (`telemetry_outbox`) holds at most 1,000 rows (oldest dropped)
//! and is posted in batches of up to 40 every 10 s. Testing-context events
//! are never relayed. Any opt-out (env, Settings, the home's config) wins.

use std::{collections::BTreeMap, time::Duration};

use serde_json::{Map, Value, json};
use silicon_peek_client::{
    Error, Result,
    api::{TelemetryBatch, TelemetryEvent, TelemetryTable},
    identity::{AccountId, ActorId},
    runtime::fs::read_private,
    telemetry::{actor_hash, scrub_context},
    timestamp::Timestamp,
};
use uuid::Uuid;

use rusqlite::OptionalExtension as _;

use crate::{config::DEV_BUNDLE_ID, db::SqlResult as _, state::Shared};

/// Rows kept in the relay outbox.
pub const OUTBOX_CAP: i64 = 1000;
/// Events per gateway request.
pub const BATCH_MAX: usize = 40;

/// Peek.app's automatic-analytics events (§6.5); every other UI event is an
/// explicit event.
const MAC_ANALYTICS: [&str; 7] = [
    "app_launched",
    "peek_visible",
    "render_error",
    "fallback_visual",
    "glass_mode",
    "display_changed",
    "appearance_changed",
];

/// Process-wide telemetry facts.
#[derive(Clone, Debug)]
pub struct Telemetry {
    os_version: Option<String>,
    locale: Option<String>,
    environment: &'static str,
}

/// One daemon event.
#[derive(Clone, Debug, Default)]
pub struct Record {
    /// The event name (`send.displayed`, `delivery.attempt`, …).
    pub event: &'static str,
    /// `ok`, `error`, `skipped` or `timeout`.
    pub outcome: &'static str,
    /// Duration, when measured.
    pub duration_ms: Option<u64>,
    /// The error code, on failure.
    pub error_code: Option<String>,
    /// The Silicon (hashed before recording). Dropped when any home of
    /// this Silicon opted out of telemetry.
    pub actor: Option<(AccountId, ActorId)>,
    /// The Silicon home the event came from (`<SILICON_HOME>/.peek`), when
    /// known without an actor (an IPC request). Dropped when it opted out.
    pub home: Option<String>,
    /// The ISI of the send, if any.
    pub isi: Option<String>,
    /// Allowlisted context keys (anything else is dropped).
    pub context: Map<String, Value>,
}

impl Record {
    /// A record for `event` with `outcome`.
    #[must_use]
    pub fn new(event: &'static str, outcome: &'static str) -> Self {
        Self {
            event,
            outcome,
            ..Self::default()
        }
    }

    /// Adds a context key.
    #[must_use]
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.context.insert(key.to_owned(), value.into());
        self
    }
}

fn read_os_version() -> Option<String> {
    let bytes = read_private(std::path::Path::new(
        "/System/Library/CoreServices/SystemVersion.plist",
    ))
    .ok()
    .flatten()
    .or_else(|| std::fs::read("/System/Library/CoreServices/SystemVersion.plist").ok())?;
    crate::update::plist_string(&bytes, "ProductVersion")
}

impl Telemetry {
    /// Facts for this process and bundle.
    #[must_use]
    pub fn new(bundle_id: &str) -> Self {
        Self {
            os_version: read_os_version(),
            locale: std::env::var("LANG")
                .ok()
                .map(|l| l.split('.').next().unwrap_or_default().to_owned())
                .filter(|l| !l.is_empty()),
            environment: if bundle_id == DEV_BUNDLE_ID {
                "development"
            } else {
                "production"
            },
        }
    }

    fn envelope(&self, r: Record, instance_id: &str) -> Value {
        let mut context = r.context;
        scrub_context(&mut context);
        let mut data = json!({
            "schema_version": 1,
            "app": "peek",
            "service": "peek-daemon",
            "source": "daemon",
            "version": silicon_peek_client::VERSION,
            "environment": self.environment,
            "instance_id": instance_id,
            "event": r.event,
            "outcome": r.outcome,
            "duration_ms": r.duration_ms,
            "error_code": r.error_code,
            "client": {
                "os": "macos",
                "os_version": self.os_version,
                "arch": std::env::consts::ARCH,
                "locale": self.locale,
            },
            "context": context,
        });
        if let Some(isi) = r.isi {
            data["isi"] = Value::String(isi);
        }
        if let Some((account, actor)) = &r.actor {
            data["actor"] = json!({
                "kind": actor.actor_type().as_str(),
                "hash": actor_hash(account, actor),
            });
        }
        data
    }
}

impl Shared {
    /// Whether peekd records and relays telemetry at all right now.
    #[must_use]
    pub fn telemetry_on(&self) -> bool {
        self.cfg.telemetry_enabled && self.settings.get().telemetry
    }

    /// Records one daemon event (fire and forget). An event about a
    /// Silicon, or from a Silicon home, is dropped when that Silicon's home
    /// opted out (`peek config telemetry off`, mirrored by `config.sync`):
    /// the explicit opt-out always wins (§6.6).
    pub fn record(&self, r: Record) {
        if !self.telemetry_on() {
            return;
        }
        let owner = (r.actor.clone(), r.home.clone());
        let event_type = r.event.to_owned();
        let data = self.telemetry.envelope(r, &self.instance_id);
        let event = TelemetryEvent {
            id: Uuid::now_v7().simple().to_string(),
            event_type,
            data,
            metadata: json!({"occurred_at": Timestamp::now().to_rfc3339()}),
        };
        let db = self.db.clone();
        tokio::spawn(async move {
            if !owner_allows_telemetry(&db, owner).await {
                return;
            }
            if let Err(e) =
                insert_events(&db, TelemetryTable::Peekclidaemon, "daemon", vec![event]).await
            {
                tracing::debug!(error = %e, "a telemetry event could not be queued");
            }
        });
    }

    /// Queues relayed events (CLI or UI), enforcing the cap.
    ///
    /// # Errors
    /// Database failures.
    pub async fn enqueue_telemetry(
        &self,
        table: TelemetryTable,
        source: &'static str,
        events: Vec<TelemetryEvent>,
    ) -> Result<()> {
        if !self.telemetry_on() || events.is_empty() {
            return Ok(());
        }
        insert_events(&self.db, table, source, events).await
    }

    /// Drops every queued event (Settings turned telemetry off).
    ///
    /// # Errors
    /// Database failures.
    pub async fn clear_telemetry(&self) -> Result<()> {
        self.db
            .call(|c| {
                c.execute("DELETE FROM telemetry_outbox", [])
                    .sql()
                    .map(|_| ())
            })
            .await
    }

    /// Relays telemetry every interval until shutdown.
    pub async fn run_telemetry(self: std::sync::Arc<Self>) {
        let mut shutdown = self.shutdown.subscribe();
        loop {
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(self.cfg.timings.telemetry_interval) => {}
                _ = shutdown.changed() => return,
            }
            if let Err(e) = self.relay_telemetry_once().await {
                tracing::debug!(error = %e, "the telemetry relay pass failed");
            }
        }
    }

    /// Posts up to one batch per (table, source); returns how many events
    /// were accepted.
    ///
    /// # Errors
    /// Database failures (gateway failures keep the rows for the next tick).
    pub async fn relay_telemetry_once(&self) -> Result<usize> {
        if !self.telemetry_on() {
            return Ok(0);
        }
        let rows: Vec<(String, String, String, Vec<u8>)> = self
            .db
            .call(|c| {
                let mut st = c
                    .prepare("SELECT id, table_id, source, event FROM telemetry_outbox ORDER BY created_at, id LIMIT 400")
                    .sql()?;
                let rows = st
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok(rows)
            })
            .await?;
        let mut groups: BTreeMap<(String, String), Vec<(String, TelemetryEvent)>> = BTreeMap::new();
        let mut corrupt = Vec::new();
        for (id, table, source, bytes) in rows {
            match serde_json::from_slice::<TelemetryEvent>(&bytes) {
                Ok(e) => {
                    let g = groups.entry((table, source)).or_default();
                    if g.len() < BATCH_MAX {
                        g.push((id, e));
                    }
                }
                Err(_) => corrupt.push(id),
            }
        }
        let client = self
            .net
            .api_client(&self.cfg.telemetry_api)?
            .with_trace_id(self.instance_id.clone());
        let mut sent = 0;
        let mut done = corrupt;
        for ((table, source), items) in groups {
            let Ok(table_enum) =
                serde_json::from_value::<TelemetryTable>(Value::String(table.clone()))
            else {
                done.extend(items.into_iter().map(|(id, _)| id));
                continue;
            };
            let (ids, events): (Vec<String>, Vec<TelemetryEvent>) = items.into_iter().unzip();
            let batch = TelemetryBatch {
                table: table_enum,
                events,
            };
            match client
                .telemetry(&batch, &source, Some(Duration::from_secs(5)))
                .await
            {
                Ok(()) => {
                    sent += ids.len();
                    done.extend(ids);
                }
                Err(e)
                    if e.status()
                        .is_some_and(|s| (400..500).contains(&s) && s != 429) =>
                {
                    // The gateway refuses this batch for good; drop it.
                    tracing::warn!(error = %e, table, "the telemetry gateway refused a batch; dropping it");
                    done.extend(ids);
                }
                Err(e) => tracing::debug!(error = %e, "telemetry relay failed; will retry"),
            }
        }
        if !done.is_empty() {
            self.db
                .tx(move |tx| {
                    for id in done {
                        tx.execute("DELETE FROM telemetry_outbox WHERE id = ?1", [id])
                            .sql()?;
                    }
                    Ok(())
                })
                .await?;
        }
        Ok(sent)
    }
}

/// Inserts events into the relay outbox and trims it to [`OUTBOX_CAP`].
/// Whether a home's own config allows telemetry: its `config.json` (the
/// source of truth, read fresh) and peekd's `config.sync` mirror; either
/// saying off wins. An unreadable config counts as the default (on).
#[must_use]
pub fn home_config_allows_telemetry(home_path: &str, mirrored: Option<&str>) -> bool {
    let mirror_on = mirrored
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.get("telemetry").and_then(Value::as_bool))
        .unwrap_or(true);
    let file_on =
        silicon_peek_client::runtime::Store::open_existing(std::path::Path::new(home_path))
            .and_then(|s| s.read_config())
            .map_or(true, |c| c.telemetry);
    mirror_on && file_on
}

/// Whether an event's Silicon (every home known to hold it) and its home
/// allow telemetry.
async fn owner_allows_telemetry(
    db: &crate::db::Db,
    (actor, home): (Option<(AccountId, ActorId)>, Option<String>),
) -> bool {
    if actor.is_none() && home.is_none() {
        return true;
    }
    db.call(move |c| {
        let mut homes: Vec<(String, Option<String>)> = Vec::new();
        if let Some((account, _)) = &actor {
            let mut st = c
                .prepare("SELECT home_path, config FROM homes WHERE account_id = ?1")
                .sql()?;
            let rows = st
                .query_map(rusqlite::params![account.as_str()], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .sql()?
                .collect::<rusqlite::Result<Vec<_>>>()
                .sql()?;
            homes.extend(rows);
        }
        if let Some(hp) = home
            && !homes.iter().any(|(h, _)| *h == hp)
        {
            let mirrored: Option<Option<String>> = c
                .query_row("SELECT config FROM homes WHERE home_path = ?1", [&hp], |r| {
                    r.get(0)
                })
                .optional()
                .sql()?;
            homes.push((hp, mirrored.flatten()));
        }
        Ok(homes
            .iter()
            .all(|(hp, cfg)| home_config_allows_telemetry(hp, cfg.as_deref())))
    })
    .await
    // Unknown: never record against a possible opt-out.
    .unwrap_or(false)
}

async fn insert_events(
    db: &crate::db::Db,
    table: TelemetryTable,
    source: &'static str,
    events: Vec<TelemetryEvent>,
) -> Result<()> {
    let table_id = serde_json::to_value(table)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "peekclidaemon".to_owned());
    let now = Timestamp::now().unix_ms();
    let rows: Vec<(String, Vec<u8>)> = events
        .into_iter()
        .filter(|e| e.data.get("environment").and_then(Value::as_str) != Some("testing"))
        .filter_map(|e| serde_json::to_vec(&e).ok().map(|b| (e.id.clone(), b)))
        .collect();
    if rows.is_empty() {
        return Ok(());
    }
    db.tx(move |tx| {
        for (id, bytes) in rows {
            tx.execute(
                "INSERT OR IGNORE INTO telemetry_outbox (id, table_id, event, created_at, source) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![format!("{source}:{id}"), table_id, bytes, now, source],
            )
            .sql()?;
        }
        tx.execute(
            "DELETE FROM telemetry_outbox WHERE id IN (SELECT id FROM telemetry_outbox ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?1)",
            [OUTBOX_CAP],
        )
        .sql()?;
        Ok(())
    })
    .await
}

/// Which table a Peek.app event belongs to: an explicit
/// `metadata.table`, else by name (§6.5).
#[must_use]
pub fn ui_table(event: &TelemetryEvent) -> TelemetryTable {
    match event.metadata.get("table").and_then(Value::as_str) {
        Some("peekfrontendanalytics") => TelemetryTable::Peekfrontendanalytics,
        Some("peekfrontendevents") => TelemetryTable::Peekfrontendevents,
        _ if MAC_ANALYTICS.contains(&event.event_type.as_str()) => {
            TelemetryTable::Peekfrontendanalytics
        }
        _ => TelemetryTable::Peekfrontendevents,
    }
}

/// Validates relayed events: 1–40 per call, each with an id and a type.
///
/// # Errors
/// `invalid_input`.
pub fn check_events(events: &[TelemetryEvent]) -> Result<()> {
    if events.len() > BATCH_MAX {
        return Err(Error::invalid_input(format!(
            "a telemetry call carries at most {BATCH_MAX} events, not {}",
            events.len()
        )));
    }
    for e in events {
        if e.id.is_empty()
            || e.id.len() > 128
            || e.event_type.is_empty()
            || e.event_type.len() > 128
        {
            return Err(Error::invalid_input(
                "every telemetry event needs an id and a type of 1–128 characters",
            ));
        }
    }
    Ok(())
}
