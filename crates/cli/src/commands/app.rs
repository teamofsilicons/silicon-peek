//! `peek app status|install|update|uninstall` and `peek daemon
//! status|restart` (BLUEPRINT §4.2, §4.3, §4.6). On Linux and Windows
//! `peek app status` answers `{"supported":false,"platform":…}` (exit 0) and
//! every other command here is `platform_unsupported` (exit 4).

use serde_json::{Value, json};
use silicon_peek_client::{Result, platform};

use crate::{
    cli::{AppCommand, DaemonCommand},
    context::Globals,
    output::Out,
    service::require_mac,
};

pub async fn run(g: &Globals, out: Out, command: AppCommand) -> Result<()> {
    match command {
        AppCommand::Status => {
            if !cfg!(target_os = "macos") {
                out.value(&json!({"supported": false, "platform": platform()}), |v| {
                    format!(
                        "Peek.app is not supported on {}",
                        v["platform"].as_str().unwrap_or_default()
                    )
                });
                return Ok(());
            }
            mac::status(out).await
        }
        AppCommand::Install => {
            require_mac("app install")?;
            mac::install(out).await
        }
        AppCommand::Update => {
            require_mac("app update")?;
            mac::update(g, out).await
        }
        AppCommand::Uninstall => {
            require_mac("app uninstall")?;
            mac::uninstall(g, out).await
        }
    }
}

pub async fn daemon(_g: &Globals, out: Out, command: DaemonCommand) -> Result<()> {
    match command {
        DaemonCommand::Status => {
            require_mac("daemon status")?;
            mac::daemon_status(out).await
        }
        DaemonCommand::Restart => {
            require_mac("daemon restart")?;
            mac::daemon_restart(out).await
        }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn human_daemon(v: &Value) -> String {
    if v["running"] != true {
        return format!(
            "peekd is not running (socket {})",
            v["socket"].as_str().unwrap_or_default()
        );
    }
    format!(
        "peekd {} running (pid {}, protocol {}) at {}\nPeek.app UI: {}\nattached homes: {}{}",
        v["version"].as_str().unwrap_or_default(),
        v["pid"],
        v["protocol"],
        v["socket"].as_str().unwrap_or_default(),
        if v["ui"]["running"] == true {
            format!("running (build {})", v["ui"]["build"])
        } else {
            "not running".to_owned()
        },
        v["homes"],
        v["via"]
            .as_str()
            .map(|via| format!("\nrestarted via {via}"))
            .unwrap_or_default()
    )
}

#[cfg(target_os = "macos")]
mod mac {
    use std::time::Duration;

    use serde_json::json;
    use silicon_peek_client::{
        Result,
        ipc::cli::{AppOffer, DaemonStatus},
        runtime::{
            auth_block,
            daemon::{REQUEST_TIMEOUT, socket_path},
        },
    };

    use super::human_daemon;
    use crate::{
        commands::{mac_session, next},
        context::Globals,
        output::{Out, kv},
        service,
    };

    pub async fn status(out: Out) -> Result<()> {
        let value = service::app_status().await?;
        out.value(&value, kv);
        if value["installed"] != true {
            next(out, &["peek app install"]);
        }
        Ok(())
    }

    pub async fn install(out: Out) -> Result<()> {
        let paths = service::paths()?;
        let installed = service::ensure_app(&paths).await?;
        let svc = service::ensure_service().await?;
        let (build, short, _) = service::bundle_info(&paths.app).await;
        let value = json!({
            "installed": true,
            "path": installed.path,
            "installed_now": installed.installed_now,
            "offered_build": installed.offered_build,
            "build": build,
            "short_version": short,
            "running": {"daemon": true, "ui": svc.hello.app.as_ref().is_some_and(|a| a.ui_running)},
            "peekd_version": svc.hello.peekd_version,
        });
        out.value(&value, |v| {
            format!(
                "Peek.app {} (build {}) is {} at {} and peekd {} is running",
                v["short_version"].as_str().unwrap_or("?"),
                v["build"],
                if v["installed_now"] == true {
                    "installed"
                } else {
                    "present"
                },
                v["path"].as_str().unwrap_or_default(),
                v["peekd_version"].as_str().unwrap_or("?"),
            )
        });
        next(
            out,
            &["peek login status --json", "peek register side <1-8>"],
        );
        Ok(())
    }

    pub async fn update(g: &Globals, out: Out) -> Result<()> {
        let (_session, auth) = mac_session(g).await?;
        let paths = service::paths()?;
        let mut svc = service::ensure_service().await?;
        let installed = svc.hello.app.as_ref().map(|a| a.build);
        let Some((build, zip, info)) = service::best_offer(&paths) else {
            out.value(
                &json!({"scheduled": false, "installed_build": installed, "offered_build": null,
                        "reason": "no Peek.app build is offered: this peek has no bundled Peek.app.zip and the offers directory is empty"}),
                kv,
            );
            return Ok(());
        };
        let (result, _) = svc
            .call(
                &AppOffer {
                    zip_path: zip.display().to_string(),
                    info,
                },
                Some(&auth),
                Vec::new(),
                REQUEST_TIMEOUT,
            )
            .await?;
        let value = json!({"scheduled": result.scheduled, "installed_build": result.installed_build,
                           "offered_build": build});
        out.value(&value, |v| {
            if v["scheduled"] == true {
                format!(
                    "build {} is scheduled (installed {}); Peek.app swaps when nothing is on screen",
                    v["offered_build"], v["installed_build"]
                )
            } else {
                format!(
                    "nothing to update: installed build {} is not older than the best offer {}",
                    v["installed_build"], v["offered_build"]
                )
            }
        });
        Ok(())
    }

    pub async fn uninstall(g: &Globals, out: Out) -> Result<()> {
        // Authenticate when this home is logged in; peekd may accept the
        // request without it, and the fallback needs none.
        let auth = match crate::context::existing_store() {
            Ok(Some(store)) => match g.session(store, false).await {
                Ok(s) => auth_block(&s.store, &s.slot_key).ok(),
                Err(_) => None,
            },
            _ => None,
        };
        let value = service::uninstall(auth.as_ref()).await?;
        out.value(&value, |v| {
            if v["was_installed"] == true {
                format!(
                    "Peek.app was moved to the Trash and its login item and helper were unregistered ({})",
                    v["path"].as_str().unwrap_or_default()
                )
            } else {
                format!(
                    "Peek.app is not installed at {}",
                    v["path"].as_str().unwrap_or_default()
                )
            }
        });
        out.hint("`apps uninstall 'peek'` removes the CLI itself");
        Ok(())
    }

    pub async fn daemon_status(out: Out) -> Result<()> {
        if let Some(mut svc) = service::connect_existing(Duration::from_secs(2)).await? {
            let (status, _) = svc
                .call(&DaemonStatus {}, None, Vec::new(), REQUEST_TIMEOUT)
                .await?;
            out.result(&status, human_daemon);
        } else {
            out.value(
                &json!({"running": false, "socket": socket_path().display().to_string(),
                        "next": "peek app install"}),
                human_daemon,
            );
            next(out, &["peek app install"]);
        }
        Ok(())
    }

    pub async fn daemon_restart(out: Out) -> Result<()> {
        let (mut svc, via) = service::restart().await?;
        let (status, _) = svc
            .call(&DaemonStatus {}, None, Vec::new(), REQUEST_TIMEOUT)
            .await?;
        let mut value = serde_json::to_value(&status).unwrap_or_default();
        value["restarted"] = json!(true);
        value["via"] = json!(via);
        out.value(&value, human_daemon);
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod mac {
    //! Unreachable off macOS: every caller checks `require_mac` first. The
    //! signatures mirror the macOS implementations, hence the allow.
    #![allow(clippy::unused_async)]

    use silicon_peek_client::Result;

    use crate::{context::Globals, output::Out, service::require_mac};

    pub async fn status(_out: Out) -> Result<()> {
        require_mac("app status")
    }
    pub async fn install(_out: Out) -> Result<()> {
        require_mac("app install")
    }
    pub async fn update(_g: &Globals, _out: Out) -> Result<()> {
        require_mac("app update")
    }
    pub async fn uninstall(_g: &Globals, _out: Out) -> Result<()> {
        require_mac("app uninstall")
    }
    pub async fn daemon_status(_out: Out) -> Result<()> {
        require_mac("daemon status")
    }
    pub async fn daemon_restart(_out: Out) -> Result<()> {
        require_mac("daemon restart")
    }
}
