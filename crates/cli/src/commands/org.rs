//! `peek org byo deepgram set|show|delete` (BLUEPRINT §5.2): the org's own
//! legacy Deepgram key, unused by current speech. The backend requires `org_role ∈ {owner, admin}` for writes
//! and never returns the key.

use serde_json::{Value, json};
use silicon_peek_client::{
    Error, Result,
    api::{ByoDeepgramRequest, ByoStatus},
};
use url::Url;

use super::{bearer, next};
use crate::{
    cli::{ByoCommand, DeepgramCommand, OrgCommand},
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
            "org {} has a legacy Deepgram key (unused by current speech; base URL {}, updated {})",
            v["org_id"].as_str().unwrap_or_default(),
            v["base_url"].as_str().unwrap_or("https://api.deepgram.com"),
            v["updated_at"].as_str().unwrap_or("?")
        )
    } else {
        format!(
            "org {} has no legacy Deepgram key; speech uses Google TTS and OpenAI transcription",
            v["org_id"].as_str().unwrap_or_default()
        )
    }
}

fn with_org(status: &ByoStatus, org: &str) -> Value {
    let mut v = serde_json::to_value(status).unwrap_or(Value::Null);
    v["org_id"] = json!(org);
    v
}

pub async fn run(g: &Globals, out: Out, command: OrgCommand) -> Result<()> {
    let OrgCommand::Byo {
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
            let (status, org) = bearer(g, &session, |client, org| {
                let request = request.clone();
                async move {
                    client
                        .set_byo_deepgram(&org, &request)
                        .await
                        .map(|s| (s, org))
                }
            })
            .await?;
            out.value(&with_org(&status, org.as_str()), human);
            next(out, &["peek org byo deepgram show --json"]);
        }
        DeepgramCommand::Show => {
            let session = g.session(crate::context::store()?, false).await?;
            let (status, org) = bearer(g, &session, |client, org| async move {
                client.byo_deepgram(&org).await.map(|s| (s, org))
            })
            .await?;
            out.value(&with_org(&status, org.as_str()), human);
        }
        DeepgramCommand::Delete => {
            let session = g.session(crate::context::store()?, false).await?;
            let org = bearer(g, &session, |client, org| async move {
                client.delete_byo_deepgram(&org).await.map(|()| org)
            })
            .await?;
            out.value(
                &json!({"configured": false, "org_id": org, "updated_at": null, "base_url": null}),
                human,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        assert_eq!(
            check_base_url("https://api.eu.deepgram.com/")
                .ok()
                .as_deref(),
            Some("https://api.eu.deepgram.com")
        );
        assert!(check_base_url("http://api.deepgram.com").is_err());
        assert!(check_base_url("http://127.0.0.1:9").is_err());
        assert!(check_base_url("https://127.0.0.1:9").is_err());
        assert!(check_base_url("https://localhost:8443").is_err());
        assert!(check_base_url("https://u:p@api.deepgram.com").is_err());
        assert!(check_base_url("nope").is_err());
    }
}
