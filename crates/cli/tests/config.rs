//! `peek config` (BLUEPRINT §7.3): the strict `config set` merge Stemcell
//! calls, plus show/get/unset/telemetry and `config home`.

mod common;

use common::{DEAD_API, Env};
use serde_json::json;

const VALID: [&str; 8] = [
    "telemetry",
    "voice",
    "language",
    "notify",
    "api_url",
    "delivery_max_age_hours",
    "position",
    "drawing",
];

#[tokio::test]
async fn config_set_is_strict() {
    let env = Env::new(DEAD_API);
    for (input, code) in [
        ("[1,2]", "invalid_input"),
        ("\"telemetry\"", "invalid_input"),
        ("{nope", "invalid_json"),
        (r#"{"telemetry":true,"telemetry":false}"#, "invalid_json"),
        (r#"{"colour":"red"}"#, "unknown_config_key"),
        (r#"{"telemetry":"no"}"#, "invalid_input"),
        (r#"{"voice":"thalia"}"#, "invalid_input"),
        (r#"{"notify":["everything"]}"#, "invalid_input"),
        (r#"{"api_url":"http://example.com"}"#, "invalid_input"),
        (r#"{"delivery_max_age_hours":169}"#, "invalid_input"),
    ] {
        let run = env.run(&["config", "set", input, "--json"]).await;
        assert_eq!(run.code, 2, "{input}: {}", run.stderr);
        assert!(run.stdout.is_empty());
        let e = run.error();
        assert_eq!(e["code"], code, "{input}");
        if matches!(code, "invalid_json" | "unknown_config_key") || input.starts_with('[') {
            assert_eq!(e["details"]["valid_keys"], json!(VALID), "{input}");
        }
    }
    assert!(
        !env.store_dir().join("config.json").exists(),
        "a refused patch writes nothing"
    );
}

#[tokio::test]
async fn config_set_merges_and_prints_the_result() {
    let env = Env::new(DEAD_API);
    let run = env
        .run(&[
            "config",
            "set",
            r#"{"notify":["show_dismissed","speech_finished"],"voice":"aura-2-thalia-en","language":"EN","delivery_max_age_hours":24}"#,
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let show = env.run(&["config", "show", "--json"]).await.json();
    assert_eq!(
        show,
        json!({"schema":1,"telemetry":true,"voice":"aura-2-thalia-en","language":"en",
               "notify":["speech_finished","show_dismissed"],"api_url":null,"delivery_max_age_hours":24,"position":null,"drawing":null})
    );
    let reset = env
        .run(&[
            "config",
            "set",
            r#"{"voice":null,"delivery_max_age_hours":null}"#,
            "--json",
        ])
        .await;
    assert_eq!(reset.code, 0);
    assert!(reset.json()["voice"].is_null());
    assert_eq!(reset.json()["delivery_max_age_hours"], 168);
    let get = env.run(&["config", "get", "notify", "--json"]).await;
    assert_eq!(
        get.json(),
        json!({"key":"notify","value":["speech_finished","show_dismissed"]})
    );
    let unset = env.run(&["config", "unset", "notify", "--json"]).await;
    assert_eq!(unset.json()["notify"], json!([]));
    let off = env.run(&["config", "telemetry", "off", "--json"]).await;
    assert_eq!(off.json()["telemetry"], false);
    let bad = env.run(&["config", "get", "colour", "--json"]).await;
    assert_eq!(bad.code, 2);
    assert_eq!(bad.error()["code"], "unknown_config_key");
}

#[tokio::test]
async fn config_home_moves_the_store() {
    let env = Env::new(DEAD_API);
    let set = env.run(&["config", "set", r#"{"telemetry":false}"#]).await;
    assert_eq!(set.code, 0);
    let target = env
        .dir
        .path()
        .canonicalize()
        .unwrap_or_default()
        .join("elsewhere");
    std::fs::create_dir(&target).unwrap_or_else(|e| panic!("{e}"));
    let run = env
        .run(&[
            "config",
            "home",
            target.to_str().unwrap_or_default(),
            "--json",
        ])
        .await;
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v = run.json();
    assert_eq!(v["store"], target.join(".peek").display().to_string());
    assert_eq!(v["moved"], json!(["config.json"]));
    assert!(target.join(".peek/config.json").is_file());
    assert!(!env.store_dir().join("config.json").exists());
    assert!(
        env.store_dir().join("home").is_file(),
        "pointer in the default store"
    );
    let show = env.run(&["config", "show", "--json"]).await;
    assert_eq!(
        show.json()["telemetry"],
        false,
        "later runs follow the pointer"
    );
    let back = env
        .run(&[
            "config",
            "home",
            env.home.to_str().unwrap_or_default(),
            "--json",
        ])
        .await;
    assert_eq!(back.code, 0, "{}", back.stderr);
    assert!(env.store_dir().join("config.json").is_file());
    assert!(!env.store_dir().join("home").exists(), "pointer removed");
}

#[tokio::test]
#[allow(clippy::expect_used)] // fixtures must be valid
async fn visual_defaults_are_persistent_absolute_and_atomic() {
    let env = Env::new(DEAD_API);
    std::fs::write(env.home.join("logo.js"), "peek.onFrame = () => false;").expect("drawing");
    let set = env
        .run(&[
            "config",
            "set",
            r#"{"position":3,"drawing":"./logo.js"}"#,
            "--json",
        ])
        .await;
    assert_eq!(set.code, 0, "{}", set.stderr);
    assert_eq!(set.json()["position"], 3);
    assert_eq!(
        set.json()["drawing"],
        env.home.join("logo.js").to_str().expect("path")
    );
    for patch in [
        r#"{"position":9}"#,
        r#"{"position":5,"drawing":"./missing.js"}"#,
        r#"{"drawing":false}"#,
    ] {
        let bad = env.run(&["config", "set", patch, "--json"]).await;
        assert_eq!(bad.code, 2, "{}", bad.stderr);
        let shown = env.run(&["config", "show", "--json"]).await;
        assert_eq!(shown.json(), set.json(), "invalid config is atomic");
    }
    let reset = env
        .run(&[
            "config",
            "set",
            r#"{"position":null,"drawing":null}"#,
            "--json",
        ])
        .await;
    assert_eq!(reset.code, 0, "{}", reset.stderr);
    assert!(reset.json()["position"].is_null());
    assert!(reset.json()["drawing"].is_null());
}
