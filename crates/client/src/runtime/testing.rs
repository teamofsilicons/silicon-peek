//! `testing.json`: saved testing environments (BLUEPRINT §1.7, §2.9).
//!
//! ```json
//! {"schema":1,"selected":"<uuid>"|null,
//!  "environments":{"<uuid>":{"api_url","app_secret","name","generation"}}}
//! ```
//!
//! The app secret is peek's own test secret. It is never read from
//! `IAM_TEST_APP_SECRET` or `IAM_TEST_KEY`, which belong to Ting.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    error::{Error, ErrorCode, Result},
    identity::{ApiUrl, Context, TestingSecret},
};

/// The schema this build reads and writes.
pub const TESTING_SCHEMA: u32 = 1;

/// The whole file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestingFile {
    /// Always 1 for this build.
    pub schema: u32,
    /// The environment used when `--test` is not given (`null`: production).
    #[serde(default)]
    pub selected: Option<Uuid>,
    /// Saved environments by UUID.
    #[serde(default)]
    pub environments: BTreeMap<Uuid, SavedEnvironment>,
    /// Fields written by a newer peek.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for TestingFile {
    fn default() -> Self {
        Self {
            schema: TESTING_SCHEMA,
            selected: None,
            environments: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// One saved testing environment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedEnvironment {
    /// The peek-server origin that validated the secret.
    pub api_url: ApiUrl,
    /// peek's test app secret for this environment.
    pub app_secret: TestingSecret,
    /// The environment's display name.
    pub name: String,
    /// The generation discovered with the secret.
    pub generation: u64,
    /// Fields written by a newer peek.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl TestingFile {
    /// A saved environment.
    ///
    /// # Errors
    /// `testing_secret_invalid` when `id` was never saved.
    pub fn get(&self, id: Uuid) -> Result<&SavedEnvironment> {
        self.environments.get(&id).ok_or_else(|| {
            Error::new(
                ErrorCode::TestingSecretInvalid,
                format!("testing environment {id} is not saved in this home"),
            )
            .with_hint(format!(
                "pass its peek test app secret once: printf %s \"$SECRET\" | peek --test {id} --app-secret-file - login <SLT>"
            ))
        })
    }

    /// Saves (or updates) an environment.
    pub fn save(&mut self, id: Uuid, env: SavedEnvironment) {
        self.environments.insert(id, env);
    }

    /// The context selected by `--test` (explicit) or `selected` (default).
    #[must_use]
    pub fn context(&self, explicit: Option<Uuid>) -> Context {
        match explicit.or(self.selected) {
            Some(id) => Context::Testing(id),
            None => Context::Production,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    #[test]
    fn round_trip_and_lookup() -> Result<()> {
        let id = Uuid::now_v7();
        let secret = format!("ask_{}", "b".repeat(43));
        let v = serde_json::json!({"schema":1,"selected":id,"environments":{id.to_string():{
            "api_url":"http://127.0.0.1:9","app_secret":secret,"name":"peek testing","generation":3}}});
        let f: TestingFile = json::from_value(v, "testing.json")?;
        assert_eq!(f.get(id)?.generation, 3);
        assert_eq!(f.context(None), Context::Testing(id));
        assert_eq!(f.context(Some(id)), Context::Testing(id));
        assert!(f.get(Uuid::now_v7()).is_err());
        assert_eq!(TestingFile::default().context(None), Context::Production);
        let bad = serde_json::json!({"schema":1,"environments":{id.to_string():{
            "api_url":"http://127.0.0.1:9","app_secret":"nope","name":"x","generation":1}}});
        assert!(json::from_value::<TestingFile>(bad, "testing.json").is_err());
        Ok(())
    }
}
