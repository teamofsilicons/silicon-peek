//! `peek accounts [--json]` (BLUEPRINT §7.3): static discovery. No network, no
//! session and no side effects (it never creates or chmods the store);
//! Stemcell calls it on every candidate binary. With `--test` it adds the
//! environment's name and generation from `GET /api/v1/accounts`.

use serde_json::Value;
use silicon_peek_client::{
    Result,
    api::AccountsInfo,
    config::Config,
    identity::ApiUrl,
    runtime::{fs::read_private, store::CONFIG_FILE},
};

use crate::{context::Globals, output::Out};

/// The config's `api_url`, read without opening (creating, tightening) the
/// store. Any problem means "no override": accounts must answer regardless.
fn config_api() -> Option<ApiUrl> {
    let dir = crate::context::store_dir().ok()?;
    let bytes = read_private(&dir.join(CONFIG_FILE)).ok()??;
    let value = silicon_peek_client::json::parse_value(&bytes).ok()?;
    let (config, _) = Config::from_stored(value.as_object()?);
    config.api_url
}

pub async fn run(g: &Globals, out: Out) -> Result<()> {
    let api = g
        .explicit_api()?
        .or_else(config_api)
        .unwrap_or_else(ApiUrl::production);
    let value = serde_json::to_value(AccountsInfo::new(&api)).unwrap_or(Value::Null);
    out.value(&value, human);
    Ok(())
}

fn human(v: &Value) -> String {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    let list = |k: &str| {
        v["platforms"][k]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let out = format!(
        "{} {} — ACCOUNTS app id `{}` (developer {})\n\
         API:       {}\n\
         ACCOUNTS:       {}\n\
         Consent:   {}\n\
         Login:     {}\n\
         Install:   {}\n\
         Docs:      {}\n\
         Source:    {}\n\
         Rust:      {} (CLI: {})\n\
         Platforms: full {}; ACCOUNTS only {}",
        s("name"),
        s("version"),
        s("app_id"),
        s("owner_id"),
        s("api_url"),
        s("accounts_url"),
        s("auth_url"),
        s("login"),
        s("install"),
        s("docs_url"),
        s("repository_url"),
        s("rust_package"),
        s("cli_package"),
        list("full"),
        list("accounts_only"),
    );
    out
}
