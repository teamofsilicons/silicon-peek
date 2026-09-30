//! Loopback Deepgram Voice Agent with real WebSocket framing.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::{
    net::{TcpListener, TcpStream},
    task::AbortHandle,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};

/// Signed with a test-only HS256 key; the mock provider checks the exact bearer value.
pub const JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjQxMDI0NDQ4MDAsInN1YiI6InRlc3QtYWdlbnQifQ.3yRl8rqFPjGBwWEuyiFe37yuJ7vMZemIUzT4bPHEUR0";

#[derive(Clone)]
pub enum ResponsePlan {
    Audio { bytes: Vec<u8>, done: bool },
    Batch { bytes: Vec<u8>, messages: usize },
    Reject(u16),
    Pause(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Record {
    pub authorization: String,
    pub settings: Value,
    pub injection: Value,
}

pub struct Agent {
    pub url: String,
    pub records: Arc<Mutex<Vec<Record>>>,
    pub connections: Arc<AtomicUsize>,
    pub disconnected: Arc<AtomicUsize>,
    plans: Arc<Mutex<Vec<ResponsePlan>>>,
    task: AbortHandle,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Agent {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/agent/converse", listener.local_addr().unwrap());
        let records = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let disconnected = Arc::new(AtomicUsize::new(0));
        let plans = Arc::new(Mutex::new(vec![ResponsePlan::Reject(403)]));
        let (seen, count, ended, responses) = (
            records.clone(),
            connections.clone(),
            disconnected.clone(),
            plans.clone(),
        );
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let plan = {
                    let mut plans = responses.lock().unwrap();
                    if plans.len() > 1 {
                        plans.remove(0)
                    } else {
                        plans[0].clone()
                    }
                };
                tokio::spawn(exchange(socket, plan, seen.clone(), ended.clone()));
            }
        })
        .abort_handle();
        Self {
            url,
            records,
            connections,
            disconnected,
            plans,
            task,
        }
    }

    pub fn respond(&self, plans: Vec<ResponsePlan>) {
        assert!(!plans.is_empty());
        *self.plans.lock().unwrap() = plans;
    }

    pub fn audio(&self, bytes: Vec<u8>) {
        self.respond(vec![ResponsePlan::Audio { bytes, done: true }]);
    }

    pub fn count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

// tungstenite fixes the handshake error type to an HTTP response.
#[allow(clippy::result_large_err)]
async fn exchange(
    socket: TcpStream,
    plan: ResponsePlan,
    seen: Arc<Mutex<Vec<Record>>>,
    disconnected: Arc<AtomicUsize>,
) {
    let reject = if let ResponsePlan::Reject(status) = plan {
        Some(status)
    } else {
        None
    };
    let ws = accept_hdr_async(socket, move |req: &Request, response: Response| {
        assert_eq!(req.uri().path(), "/v1/agent/converse");
        assert_eq!(
            req.headers()["authorization"].to_str().unwrap(),
            format!("Bearer {JWT}")
        );
        match reject {
            Some(status) => Err(tokio_tungstenite::tungstenite::http::Response::builder()
                .status(status)
                .body(Some("private provider details".to_owned()))
                .unwrap()),
            None => Ok(response),
        }
    })
    .await;
    let Ok(mut ws) = ws else { return };
    if ws.send(event("Welcome")).await.is_err() {
        return;
    }
    let Some(Ok(settings)) = ws.next().await else {
        return;
    };
    let settings: Value = serde_json::from_str(settings.to_text().unwrap()).unwrap();
    if ws.send(event("SettingsApplied")).await.is_err() {
        return;
    }
    let (bytes, done, pause, messages) = match plan {
        ResponsePlan::Audio { bytes, done } => (bytes, done, false, 1),
        ResponsePlan::Batch { bytes, messages } => (bytes, true, false, messages),
        ResponsePlan::Pause(bytes) => (bytes, false, true, 1),
        ResponsePlan::Reject(_) => return,
    };
    for _ in 0..messages {
        let Some(Ok(injection)) = ws.next().await else {
            return;
        };
        let injection: Value = serde_json::from_str(injection.to_text().unwrap()).unwrap();
        let conversation =
            json!({"type":"ConversationText","role":"assistant","content":injection["message"]});
        seen.lock().unwrap().push(Record {
            authorization: format!("Bearer {JWT}"),
            settings: settings.clone(),
            injection,
        });
        if ws
            .send(Message::Text(conversation.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    for chunk in bytes.chunks(16_384) {
        if ws
            .send(Message::Binary(chunk.to_vec().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    if pause {
        while let Some(Ok(message)) = ws.next().await {
            if message.is_close() {
                break;
            }
        }
        disconnected.fetch_add(1, Ordering::SeqCst);
    } else if done {
        let _ = ws.send(event("AgentAudioDone")).await;
    }
}

fn event(kind: &str) -> Message {
    Message::Text(json!({"type":kind}).to_string().into())
}
