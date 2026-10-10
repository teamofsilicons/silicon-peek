//! `peek account byo deepgram set|show|delete` (BLUEPRINT §5.2): the account's own
//! legacy Deepgram key, unused by current speech. The backend requires `account_role ∈ {owner, admin}` for writes
//! and never returns the key.

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, Result,
    api::{ByoDeepgramRequest, ByoStatus},
};
use url::Url;

use super::{bearer, next};
use crate::{
    cli::{AccountCommand, ByoCommand, DeepgramCommand},
    context::Globals,
    input,
    output::Out,
};

fn check_base_url(raw: &str) -> Result<String> {
    let url = Url::parse(raw).map_err(|e| {
        Error::invalid_input(format!("--base-url `{raw}` is not a URL: {e}"))
            .with_hint("for example https://api.eu.deepgram.com")
    })?;
    // The legacy key validator calls this origin, so it is always https
    // and a host name (the server also checks its allowlist).
    if url.scheme() != "https" {
        return Err(Error::invalid_input(format!(
            "--base-url `{raw}` must use https"
        )));
    }
    if !matches!(url.host(), Some(url::Host::Domain(d)) if d != "localhost" && !d.ends_with(".localhost"))
    {
        return Err(Error::invalid_input(format!(
            "--base-url `{raw}` must name a public host (not an IP address or localhost)"
        ))
        .with_hint("for example https://api.eu.deepgram.com"));
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
        return Err(Error::invalid_input(format!(
            "--base-url `{raw}` must not contain credentials or a query"
        )));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn human(v: &Value) -> String {
    if v["configured"] == true {
        format!(
            "account {} has a legacy Deepgram key (unused by current speech; base URL {}, updated {})",
            v["account_id"].as_str().unwrap_or_default(),
            v["base_url"].as_str().unwrap_or("https://api.deepgram.com"),
            v["updated_at"].as_str().unwrap_or("?")
        )
    } else {
        format!(
            "account {} has no legacy Deepgram key; speech uses ElevenLabs v4 TTS through Deepgram and OpenAI transcription",
            v["account_id"].as_str().unwrap_or_default()
        )
    }
}

fn with_account(status: &ByoStatus, account: &str) -> Value {
    let mut v = serde_json::to_value(status).unwrap_or(Value::Null);
    v["account_id"] = json!(account);
    v
}

pub async fn run(g: &Globals, out: Out, command: AccountCommand) -> Result<()> {
    let AccountCommand::Byo {
        command: ByoCommand::Deepgram { command },
    } = command;
    match command {
        DeepgramCommand::Set { key_file, base_url } => {
            g.check_stdin(&[("--key-file", key_file == "-")])?;
            let key = input::read_secret("--key-file", &key_file)?;
            let base_url = base_url.as_deref().map(check_base_url).transpose()?;
            let session = g.session(crate::context::store()?, false).await?;
            let request = ByoDeepgramRequest {
                api_key: key,
                base_url,
            };
            let (status, account) = bearer(g, &session, |client, account| {
                let request = request.clone();
                async move {
                    client
                        .set_byo_deepgram(&account, &request)
                        .await
                        .map(|s| (s, account))
                }
            })
            .await?;
            out.value(&with_account(&status, account.as_str()), human);
            next(out, &["peek account byo deepgram show --json"]);
        }
        DeepgramCommand::Show => {
            let session = g.session(crate::context::store()?, false).await?;
            let (status, account) = bearer(g, &session, |client, account| async move {
                client.byo_deepgram(&account).await.map(|s| (s, account))
            })
            .await?;
            out.value(&with_account(&status, account.as_str()), human);
        }
        DeepgramCommand::Delete => {
            let session = g.session(crate::context::store()?, false).await?;
            let account = bearer(g, &session, |client, account| async move {
                client.delete_byo_deepgram(&account).await.map(|()| account)
            })
            .await?;
            out.value(
                &json!({"configured": false, "account_id": account, "updated_at": null, "base_url": null}),
                human,
            );
        }
    }
    Ok(())
}
