//! IPC framing over real Unix socket pairs (sync and tokio), with blobs split
//! across writes (BLUEPRINT §1.6).

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write as _;

use serde_json::{Map, Value, json};
use silicon_peek_client::ipc::{
    Message, Request,
    cli::RegisterDrawing,
    frame::{
        AsyncFrameReader, Frame, FrameError, FrameLimits, FrameReader, encode, write_frame,
        write_frame_async,
    },
};

fn header(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

#[test]
fn sync_socket_pair_with_byte_by_byte_writes() {
    let (mut a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let script = b"export default (ctx, input) => { ctx.fillRect(0, 0, 100, 100) }".to_vec();
    let req = Request::new(
        &RegisterDrawing {
            filename: "logo.js".into(),
            check_only: false,
            preview: true,
            dump_frame: None,
        },
        None,
        vec![script.clone()],
    )
    .unwrap();
    let frame = Message::Request(req.clone()).into_frame().unwrap();
    let wire = encode(&frame, &FrameLimits::V1).unwrap();
    let writer = std::thread::spawn(move || {
        for byte in wire {
            a.write_all(&[byte]).unwrap();
        }
    });
    let mut r = FrameReader::new(b);
    let got = r.read_frame().unwrap().expect("frame");
    writer.join().unwrap();
    assert_eq!(got.blobs, vec![script]);
    let Message::Request(back) = Message::from_frame(got).unwrap() else {
        panic!("not a request");
    };
    assert_eq!(back, req);
    assert!(
        r.read_frame().unwrap().is_none(),
        "clean EOF after the writer closed"
    );
}

#[test]
fn a_peer_that_dies_mid_blob_is_an_error() {
    let (mut a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    a.write_all(b"{\"v\":1,\"event\":\"tts.chunk\",\"bin\":[10]}\n12345")
        .unwrap();
    drop(a);
    let mut r = FrameReader::new(b);
    assert!(matches!(r.read_frame(), Err(FrameError::UnexpectedEof)));
}

#[test]
fn oversize_declarations_are_refused_without_reading_the_blob() {
    let (mut a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let limits = FrameLimits {
        max_blob: 1024,
        ..FrameLimits::V1
    };
    a.write_all(b"{\"v\":1,\"bin\":[1025]}\n").unwrap();
    let mut r = FrameReader::with_limits(b, limits);
    assert!(matches!(r.read_frame(), Err(FrameError::TooLarge { .. })));
    let mut sink = Vec::new();
    assert!(matches!(
        write_frame(
            &mut sink,
            &Frame {
                header: Map::new(),
                blobs: vec![vec![0; 1025]]
            },
            &limits
        ),
        Err(FrameError::TooLarge { .. })
    ));
}

#[tokio::test]
async fn tokio_socket_pair_streams_many_frames() {
    let (a, b) = tokio::net::UnixStream::pair().unwrap();
    let (_ra, mut wa) = a.into_split();
    let frames: Vec<Frame> = (0..50u64)
        .map(|seq| Frame {
            header: header(json!({"v":1,"event":"tts.chunk","send_id":"snd_x","seq":seq})),
            blobs: vec![vec![
                u8::try_from(seq).unwrap();
                4096 + usize::try_from(seq).unwrap()
            ]],
        })
        .collect();
    let expected = frames.clone();
    let writer = tokio::spawn(async move {
        for f in &frames {
            write_frame_async(&mut wa, f, &FrameLimits::V1)
                .await
                .unwrap();
        }
    });
    let mut r = AsyncFrameReader::new(b);
    for want in expected {
        assert_eq!(r.read_frame().await.unwrap(), Some(want));
    }
    writer.await.unwrap();
}
