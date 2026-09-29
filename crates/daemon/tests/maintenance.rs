//! Maintenance paths with every external program injected: Peek.app
//! self-update (`update_once`), the stale-CLI watchdog, and the telemetry
//! relay through the backend gateway.

// Each test is one end-to-end scenario, asserted step by step.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use common::{Harness, eventually};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use silicon_peek_client::{
    api::TelemetryEvent,
    ipc::{
        cli::{AppOffer, AppOfferInfo, ConfigSync, ConfigSyncConfig, Telemetry},
        ui::{SettingsChanged, UiTelemetry},
    },
};
use silicon_peek_daemon::{
    commands::{CommandFuture, CommandOutput, CommandRunner, CommandSpec},
    update::UpdateOutcome,
};
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

/// Records every command; fakes `ditto` by writing a bundle whose
/// Info.plist carries `new_build`.
#[derive(Debug)]
struct FakeCommands {
    calls: Mutex<Vec<CommandSpec>>,
    codesign_ok: Mutex<bool>,
    new_build: Mutex<u64>,
}

impl FakeCommands {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            codesign_ok: Mutex::new(true),
            new_build: Mutex::new(1001),
        })
    }

    fn names(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(CommandSpec::name)
            .collect()
    }

    fn find(&self, name: &str) -> Option<CommandSpec> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.name() == name)
            .cloned()
    }
}

fn info_plist(bundle: &str, build: u64) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>{bundle}</string>
  <key>CFBundleShortVersionString</key><string>0.1.{}</string>
  <key>CFBundleVersion</key><string>{build}</string>
</dict></plist>
"#,
        build.saturating_sub(1000)
    )
}

fn write_app(dir: &Path, build: u64) {
    let contents = dir.join("Peek.app/Contents");
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::write(
        contents.join("Info.plist"),
        info_plist("ai.tos.peek", build),
    )
    .unwrap();
}

impl CommandRunner for FakeCommands {
    fn run(&self, spec: CommandSpec) -> CommandFuture<'_> {
        self.calls.lock().unwrap().push(spec.clone());
        let out = match spec.name().as_str() {
            "ditto" => {
                let stage = PathBuf::from(spec.args.last().unwrap());
                write_app(&stage, *self.new_build.lock().unwrap());
                CommandOutput::ok()
            }
            "codesign" if !*self.codesign_ok.lock().unwrap() => {
                CommandOutput::failed(3, "stage/Peek.app: code object is not signed at all")
            }
            _ => CommandOutput::ok(),
        };
        Box::pin(async move { Ok(out) })
    }
}

fn offer_zip(dir: &Path, build: u64, bundle: &str) -> AppOffer {
    let zip = dir.join(format!("Peek-{build}.app.zip"));
    std::fs::write(&zip, format!("zip bytes of build {build}")).unwrap();
    let sha = hex::encode(Sha256::digest(std::fs::read(&zip).unwrap()));
    AppOffer {
        zip_path: zip.to_string_lossy().into_owned(),
        info: AppOfferInfo {
            bundle_id: bundle.into(),
            bundle_version: build,
            short_version: format!("0.1.{}", build.saturating_sub(1000)),
            team_id: "LTBSK59BJ2".into(),
            zip_sha256: sha,
            minimum_system_version: "26.0".into(),
        },
    }
}

fn installed_build(h: &Harness) -> String {
    std::fs::read_to_string(h.cfg.app_path().join("Contents/Info.plist")).unwrap()
}

#[tokio::test]
async fn update_once_swaps_the_newest_verified_build_and_exits() {
    let fake = FakeCommands::new();
    let runner: Arc<dyn CommandRunner> = fake.clone();
    let h = Harness::start_with(move |c| c.commands = runner).await;
    write_app(&h.cfg.applications_dir, 1000);
    let ui = h.ui().await;
    let home = h.home("si:cleanup");
    let offers = h.root.path().join("pkg");
    std::fs::create_dir_all(&offers).unwrap();

    // A downgrade and a dev build are never taken.
    let old = offer_zip(&offers, 999, "ai.tos.peek");
    let (r, _) = h.call(&home, &old, vec![]).await.unwrap();
    assert!(!r.scheduled);
    assert_eq!(r.installed_build, 1000);
    let dev = offer_zip(&offers, 1500, "ai.tos.peek.dev");
    let e = h.call(&home, &dev, vec![]).await.err().unwrap();
    assert!(e.message().contains("updates `ai.tos.peek` only"));
    assert_eq!(
        h.handle().update_now().await.unwrap(),
        UpdateOutcome::UpToDate { installed: 1000 }
    );

    // A tampered offer (hash mismatch) is refused.
    let mut bad = offer_zip(&offers, 1001, "ai.tos.peek");
    bad.info.zip_sha256 = "a".repeat(64);
    assert!(h.call(&home, &bad, vec![]).await.is_err());

    // A malformed hash is invalid_input: it used to panic peekd (slicing
    // the first 12 bytes), and release builds abort on panic.
    for sha in [
        "abc".to_owned(),
        format!("{}é{}", "a".repeat(11), "b".repeat(51)),
    ] {
        let mut malformed = offer_zip(&offers, 1001, "ai.tos.peek");
        malformed.info.zip_sha256 = sha;
        let e = h.call(&home, &malformed, vec![]).await.err().unwrap();
        assert_eq!(e.code().as_str(), "invalid_input", "{}", e.message());
        assert!(e.message().contains("64 hex digits"), "{}", e.message());
    }

    // The real offer.
    let offer = offer_zip(&offers, 1001, "ai.tos.peek");
    let (r, _) = h.call(&home, &offer, vec![]).await.unwrap();
    assert!(r.scheduled);
    let stored = h.cfg.support_dir.join(format!(
        "offers/1001-{}.app.zip",
        &offer.info.zip_sha256[..12]
    ));
    assert!(stored.is_file());

    let out = h.handle().update_now().await.unwrap();
    assert_eq!(
        out,
        UpdateOutcome::Applied {
            from: 1000,
            to: 1001
        }
    );
    assert!(
        installed_build(&h).contains("<string>1001</string>"),
        "swapped into place"
    );
    assert!(
        !h.cfg
            .applications_dir
            .join(format!(".Peek.app.update.{}", std::process::id()))
            .exists(),
        "the stage (now the old bundle) is removed"
    );
    let applied: Value = serde_json::from_slice(
        &std::fs::read(h.cfg.support_dir.join("update-applied.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(applied["from"], 1000);
    assert_eq!(applied["to"], 1001);
    assert_eq!(fake.names(), vec!["ditto", "codesign", "xattr", "open"]);
    let codesign = fake.find("codesign").unwrap();
    let req = codesign
        .args
        .iter()
        .find(|a| a.to_string_lossy().starts_with("-R="))
        .unwrap();
    assert!(
        req.to_string_lossy()
            .contains("certificate leaf[subject.OU] = \"LTBSK59BJ2\"")
    );
    assert!(req.to_string_lossy().contains("identifier \"ai.tos.peek\""));
    let open = fake.find("open").unwrap();
    let args: Vec<String> = open
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[..2], ["-g", "-j"]);
    assert_eq!(args[args.len() - 2..], ["--after-update", "1000"]);
    let _ = ui.expect("restarting").await;
    let ops: Vec<String> = ui
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|(o, _)| o.clone())
        .collect();
    // The idle wait, the re-check under the install lock right before the
    // swap (an ask may have arrived meanwhile), then the quit.
    assert_eq!(
        ops,
        vec!["app.update.prepare", "app.update.prepare", "app.quit"]
    );
    tokio::time::timeout(Duration::from_secs(2), h.handle().exit_requested())
        .await
        .expect("peekd asks to exit after swapping");
}

#[tokio::test]
async fn failed_verification_rejects_and_a_busy_ui_defers() {
    let fake = FakeCommands::new();
    let runner: Arc<dyn CommandRunner> = fake.clone();
    let h = Harness::start_with(move |c| c.commands = runner).await;
    write_app(&h.cfg.applications_dir, 1000);
    let home = h.home("si:cleanup");
    let dir = h.root.path().join("pkg");
    std::fs::create_dir_all(&dir).unwrap();
    let offer = offer_zip(&dir, 1002, "ai.tos.peek");
    h.call(&home, &offer, vec![]).await.unwrap();

    *fake.codesign_ok.lock().unwrap() = false;
    let out = h.handle().update_now().await.unwrap();
    let UpdateOutcome::Rejected { build, reason } = out else {
        panic!("expected a rejection, got {out:?}")
    };
    assert_eq!(build, 1002);
    assert!(reason.contains("code signature"));
    let rejected: Value =
        serde_json::from_slice(&std::fs::read(h.cfg.support_dir.join("rejected.json")).unwrap())
            .unwrap();
    assert!(rejected[&offer.info.zip_sha256].is_string());
    assert!(
        installed_build(&h).contains("<string>1000</string>"),
        "the running build stays"
    );
    assert_eq!(
        h.handle().update_now().await.unwrap(),
        UpdateOutcome::UpToDate { installed: 1000 }
    );

    // A newer, valid build waits while the UI is busy.
    *fake.codesign_ok.lock().unwrap() = true;
    *fake.new_build.lock().unwrap() = 1003;
    let ui = h.ui().await;
    *ui.ready.lock().unwrap() = false;
    h.call(&home, &offer_zip(&dir, 1003, "ai.tos.peek"), vec![])
        .await
        .unwrap();
    assert_eq!(
        h.handle().update_now().await.unwrap(),
        UpdateOutcome::Deferred { build: 1003 }
    );
    assert!(installed_build(&h).contains("<string>1000</string>"));

    // The UI was idle for app.update.prepare but is busy by app.quit (an ask
    // arrived meanwhile): the update backs off instead of quitting (and
    // SIGTERMing) it, and nothing is swapped.
    *ui.ready.lock().unwrap() = true;
    *ui.quit_ready.lock().unwrap() = Some(false);
    assert_eq!(
        h.handle().update_now().await.unwrap(),
        UpdateOutcome::Deferred { build: 1003 }
    );
    assert!(installed_build(&h).contains("<string>1000</string>"));
    assert!(
        !h.cfg
            .applications_dir
            .join(format!(".Peek.app.update.{}", std::process::id()))
            .exists(),
        "the stage is removed when the update backs off"
    );
    let ops: Vec<String> = ui
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|(o, _)| o.clone())
        .collect();
    // prepare (idle wait), prepare again under the lock, then the declined quit.
    assert_eq!(
        ops[ops.len() - 3..],
        ["app.update.prepare", "app.update.prepare", "app.quit"]
    );
    // New bubbles are shown again once the update backed off.
    h.register(&home, 1, &ui).await;
    let send = silicon_peek_client::ipc::cli::SendOp {
        isi: Some("deliberate".into()),
        speak: None,
        show: Some(
            serde_json::from_value(json!({"elements":[{"type":"text","text":"after"}]})).unwrap(),
        ),
        ask: None,
        voice: None,
        voice_instructions: None,
        lang: None,
        notify: vec![],
        duration_ms: None,
        expires_in_s: None,
        wait: false,
        expires_at: None,
        due_at: None,
        tz: None,
        replace: false,
    };
    h.call(&home, &send, vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
}

#[tokio::test]
async fn the_cli_watchdog_updates_lagging_registries_once_an_hour() {
    let fake = FakeCommands::new();
    let runner: Arc<dyn CommandRunner> = fake.clone();
    let h = Harness::start_with(move |c| {
        c.commands = runner;
        c.timings.watchdog_behind = Duration::ZERO;
        c.honeycomb.env_override = Some(PathBuf::from("/opt/fake/honeycomb"));
    })
    .await;
    let registry = |home: &Path, fp: &str, version: &str| {
        let ctx = home.join(".honeycomb/dir/contexts").join(fp);
        std::fs::create_dir_all(&ctx).unwrap();
        std::fs::write(
            ctx.join("installed.json"),
            json!({"peek": {"app_id":"peek","version":version,"directory":ctx.join("pkg"),"channel":"prod"}}).to_string(),
        )
        .unwrap();
    };
    // The Carbon's own registry lags; a Silicon's package registry is current.
    registry(&h.cfg.real_home, "c0d9", "0.0.9");
    let home = h.home("si:cleanup");
    registry(
        &home.dir.join(".silicon/packages"),
        "a1b2",
        silicon_peek_client::VERSION,
    );
    let _ = h
        .call(&home, &silicon_peek_client::ipc::cli::StatusOp {}, vec![])
        .await
        .unwrap();

    assert_eq!(h.handle().watchdog_now().await.unwrap(), 1);
    let hc = fake.find("honeycomb").unwrap();
    assert_eq!(hc.program, PathBuf::from("/opt/fake/honeycomb"));
    let args: Vec<String> = hc
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args, ["update", "peek", "--json"]);
    let env: Vec<(String, String)> = hc
        .env
        .iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert!(env.contains(&("HONEYCOMB_NO_SERVICE".into(), "1".into())));
    assert!(env.contains(&("HONEYCOMB_NO_MODIFY_PATH".into(), "1".into())));
    assert!(env.contains(&(
        "SILICON_HOME".into(),
        h.cfg.real_home.to_string_lossy().into_owned()
    )));
    // Not again within the hour.
    assert_eq!(h.handle().watchdog_now().await.unwrap(), 0);
    // Turned off in Settings.
    let ui = h.ui().await;
    ui.request(
        &SettingsChanged {
            key: "updates.cli_watchdog".into(),
            value: json!(false),
        },
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(h.handle().watchdog_now().await.unwrap(), 0);
}

fn event(id: &str, name: &str) -> TelemetryEvent {
    TelemetryEvent {
        id: id.into(),
        event_type: name.into(),
        data: json!({"schema_version":1,"source":"cli","event":name,"environment":"production"}),
        metadata: json!({"occurred_at":"2026-09-26T10:00:00Z"}),
    }
}

#[tokio::test]
async fn a_homes_opt_out_covers_peekds_own_events_and_backend_calls() {
    let h = Harness::start_with_server(|c, uri| {
        c.telemetry_enabled = true;
        c.telemetry_api = silicon_peek_client::identity::ApiUrl::parse(uri).unwrap();
    })
    .await;
    let ui = h.ui().await;
    // `peek config telemetry off` in one Silicon's home (config.json).
    let quiet = h.home("si:quiet");
    {
        let lock = quiet.store.lock().unwrap();
        let mut cfg = quiet.store.read_config().unwrap();
        cfg.telemetry = false;
        quiet.store.write_config(&lock, &cfg).unwrap();
    }
    let loud = h.home("si:loud");
    let show = || silicon_peek_client::ipc::cli::SendOp {
        isi: Some("deliberate".into()),
        speak: None,
        show: Some(
            serde_json::from_value(json!({"elements":[{"type":"text","text":"hi"}]})).unwrap(),
        ),
        ask: None,
        voice: None,
        voice_instructions: None,
        lang: None,
        notify: vec![],
        duration_ms: None,
        expires_in_s: None,
        wait: false,
        expires_at: None,
        due_at: None,
        tz: None,
        replace: false,
    };
    h.register(&quiet, 1, &ui).await;
    h.call(&quiet, &show(), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;
    h.register(&loud, 2, &ui).await;
    h.call(&loud, &show(), vec![]).await.unwrap();
    let _ = ui.expect("peek.show").await;

    let db = h.db();
    let events = |db: &rusqlite::Connection| -> Vec<Value> {
        let mut st = db
            .prepare("SELECT event FROM telemetry_outbox WHERE source = 'daemon'")
            .unwrap();
        st.query_map([], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .map(|b| serde_json::from_slice(&b.unwrap()).unwrap())
            .collect()
    };
    let hash_of = |actor: &str| {
        silicon_peek_client::telemetry::actor_hash(
            &silicon_peek_client::identity::OrgId::parse("tos").unwrap(),
            &silicon_peek_client::identity::ActorId::parse(actor).unwrap(),
        )
    };
    eventually(5, "the opted-in Silicon's send.displayed", || {
        events(&db).iter().any(|e| {
            e["type"] == "send.displayed" && e["data"]["actor"]["hash"] == hash_of("si:loud")
        })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let all = events(&db);
    assert!(
        !all.iter()
            .any(|e| e["data"]["actor"]["hash"] == hash_of("si:quiet")),
        "no event about the opted-out Silicon: {all:?}"
    );
    let ipc = all.iter().filter(|e| e["type"] == "ipc.request").count();
    // register side + register drawing + send from the opted-in home only.
    assert_eq!(ipc, 3, "{all:?}");

    // peekd's backend calls for the opted-out home say so.
    let mut puts: Vec<wiremock::Request> = Vec::new();
    for _ in 0..100 {
        puts = h
            .server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.method.as_str() == "PUT" && r.url.path() == "/api/v1/drawings/current")
            .collect();
        if puts.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let header_for = |token: &str| {
        puts.iter()
            .find(|r| {
                r.headers
                    .get("authorization")
                    .is_some_and(|v| v.to_str().unwrap_or_default().contains(token))
            })
            .map(|r| {
                r.headers
                    .get("x-peek-telemetry")
                    .map(|v| v.to_str().unwrap().to_owned())
            })
    };
    assert_eq!(header_for("oat_siquiet"), Some(Some("off".to_owned())));
    assert_eq!(header_for("oat_siloud"), Some(Some("on".to_owned())));
}

#[tokio::test]
async fn an_environment_opt_out_mirrored_by_config_sync_lasts_until_the_cli_sends_telemetry() {
    let h = Harness::start().await;
    let ui = h.ui().await;
    // config.json allows telemetry, but the CLI's environment opts out
    // (SPACE_STATION_TELEMETRY=0): it forwards that with config.sync.
    let home = h.home("si:envquiet");
    let sync = ConfigSync {
        config: ConfigSyncConfig {
            voice: None,
            voice_instructions: None,
            language: None,
            notify: vec![],
            telemetry: false,
            env_opt_out: true,
        },
    };
    h.call(&home, &sync, vec![]).await.unwrap();
    h.register(&home, 3, &ui).await;
    let puts = |n: usize| {
        let server = &h.server;
        async move {
            for _ in 0..100 {
                let found: Vec<wiremock::Request> = server
                    .received_requests()
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|r| {
                        r.method.as_str() == "PUT" && r.url.path() == "/api/v1/drawings/current"
                    })
                    .collect();
                if found.len() >= n {
                    return found;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("fewer than {n} drawing uploads arrived");
        }
    };
    let header = |r: &wiremock::Request| {
        r.headers
            .get("x-peek-telemetry")
            .map(|v| v.to_str().unwrap().to_owned())
    };
    let first = puts(1).await;
    assert_eq!(header(&first[0]).as_deref(), Some("off"));

    // A later run of the home without the opt-out hands peekd telemetry:
    // the mirrored opt-out is over (that batch still followed it).
    h.call(
        &home,
        &Telemetry {
            events: vec![event("e1", "command.finished")],
        },
        vec![],
    )
    .await
    .unwrap();
    let db = h.db();
    let mirror: String = common::query_one(
        &db,
        "SELECT config FROM homes WHERE home_path = ?1",
        &[&home.auth.home],
    )
    .unwrap();
    let mirror: Value = serde_json::from_str(&mirror).unwrap();
    assert_eq!(mirror["telemetry"], true);
    assert!(mirror.get("env_opt_out").is_none());
    let left: i64 = common::query_one(
        &db,
        "SELECT count(*) FROM telemetry_outbox WHERE source = 'cli'",
        &[],
    )
    .unwrap();
    assert_eq!(left, 0, "e1 arrived under the opt-out");
    h.call(
        &home,
        &silicon_peek_client::ipc::cli::RegisterDrawing {
            filename: "logo.js".into(),
            check_only: false,
            preview: false,
            dump_frame: None,
        },
        vec![b"export default function draw(ctx) { ctx.fillStyle = 'red'; ctx.fillRect(0,0,9,9); }\n".to_vec()],
    )
    .await
    .unwrap();
    let both = puts(2).await;
    assert_eq!(header(&both[1]).as_deref(), Some("on"));
}

#[tokio::test]
async fn telemetry_is_relayed_through_the_gateway_and_honours_opt_outs() {
    let h = Harness::start_with_server(|c, uri| {
        c.telemetry_enabled = true;
        c.telemetry_api = silicon_peek_client::identity::ApiUrl::parse(uri).unwrap();
    })
    .await;
    Mock::given(method("POST"))
        .and(path("/api/web/telemetry"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&h.server)
        .await;
    let home = h.home("si:cleanup");
    h.call(
        &home,
        &Telemetry {
            events: vec![
                event("e1", "command.finished"),
                event("e2", "command.finished"),
            ],
        },
        vec![],
    )
    .await
    .unwrap();
    let ui = h.ui().await;
    ui.request(
        &UiTelemetry {
            events: vec![event("m1", "glass_mode"), event("m2", "mic_pressed")],
        },
        vec![],
    )
    .await
    .unwrap();
    // The home opts out: its later events are dropped.
    h.call(
        &home,
        &ConfigSync {
            config: ConfigSyncConfig {
                voice: None,
                voice_instructions: None,
                language: None,
                notify: vec![],
                telemetry: false,
                env_opt_out: false,
            },
        },
        vec![],
    )
    .await
    .unwrap();
    h.call(
        &home,
        &Telemetry {
            events: vec![event("e3", "command.finished")],
        },
        vec![],
    )
    .await
    .unwrap();

    let db = h.db();
    eventually(5, "daemon events", || {
        common::query_one::<i64>(
            &db,
            "SELECT count(*) FROM telemetry_outbox WHERE source = 'daemon'",
            &[],
        )
        .unwrap_or(0)
            > 0
    })
    .await;
    let sent = h.handle().relay_telemetry_now().await.unwrap();
    assert!(sent >= 4);
    let reqs: Vec<wiremock::Request> = h
        .server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/api/web/telemetry")
        .collect();
    let mut by_source: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    for r in &reqs {
        let source = r
            .headers
            .get("x-peek-source")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        by_source
            .entry(source)
            .or_default()
            .push(serde_json::from_slice(&r.body).unwrap());
    }
    let cli = &by_source["cli"][0];
    assert_eq!(cli["table"], "peekclidaemon");
    let ids: Vec<&str> = cli["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["e1", "e2"], "e3 was dropped by the home's opt-out");
    let mac_tables: Vec<&str> = by_source["mac"]
        .iter()
        .map(|b| b["table"].as_str().unwrap())
        .collect();
    assert!(
        mac_tables.contains(&"peekfrontendanalytics") && mac_tables.contains(&"peekfrontendevents")
    );
    let daemon = &by_source["daemon"][0];
    assert_eq!(daemon["table"], "peekclidaemon");
    let names: Vec<&str> = daemon["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"daemon.started"));
    for e in daemon["events"].as_array().unwrap() {
        assert_eq!(e["data"]["service"], "peek-daemon");
        assert!(
            !e.to_string().contains("si:cleanup"),
            "raw actor ids are never recorded"
        );
    }
    let left: i64 = common::query_one(&db, "SELECT count(*) FROM telemetry_outbox", &[]).unwrap();
    assert_eq!(left, 0);

    // Settings → off clears the outbox and stops recording.
    h.call(
        &home,
        &Telemetry {
            events: vec![event("e4", "x")],
        },
        vec![],
    )
    .await
    .unwrap();
    ui.request(
        &SettingsChanged {
            key: "telemetry".into(),
            value: json!(false),
        },
        vec![],
    )
    .await
    .unwrap();
    let left: i64 = common::query_one(&db, "SELECT count(*) FROM telemetry_outbox", &[]).unwrap();
    assert_eq!(left, 0);
    let settings: Value =
        serde_json::from_slice(&std::fs::read(h.cfg.support_dir.join("settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings["telemetry"], false);
    let e = ui
        .request(
            &SettingsChanged {
                key: "bogus".into(),
                value: json!(1),
            },
            vec![],
        )
        .await
        .err()
        .unwrap();
    assert!(e.message().contains("not a Peek setting"));
}

#[tokio::test]
async fn a_newer_bundled_app_in_hello_becomes_an_offer() {
    use silicon_peek_client::{
        ipc::cli::{BundledApp, CliHello, Hello},
        runtime::daemon::DaemonConnection,
    };
    let h = Harness::start().await;
    write_app(&h.cfg.applications_dir, 1000);
    let pkg = h.root.path().join("packages/peek/0.1.7-abc");
    std::fs::create_dir_all(&pkg).unwrap();
    let offer = offer_zip(&pkg, 1007, "ai.tos.peek");
    let zip = pkg.join("Peek.app.zip");
    std::fs::rename(&offer.zip_path, &zip).unwrap();
    let info = format!(
        "bundle_id=ai.tos.peek\nbundle_version=1007\nshort_version=0.1.7\nteam_id=LTBSK59BJ2\nzip_sha256={}\nminimum_system_version=26.0\n",
        offer.info.zip_sha256
    );
    std::fs::write(pkg.join("Peek.app.info"), info).unwrap();
    let mut c = DaemonConnection::connect(h.handle().socket_path(), Duration::from_secs(2))
        .await
        .unwrap();
    let hello = Hello::Cli(CliHello {
        cli_version: "0.1.7".into(),
        protocols: vec![1],
        platform: "macos-aarch64".into(),
        bundled_app: Some(BundledApp {
            build: 1007,
            short_version: "0.1.7".into(),
            zip_path: zip.to_string_lossy().into_owned(),
            zip_sha256: offer.info.zip_sha256.clone(),
        }),
    });
    let r = c.hello(&hello, Duration::from_secs(2)).await.unwrap();
    assert_eq!(r.app.unwrap().build, 1000);
    let stored = h.cfg.support_dir.join(format!(
        "offers/1007-{}.app.info",
        &offer.info.zip_sha256[..12]
    ));
    eventually(5, "the stored offer", || stored.is_file()).await;
}
