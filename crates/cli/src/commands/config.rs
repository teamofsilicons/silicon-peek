//! `peek config …` (BLUEPRINT §7.3). `config set '<json-object>'` is the
//! Stemcell contract: strict parse (non-object, duplicate or unknown keys →
//! exit 2 with `details.valid_keys`), atomic merge under the store lock,
//! best-effort `config.sync` to peekd, then the resulting config is printed.

use std::{path::Path, time::Duration};

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    config::Config,
    ipc::cli::{ConfigSync, ConfigSyncConfig, HelloResult, features},
    runtime::{
        Store, auth_block,
        fs::{read_private, remove_file, write_atomic},
        store::{
            CONFIG_FILE, DAEMON_TOKEN_FILE, HOME_POINTER_FILE, SESSION_FILE, STORE_DIR,
            set_home_pointer, silicon_home,
        },
    },
    schema::send::Notify,
    telemetry::env_opt_out,
};

use super::next;
use crate::{
    cli::{ConfigCommand, OnOff},
    context::Globals,
    output::{Out, kv},
    service,
};

pub async fn run(g: &Globals, out: Out, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Set { object } => merge(g, out, &object).await,
        ConfigCommand::Show => {
            let store = crate::context::store()?;
            let config = store.read_config()?;
            out.value(&config.to_public_value(), kv);
            Ok(())
        }
        ConfigCommand::Get { key } => {
            let store = crate::context::store()?;
            let value = store.read_config()?.get(&key)?;
            out.value(
                &json!({"key": key, "value": value}),
                |v| match &v["value"] {
                    Value::Null => "null".to_owned(),
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            );
            Ok(())
        }
        ConfigCommand::Unset { key } => {
            let mut patch = serde_json::Map::new();
            patch.insert(key, Value::Null);
            merge(g, out, &Value::Object(patch).to_string()).await
        }
        ConfigCommand::Telemetry { state } => {
            let on = state == OnOff::On;
            merge(g, out, &json!({"telemetry": on}).to_string()).await
        }
        ConfigCommand::Home { dir } => home(out, &dir),
    }
}

async fn merge(g: &Globals, out: Out, patch: &str) -> Result<()> {
    let mut patch = Config::parse_patch(patch)?;
    Config::default().merge(&patch)?;
    if let Some(Value::String(path)) = patch.get_mut("drawing") {
        crate::input::read_drawing(Path::new(path))?;
        crate::input::resolve(Path::new(path))?
            .to_str()
            .ok_or_else(|| Error::invalid_input("config `drawing` path must be UTF-8"))?
            .clone_into(path);
    }
    let store = crate::context::store()?;
    let config = store.merge_config(&Value::Object(patch).to_string())?;
    sync(g, &store, &config).await;
    out.value(&config.to_public_value(), kv);
    next(out, &["peek config show", "peek send --speak \"…\""]);
    Ok(())
}

/// The config as peekd mirrors it: `telemetry` is this home's *effective*
/// setting, so an opt-out in the environment (`PEEK_TELEMETRY`,
/// `SPACE_STATION_TELEMETRY`, `SILICON_TELEMETRY`) reaches peekd too.
/// `--no-telemetry` stays with this one process.
pub fn sync_payload(config: &Config) -> ConfigSyncConfig {
    let mut payload = config.sync_payload();
    if payload.telemetry && env_opt_out() {
        payload.telemetry = false;
        payload.env_opt_out = true;
    }
    payload
}

/// [`sync_payload`] for a peekd that answered `hello`: `shown` is dropped
/// from `notify` when that peekd does not announce `notify_shown` (it would
/// refuse the unknown value).
pub fn sync_payload_for(config: &Config, hello: &HelloResult) -> ConfigSyncConfig {
    let mut payload = sync_payload(config);
    if !hello.has_feature(features::NOTIFY_SHOWN) {
        payload.notify.retain(|n| *n != Notify::Shown);
    }
    payload
}

/// Pushes the non-secret config to a running peekd (never starts it; skipped
/// when this home has no session, because every peekd op is authenticated).
async fn sync(g: &Globals, store: &Store, config: &Config) {
    let task = async {
        let session = g.session(store.clone(), false).await.ok()?;
        let file = store.read_session().ok()?;
        file.usable_slot(&session.slot_key, store.dir()).ok()?;
        let auth = auth_block(store, &session.slot_key).ok()?;
        let mut s = service::connect_existing(Duration::from_millis(500))
            .await
            .ok()??;
        let payload = sync_payload_for(config, &s.hello);
        s.call(
            &ConfigSync { config: payload },
            Some(&auth),
            Vec::new(),
            Duration::from_secs(2),
        )
        .await
        .ok()
    };
    let _ = tokio::time::timeout(Duration::from_secs(4), task).await;
}

const MOVABLE: [&str; 4] = [
    SESSION_FILE,
    CONFIG_FILE,
    DAEMON_TOKEN_FILE,
    "feature-consent.json",
];

/// `peek config home <DIR>`: moves the store and writes the pointer.
fn home(out: Out, dir: &Path) -> Result<()> {
    let home = silicon_home(std::env::var_os("SILICON_HOME").as_deref())?;
    let target = std::fs::canonicalize(crate::input::resolve(dir)?).map_err(|e| {
        Error::invalid_input(format!(
            "{} is not an existing directory: {e}",
            dir.display()
        ))
        .with_hint("create the directory first; the store becomes <DIR>/.peek")
    })?;
    if !target.is_dir() {
        return Err(Error::invalid_input(format!(
            "{} is not a directory",
            target.display()
        )));
    }
    let current = crate::context::store()?;
    let default_dir = home.join(STORE_DIR);
    let new_dir = target.join(STORE_DIR);
    let pointer = default_dir.join(HOME_POINTER_FILE);
    if new_dir == current.dir() {
        let value = json!({"home": target.display().to_string(), "store": new_dir.display().to_string(),
            "pointer": (target != home).then(|| pointer.display().to_string()), "moved": []});
        out.value(&value, |_| {
            format!("the store is already {}", new_dir.display())
        });
        return Ok(());
    }
    if !crate::context::default_profile()
        || current.path("profiles").exists()
        || new_dir.join("profiles").exists()
    {
        return Err(Error::invalid_input(
            "home relocation cannot move a store with named profiles",
        )
        .with_hint(
            "keep this home, or choose a separate SILICON_HOME before logging into new profiles",
        ));
    }
    let lock = current.lock()?;
    let destination = Store::open(&new_dir)?;
    let mut to_move = Vec::new();
    for name in MOVABLE {
        let Some(bytes) = read_private(&current.path(name))? else {
            continue;
        };
        match read_private(&destination.path(name))? {
            Some(existing) if existing != bytes => {
                return Err(Error::new(
                    ErrorCode::InvalidInput,
                    format!(
                        "{} already holds a different {name}; nothing was changed",
                        destination.dir().display()
                    ),
                )
                .with_hint(format!(
                    "pick a directory without a peek store, or inspect and remove {} first",
                    destination.path(name).display()
                )));
            }
            Some(_) => {}
            None => to_move.push((name, bytes)),
        }
    }
    for (name, bytes) in &to_move {
        write_atomic(destination.dir(), name, bytes)?;
    }
    if target == home {
        remove_file(&pointer)?;
    } else {
        set_home_pointer(&home, &target)?;
    }
    for (name, _) in &to_move {
        remove_file(&current.path(name))?;
    }
    drop(lock);
    let moved: Vec<&str> = to_move.iter().map(|(n, _)| *n).collect();
    let value = json!({
        "home": target.display().to_string(),
        "store": destination.dir().display().to_string(),
        "pointer": (target != home).then(|| pointer.display().to_string()),
        "moved": moved,
        "previous_store": current.dir().display().to_string(),
    });
    out.value(&value, |v| {
        format!(
            "this home's peek store is now {}{}",
            v["store"].as_str().unwrap_or_default(),
            if moved.is_empty() {
                String::new()
            } else {
                format!(" (moved {})", moved.join(", "))
            }
        )
    });
    next(out, &["peek login status --json"]);
    Ok(())
}
