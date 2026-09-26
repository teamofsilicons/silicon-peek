//! `PUT/GET/DELETE /api/v1/drawings/current`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::body::Body;
use common::*;
use sha2::{Digest, Sha256};

const SCRIPT: &str = "export default function draw(ctx, input) { ctx.circle(0, 0, 10); }";

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn put(body: Vec<u8>, sha256: Option<&str>) -> axum::http::Request<Body> {
    let mut b = authed("PUT", "/api/v1/drawings/current", ACCESS)
        .header("content-type", "application/javascript");
    if let Some(s) = sha256 {
        b = b.header("x-peek-drawing-sha256", s);
    }
    b.body(Body::from(body)).unwrap()
}

#[tokio::test]
async fn put_get_delete_round_trip() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    let digest = sha(SCRIPT.as_bytes());
    let r = h.send(put(SCRIPT.as_bytes().to_vec(), Some(&digest))).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["sha256"], digest);
    assert_eq!(r.json()["bytes"], SCRIPT.len());
    assert!(r.json()["updated_at"].as_str().unwrap().ends_with('Z'));

    let r = h
        .send(
            authed("GET", "/api/v1/drawings/current", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, SCRIPT.as_bytes());
    assert_eq!(r.header("etag"), Some(format!("\"{digest}\"")));
    assert_eq!(
        r.header("content-type").as_deref(),
        Some("application/javascript; charset=utf-8")
    );

    let r = h
        .send(
            authed("GET", "/api/v1/drawings/current", ACCESS)
                .header("if-none-match", format!("\"{digest}\""))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 304);

    let r = h
        .send(
            authed("DELETE", "/api/v1/drawings/current", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 204);
    let r = h
        .send(
            authed("GET", "/api/v1/drawings/current", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 404);
    assert_eq!(r.code(), "drawing_not_found");
    let r = h
        .send(
            authed("DELETE", "/api/v1/drawings/current", ACCESS)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 204, "idempotent");
    let events = h.events();
    let put_event = events.iter().find(|e| e["event"] == "drawing.put").unwrap();
    assert_eq!(put_event["context"]["drawing_sha256"], digest);
    assert!(
        !put_event.to_string().contains("circle"),
        "drawing source is never recorded"
    );
}

#[tokio::test]
async fn uploads_are_checked() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    let r = h.send(put(SCRIPT.as_bytes().to_vec(), None)).await;
    assert_eq!(r.status, 400, "the hash header is required");

    let r = h
        .send(put(SCRIPT.as_bytes().to_vec(), Some(&sha(b"other"))))
        .await;
    assert_eq!(r.status, 400);
    assert!(
        r.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("hashes to")
    );

    let big = vec![b'a'; 256 * 1024 + 1];
    let r = h.send(put(big.clone(), Some(&sha(&big)))).await;
    assert_eq!(r.status, 413);
    assert_eq!(r.code(), "drawing_too_large");

    let exact = vec![b'a'; 256 * 1024];
    let r = h.send(put(exact.clone(), Some(&sha(&exact)))).await;
    assert_eq!(r.status, 200, "exactly 256 KiB is accepted");

    let r = h
        .send(put(vec![0xff, 0xfe], Some(&sha(&[0xff, 0xfe]))))
        .await;
    assert_eq!(r.status, 400, "not UTF-8");

    let r = h
        .send(
            authed("PUT", "/api/v1/drawings/current", ACCESS)
                .header("content-type", "image/png")
                .header("x-peek-drawing-sha256", sha(b"x"))
                .body(Body::from("x"))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 415);
}

#[tokio::test]
async fn drawings_are_per_actor() {
    let h = Harness::start().await;
    mount_silicon(&h.iam, ACCESS).await;
    mount_introspect(
        &h.iam,
        "oat_otherSilicon01",
        introspection("si:other", ORG, &FULL_SCOPES, None, None),
    )
    .await;
    let digest = sha(SCRIPT.as_bytes());
    assert_eq!(
        h.send(put(SCRIPT.as_bytes().to_vec(), Some(&digest)))
            .await
            .status,
        200
    );
    let r = h
        .send(
            authed("GET", "/api/v1/drawings/current", "oat_otherSilicon01")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 404);
}
