//! Local UI permission controls for explicit saved login contexts.
use crate::{homes::home_id, net::HomeRef, state::Shared};
use serde::{Deserialize, Serialize};
use silicon_peek_client::{
    Error, ErrorCode, Result,
    identity::{Actor, SlotKey},
    ipc::Op,
    runtime::{
        CLI_MARGIN, RefreshPolicy, Store,
        authorization::{self, Action},
    },
};

#[derive(Deserialize, Serialize)]
pub(crate) struct PermissionAction {
    pub home_id: String,
    pub context_id: String,
    pub action: String,
}
impl Op for PermissionAction {
    const NAME: &'static str = "permissions.ting";
    type Output = PermissionResult;
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PermissionResult {
    pub context_id: String,
    pub enrolled: bool,
}
#[derive(Serialize)]
struct PublicContext {
    home_id: String,
    context_id: String,
    actor: Actor,
    account_id: String,
    api_url: String,
    context: String,
    label: String,
}
impl Shared {
    pub(crate) async fn permission_contexts(&self) -> Result<serde_json::Value> {
        let mut contexts = Vec::new();
        for home in self.known_homes().await? {
            let Ok(store) = Store::open_existing(std::path::Path::new(&home.home_path)) else {
                continue;
            };
            let Ok(file) = store.read_session() else {
                continue;
            };
            for raw in file.slots.keys() {
                let Ok(key) = SlotKey::parse(raw) else {
                    continue;
                };
                let Ok(slot) = file.usable_slot(&key, store.dir()) else {
                    continue;
                };
                let profile = store
                    .dir()
                    .parent()
                    .filter(|parent| parent.file_name().is_some_and(|name| name == "profiles"))
                    .and_then(|_| store.dir().file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or("default");
                contexts.push(PublicContext {
                    home_id: home_id(&home.home_path),
                    context_id: slot.context_id()?.into(),
                    actor: slot.actor.clone(),
                    account_id: slot.account_id.to_string(),
                    api_url: key.api_url().to_string(),
                    context: key.context().to_string(),
                    label: format!(
                        "{} · {} · {} · {}",
                        profile,
                        slot.actor.public_id,
                        slot.account_id,
                        key.context()
                    ),
                });
            }
        }
        Ok(serde_json::json!({"contexts":contexts}))
    }
    pub(crate) async fn ting_permission(
        &self,
        request: PermissionAction,
    ) -> Result<PermissionResult> {
        if !matches!(request.action.as_str(), "status" | "enroll") {
            return Err(Error::invalid_input("choose status or enroll"));
        }
        let home = self
            .known_homes()
            .await?
            .into_iter()
            .find(|home| home_id(&home.home_path) == request.home_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::NotLoggedIn,
                    "the selected saved account is unavailable",
                )
            })?;
        let store = Store::open_existing(std::path::Path::new(&home.home_path))?;
        let file = store.read_session()?;
        let mut selected = None;
        for raw in file.slots.keys() {
            let Ok(key) = SlotKey::parse(raw) else {
                continue;
            };
            if file
                .usable_slot(&key, store.dir())
                .is_ok_and(|slot| slot.context_id().ok() == Some(request.context_id.as_str()))
            {
                selected = Some(key);
                break;
            }
        }
        let key = selected.ok_or_else(|| {
            Error::new(
                ErrorCode::SessionRejected,
                "the selected login context changed; choose the account again",
            )
        })?;
        let home_ref = HomeRef {
            home_path: home.home_path,
            api_url: key.api_url().clone(),
            context: key.context(),
        };
        if request.action == "enroll" {
            let client = self
                .net
                .session(&home_ref, CLI_MARGIN, &RefreshPolicy::single_attempt())
                .await?
                .0;
            authorization::perform(
                &store,
                &client,
                &key,
                &request.context_id,
                Action::Enroll(None),
            )
            .await?;
        }
        let file = store.read_session()?;
        let slot = file.usable_slot(&key, store.dir())?;
        if slot.context_id()? != request.context_id {
            return Err(Error::new(
                ErrorCode::SessionRejected,
                "the selected login changed",
            ));
        }
        let enrolled = slot.ting.as_ref().is_some_and(|ting| ting.subscribed);
        if request.action == "enroll" && enrolled {
            let actor_key = crate::state::ActorKey {
                context: key.context(),
                account: slot.account_id.clone(),
                actor: slot.actor.public_id.clone(),
            };
            self.wake_authority_rows(&actor_key, key.api_url(), true)
                .await?;
        }
        Ok(PermissionResult {
            context_id: request.context_id,
            enrolled,
        })
    }
}
