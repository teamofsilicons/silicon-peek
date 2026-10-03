//! Silicon homes as peekd learns them (§2.8): the CLI `auth` block checks,
//! the `homes` table and its `homes.json` mirror, `attach`/`detach`,
//! `config.sync`, pending revocations, and launching Peek.app.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use rusqlite::{OptionalExtension as _, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    api::TestingEnvironment,
    identity::{ActorId, Context, SlotKey},
    ipc::{
        AuthBlock,
        cli::{AttachResult, ConfigSyncConfig, DetachResult, Warning, warnings},
        ui::CancelReason,
    },
    runtime::{Store, authenticate_home, login::revoke_pending, session::RELOGIN_HINT},
};

use crate::{
    bubbles::now_ms,
    commands::CommandSpec,
    db::SqlResult as _,
    net::HomeRef,
    state::{ActorKey, Caller, Shared, SharedRef},
};

/// `homes.json` row.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HomeEntry {
    /// Canonical `<SILICON_HOME>/.peek`.
    pub home_path: String,
    /// The `SILICON_HOME` itself.
    pub silicon_home: String,
    /// Last backend seen.
    pub api_url: String,
    /// Last context seen.
    pub context: String,
    /// Last org.
    pub org_id: Option<String>,
    /// Last Silicon.
    pub actor_id: Option<String>,
    /// Unix ms.
    pub last_seen_at: i64,
}

fn token_sha(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// peekd's stable id for a home: the first 16 hex of `sha256(home_path)`.
#[must_use]
pub fn home_id(home_path: &str) -> String {
    let mut h = hex::encode(Sha256::digest(home_path.as_bytes()));
    h.truncate(16);
    h
}

fn tcc_denied(home: &str) -> Error {
    Error::new(
        ErrorCode::AuthorityRequired,
        format!("macOS denied peekd access to {home} (Files & Folders privacy)"),
    )
    .with_hint("move SILICON_HOME out of a protected folder (Documents, Desktop, Downloads) or allow Peek in System Settings → Privacy & Security → Files and Folders")
}

impl Shared {
    /// §1.6's checks on a CLI `auth` block, in order: private directory,
    /// home token, unrejected slot. Identity comes from the slot.
    ///
    /// # Errors
    /// `invalid_silicon_home`, `home_token_mismatch`, `not_logged_in`,
    /// `session_rejected`, or `authority_required` for a TCC denial.
    pub async fn authenticate(&self, auth: &AuthBlock) -> Result<Caller> {
        if let Err(e) = std::fs::metadata(&auth.home)
            && e.raw_os_error() == Some(libc::EPERM)
        {
            return Err(tcc_denied(&auth.home));
        }
        let block = auth.clone();
        let verified = tokio::task::spawn_blocking(move || authenticate_home(&block))
            .await
            .map_err(|e| Error::internal(format!("authenticating a home failed: {e}")))??;
        let testing = match auth.context {
            Context::Production => None,
            Context::Testing(id) => verified
                .store
                .read_testing()
                .ok()
                .and_then(|t| t.environments.get(&id).cloned())
                .map(|env| TestingEnvironment {
                    id,
                    name: env.name,
                    generation: env.generation,
                }),
        };
        let caller = Caller {
            key: ActorKey {
                context: auth.context,
                org: verified.org_id.clone(),
                actor: verified.actor_id.clone(),
            },
            home: HomeRef {
                home_path: verified.store.dir().to_string_lossy().into_owned(),
                api_url: auth.api_url.clone(),
                context: auth.context,
            },
            display_name: verified.display_name.clone(),
            ting_subscribed: verified.ting_subscribed,
            store: verified.store,
            testing,
        };
        self.record_home(&caller, auth.home_token.expose()).await?;
        Ok(caller)
    }

    async fn record_home(&self, caller: &Caller, token: &str) -> Result<()> {
        let sha = token_sha(token);
        let c2 = caller.clone();
        let now = now_ms();
        let changed = self
            .db
            .call(move |c| {
                let before: Option<(String, String, Option<String>, Option<String>)> = c
                    .query_row(
                        "SELECT api_url, context, org_id, actor_id FROM homes WHERE home_path = ?1",
                        [&c2.home.home_path],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()
                    .sql()?;
                c.execute(
                    "INSERT INTO homes (home_path, token_sha256, api_url, context, org_id, actor_id, attached_at, last_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                     ON CONFLICT(home_path) DO UPDATE SET token_sha256 = excluded.token_sha256, api_url = excluded.api_url,
                        context = excluded.context, org_id = excluded.org_id, actor_id = excluded.actor_id,
                        last_seen_at = excluded.last_seen_at",
                    params![
                        c2.home.home_path,
                        sha,
                        c2.home.api_url.as_str(),
                        c2.key.context_str(),
                        c2.key.org.as_str(),
                        c2.key.actor.as_str(),
                        now
                    ],
                )
                .sql()?;
                // A Silicon's slot follows the home it last used.
                c.execute(
                    "UPDATE slots SET home_path = ?4, api_url = ?5, display_name = coalesce(?6, display_name)
                     WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![
                        c2.key.context_str(),
                        c2.key.org.as_str(),
                        c2.key.actor.as_str(),
                        c2.home.home_path,
                        c2.home.api_url.as_str(),
                        c2.display_name
                    ],
                )
                .sql()?;
                let after = (
                    c2.home.api_url.as_str().to_owned(),
                    c2.key.context_str(),
                    Some(c2.key.org.as_str().to_owned()),
                    Some(c2.key.actor.as_str().to_owned()),
                );
                Ok(before.as_ref() != Some(&after))
            })
            .await?;
        if changed {
            self.write_homes_json().await;
        }
        self.wake_authority_rows(&caller.key, &caller.home.api_url, false)
            .await?;
        Ok(())
    }

    /// Every known home.
    ///
    /// # Errors
    /// Database failures.
    pub async fn known_homes(&self) -> Result<Vec<HomeEntry>> {
        self.db
            .call(|c| {
                let mut st = c
                    .prepare("SELECT home_path, api_url, context, org_id, actor_id, last_seen_at FROM homes ORDER BY home_path")
                    .sql()?;
                let rows = st
                    .query_map([], |r| {
                        let home_path: String = r.get(0)?;
                        let silicon_home = Path::new(&home_path)
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        Ok(HomeEntry {
                            home_path,
                            silicon_home,
                            api_url: r.get(1)?,
                            context: r.get(2)?,
                            org_id: r.get(3)?,
                            actor_id: r.get(4)?,
                            last_seen_at: r.get(5)?,
                        })
                    })
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok(rows)
            })
            .await
    }

    /// Rewrites `homes.json` (the registries the updater scans).
    pub async fn write_homes_json(&self) {
        let Ok(homes) = self.known_homes().await else {
            return;
        };
        let body = json!({"schema": 1, "homes": homes});
        let mut bytes = serde_json::to_vec_pretty(&body).unwrap_or_default();
        bytes.push(b'\n');
        if let Err(e) = silicon_peek_client::runtime::fs::write_atomic(
            &self.paths.support,
            "homes.json",
            &bytes,
        ) {
            tracing::warn!(error = %e, "writing homes.json failed");
        }
    }

    /// `attach` (after every login): records the home, drains
    /// `authority_required` rows now, and retries pending revocations.
    ///
    /// # Errors
    /// Database failures.
    pub async fn attach(self: &SharedRef, caller: &Caller) -> Result<AttachResult> {
        self.wake_authority_rows(&caller.key, &caller.home.api_url, true)
            .await?;
        let this = Arc::clone(self);
        let home = caller.home.clone();
        tokio::spawn(async move { this.sweep_revocations(&home.home_path).await });
        Ok(AttachResult {
            home_id: home_id(&caller.home.home_path),
            actor_id: caller.key.actor.clone(),
            org_id: caller.key.org.clone(),
        })
    }

    /// `detach{reason:logout}`: the slot is already gone from `session.json`
    /// (§2.6 step 3), so only checks 1–2 apply; the actor comes from the
    /// logout tombstone or peekd's own record of the home. Cancels the
    /// Silicon's undelivered rows and pending asks.
    ///
    /// # Errors
    /// `invalid_silicon_home`, `home_token_mismatch`, `not_logged_in`.
    pub async fn detach(self: &SharedRef, auth: &AuthBlock) -> Result<DetachResult> {
        let block = auth.clone();
        let (store, file) = tokio::task::spawn_blocking(move || -> Result<_> {
            let store = Store::open_existing(Path::new(&block.home))?;
            let token = store.daemon_token()?.ok_or_else(|| {
                Error::new(
                    ErrorCode::NotLoggedIn,
                    format!(
                        "{} has no daemon-token; this home never logged in",
                        store.dir().display()
                    ),
                )
            })?;
            if !token.ct_eq(block.home_token.expose()) {
                return Err(Error::new(
                    ErrorCode::HomeTokenMismatch,
                    format!(
                        "the presented home token does not match {}/daemon-token",
                        store.dir().display()
                    ),
                )
                .with_hint("run the command from the Silicon's own home (SILICON_HOME)"));
            }
            let file = store.read_session()?;
            Ok((store, file))
        })
        .await
        .map_err(|e| Error::internal(format!("authenticating a home failed: {e}")))??;
        let slot_key = SlotKey::new(auth.api_url.clone(), auth.context);
        let home_path = store.dir().to_string_lossy().into_owned();
        let key = if let Some(slot) = file.slot(&slot_key) {
            if auth.context_id.as_deref() != Some(slot.context_id()?) {
                return Err(Error::new(
                    ErrorCode::SessionRejected,
                    "this logout belongs to a previous login context",
                ));
            }
            Some((slot.org_id.clone(), slot.actor.public_id.clone()))
        } else {
            let tomb = file
                .logged_out
                .as_ref()
                .filter(|l| l.slot.as_deref().is_none_or(|s| s == slot_key.as_string()));
            match tomb {
                Some(t) if t.context_id.is_some() && t.context_id == auth.context_id => t
                    .org_id
                    .as_ref()
                    .map(|org| ActorId::parse(&t.actor).map(|actor| (org.clone(), actor)))
                    .transpose()?,
                _ => None,
            }
        };
        let Some((org, actor)) = key else {
            return Err(Error::new(
                ErrorCode::NotLoggedIn,
                format!("peekd has no record of which Silicon {home_path} held in {slot_key}"),
            )
            .with_hint(RELOGIN_HINT));
        };
        let key = ActorKey {
            context: auth.context,
            org,
            actor,
        };
        let cancelled_rows = self.cancel_undelivered(&key).await?;
        self.cancel_actor_bubbles(&key, CancelReason::Unregistered)
            .await?;
        Ok(DetachResult { cancelled_rows })
    }

    /// `config.sync`: mirrors the home's config (voice, language, notify,
    /// telemetry). The CLI sends its *effective* telemetry: `false` also when
    /// only its environment opts out (`PEEK_TELEMETRY`,
    /// `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY`), so peekd honours
    /// that opt-out for everything it does for the home, including
    /// `X-Peek-Telemetry: off` on its backend calls.
    ///
    /// # Errors
    /// Database failures.
    pub async fn config_sync(&self, caller: &Caller, config: &ConfigSyncConfig) -> Result<()> {
        let hp = caller.home.home_path.clone();
        let body = serde_json::to_string(config)
            .map_err(|e| Error::internal(format!("serializing config failed: {e}")))?;
        self.db
            .call(move |c| {
                c.execute(
                    "UPDATE homes SET config = ?2 WHERE home_path = ?1",
                    params![hp, body],
                )
                .sql()
                .map(|_| ())
            })
            .await?;
        self.net
            .set_home_opted_out(&caller.home.home_path, !config.telemetry);
        Ok(())
    }

    /// A CLI of this home handed peekd telemetry, which it does only when
    /// neither the home's config nor its environment opts out: an opt-out
    /// the mirror holds from an earlier CLI environment (`env_opt_out`) is
    /// over. A config opt-out is never lifted here (and the home's
    /// `config.json` is always read as well).
    ///
    /// # Errors
    /// Database failures.
    pub async fn home_telemetry_confirmed(&self, home_path: &str) -> Result<()> {
        let hp = home_path.to_owned();
        let cleared = self
            .db
            .call(move |c| {
                let cfg: Option<Option<String>> = c
                    .query_row(
                        "SELECT config FROM homes WHERE home_path = ?1",
                        [&hp],
                        |r| r.get(0),
                    )
                    .optional()
                    .sql()?;
                let Some(mut mirror) = cfg
                    .flatten()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                else {
                    return Ok(false);
                };
                if mirror.get("telemetry").and_then(Value::as_bool) != Some(false)
                    || mirror.get("env_opt_out").and_then(Value::as_bool) != Some(true)
                {
                    return Ok(false);
                }
                mirror["telemetry"] = Value::Bool(true);
                if let Some(m) = mirror.as_object_mut() {
                    m.remove("env_opt_out");
                }
                c.execute(
                    "UPDATE homes SET config = ?2 WHERE home_path = ?1",
                    params![hp, mirror.to_string()],
                )
                .sql()?;
                Ok(true)
            })
            .await?;
        if cleared {
            self.net.set_home_opted_out(home_path, false);
        }
        Ok(())
    }

    /// Loads the homes whose mirrored config opts out of telemetry into
    /// [`crate::net::Net`] (at startup).
    ///
    /// # Errors
    /// Database failures.
    pub async fn load_telemetry_mirror(&self) -> Result<usize> {
        let opted_out: Vec<String> = self
            .db
            .call(|c| {
                let mut st = c
                    .prepare("SELECT home_path, config FROM homes WHERE config IS NOT NULL")
                    .sql()?;
                let rows = st
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .sql()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .sql()?;
                Ok(rows
                    .into_iter()
                    .filter(|(_, cfg)| {
                        serde_json::from_str::<Value>(cfg)
                            .ok()
                            .and_then(|v| v.get("telemetry").and_then(Value::as_bool))
                            == Some(false)
                    })
                    .map(|(hp, _)| hp)
                    .collect())
            })
            .await?;
        for hp in &opted_out {
            self.net.set_home_opted_out(hp, true);
        }
        Ok(opted_out.len())
    }

    /// Whether a home's config (its `config.json` and the mirror) allows
    /// telemetry.
    pub async fn home_telemetry(&self, home_path: &str) -> bool {
        let hp = home_path.to_owned();
        let hp_for_file = hp.clone();
        self.db
            .call(move |c| {
                let cfg: Option<Option<String>> = c
                    .query_row("SELECT config FROM homes WHERE home_path = ?1", [hp], |r| {
                        r.get(0)
                    })
                    .optional()
                    .sql()?;
                Ok(crate::telemetry::home_config_allows_telemetry(
                    &hp_for_file,
                    cfg.flatten().as_deref(),
                ))
            })
            .await
            .unwrap_or(true)
    }

    /// Retries a home's queued refresh-token revocations (§2.6 step 4).
    pub async fn sweep_revocations(&self, home_path: &str) {
        let Ok(store) = Store::open_existing(Path::new(home_path)) else {
            return;
        };
        let net = self.net.clone();
        let hp = home_path.to_owned();
        let result = revoke_pending(&store, move |slot: &SlotKey| {
            net.client(&HomeRef {
                home_path: hp.clone(),
                api_url: slot.api_url().clone(),
                context: slot.context(),
            })
            .ok()
            .map(|(_, c)| c)
        })
        .await;
        match result {
            Ok(s) if s.confirmed + s.pending > 0 => {
                tracing::info!(
                    home = home_path,
                    confirmed = s.confirmed,
                    pending = s.pending,
                    "revocation sweep"
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(home = home_path, error = %e, "revocation sweep failed"),
        }
    }

    /// The pending fallback-visual error of a Silicon, attached once to its
    /// next CLI result (§7.5, visual.md A7).
    pub async fn take_fallback_warning(&self, key: &ActorKey) -> Option<Warning> {
        let k = key.clone();
        let err: Option<String> = self
            .db
            .call(move |c| {
                let e: Option<Option<String>> = c
                    .query_row(
                        "SELECT last_error FROM drawings WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3 AND error_pending = 1",
                        params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                        |r| r.get(0),
                    )
                    .optional()
                    .sql()?;
                c.execute(
                    "UPDATE drawings SET error_pending = 0 WHERE context = ?1 AND org_id = ?2 AND actor_id = ?3",
                    params![k.context_str(), k.org.as_str(), k.actor.as_str()],
                )
                .sql()?;
                Ok(e.flatten())
            })
            .await
            .ok()
            .flatten();
        let v: Value = serde_json::from_str(&err?).ok()?;
        let message = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let reason = v.get("reason").and_then(Value::as_str).unwrap_or("throws");
        Some(Warning {
            code: warnings::DRAWING_FALLBACK_ACTIVE.to_owned(),
            message: format!(
                "your drawing stopped ({reason}) and Peek shows the fallback visual: {message}; fix it and run `peek register drawing` again"
            ),
            details: Some(v),
        })
    }

    /// Opens Peek.app in the background (`open -g -j`) when it is not
    /// connected, at most every 30 s.
    pub fn launch_ui_soon(self: &SharedRef) {
        if !self.cfg.launch_ui || self.ui.is_connected() {
            return;
        }
        {
            let Ok(mut last) = self.last_launch.lock() else {
                return;
            };
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
                return;
            }
            *last = Some(Instant::now());
        }
        let app = self.cfg.app_path();
        if !app.exists() {
            tracing::warn!(app = %app.display(), "Peek.app is not installed; bubbles wait until it runs (peek app install)");
            return;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let spec = CommandSpec::new("/usr/bin/open")
                .arg("-g")
                .arg("-j")
                .arg(app.as_os_str())
                .arg("--args")
                .arg("--launched-by")
                .arg("peekd")
                .timeout(Duration::from_secs(15));
            match this.cfg.commands.run(spec).await {
                Ok(out) if out.success() => tracing::info!("asked LaunchServices to open Peek.app"),
                Ok(out) => tracing::warn!(detail = %out.last_line(), "opening Peek.app failed"),
                Err(e) => tracing::warn!(error = %e, "opening Peek.app failed"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_home_ids() {
        assert_eq!(home_id("/a/.peek"), home_id("/a/.peek"));
        assert_ne!(home_id("/a/.peek"), home_id("/b/.peek"));
        assert_eq!(home_id("/a/.peek").len(), 16);
    }
}
