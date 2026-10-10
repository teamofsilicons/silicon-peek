//! IPC protocol v1 between the CLI, peekd and Peek.app (BLUEPRINT §1.6).
//!
//! Three message kinds share one socket and one [framing](frame):
//!
//! ```jsonc
//! {"v":1,"id":"<uuid>","op":"<name>", ...fields}                  // request
//! {"v":1,"id":"<uuid>","ok":true,"result":{...}}                  // reply
//! {"v":1,"id":"<uuid>","ok":false,"error":{"code","message",…}}   // error reply
//! {"v":1,"event":"<name>", ...fields}                             // event (no id)
//! ```
//!
//! Unknown ops are refused by name (`unknown_op`). Unknown fields are ignored
//! and additive fields are allowed, so typed bodies here do not use
//! `deny_unknown_fields`; duplicate keys are always refused by the framing.
//!
//! Typed bodies implement [`Op`] (requests, with their result type) or
//! [`EventBody`]; [`Request::new`] / [`Request::parse`] and friends convert.

pub mod cli;
pub mod frame;
pub mod ui;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::{
    Secret,
    error::{Error, ErrorCode, ErrorObject, Origin, Result},
    identity::{ApiUrl, Context},
    ids::request_id,
};
use frame::Frame;

/// The envelope version (`"v"`).
pub const ENVELOPE_VERSION: u64 = 1;

/// Protocol majors this build speaks, newest last.
pub const SUPPORTED_PROTOCOLS: &[u32] = &[1];

/// The newest protocol this build speaks.
pub const PROTOCOL: u32 = 1;

/// Picks the newest protocol both sides speak.
#[must_use]
pub fn negotiate(offered: &[u32]) -> Option<u32> {
    SUPPORTED_PROTOCOLS
        .iter()
        .rev()
        .copied()
        .find(|p| offered.contains(p))
}

/// A typed request body.
pub trait Op: Serialize + DeserializeOwned {
    /// The op name on the wire.
    const NAME: &'static str;
    /// The result type of a successful reply.
    type Output: Serialize + DeserializeOwned;
}

/// A typed event body.
pub trait EventBody: Serialize + DeserializeOwned {
    /// The event name on the wire.
    const NAME: &'static str;
}

/// An empty result or body (`{}`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// The CLI authentication block, required on every CLI op except `hello` and
/// `daemon.status`. peekd derives the actor and account from the verified store
/// slot, never from request fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthBlock {
    /// Absolute canonical store path (`$SILICON_HOME/.peek`).
    pub home: String,
    /// The 64-hex contents of `<home>/daemon-token`.
    pub home_token: Secret,
    /// The API origin of the session slot.
    pub api_url: ApiUrl,
    /// `production` or the testing environment UUID.
    pub context: Context,
    /// Stable login context; prevents a queued request adopting a replacement account.
    #[serde(default)]
    pub context_id: Option<String>,
}

/// A request.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// Correlates the reply.
    pub id: String,
    /// The op name.
    pub op: String,
    /// The CLI authentication block, if present.
    pub auth: Option<AuthBlock>,
    /// The remaining fields.
    pub fields: Map<String, Value>,
    /// Binary blobs.
    pub blobs: Vec<Vec<u8>>,
}

/// A reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The request's ID.
    pub id: String,
    /// The result object, or the error.
    pub outcome: std::result::Result<Value, ErrorObject>,
    /// Binary blobs (e.g. a PNG preview).
    pub blobs: Vec<Vec<u8>>,
}

/// An event.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The event name.
    pub event: String,
    /// The fields.
    pub fields: Map<String, Value>,
    /// Binary blobs (e.g. a TTS chunk).
    pub blobs: Vec<Vec<u8>>,
}

/// Any message.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// A request.
    Request(Request),
    /// A reply.
    Reply(Reply),
    /// An event.
    Event(Event),
}

fn protocol(msg: impl Into<String>) -> Error {
    Error::new(ErrorCode::ProtocolError, msg).with_hint(
        "the CLI, peekd and Peek.app disagree about the protocol; update with `apps update 'peek'`",
    )
}

fn to_object<T: Serialize>(value: &T, what: &str) -> Result<Map<String, Value>> {
    match serde_json::to_value(value) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(Error::internal(format!(
            "{what} must serialize to a JSON object"
        ))),
        Err(e) => Err(Error::internal(format!("serializing {what} failed: {e}"))),
    }
}

fn take_string(m: &mut Map<String, Value>, key: &str, what: &str) -> Result<String> {
    match m.remove(key) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s),
        Some(_) => Err(protocol(format!(
            "{what} `{key}` must be a non-empty string"
        ))),
        None => Err(protocol(format!("{what} has no `{key}`"))),
    }
}

impl Request {
    /// Builds a request with a fresh ID.
    ///
    /// # Errors
    /// `internal_error` if the body does not serialize to an object.
    pub fn new<O: Op>(op: &O, auth: Option<AuthBlock>, blobs: Vec<Vec<u8>>) -> Result<Self> {
        let fields = to_object(op, O::NAME)?;
        for reserved in ["v", "id", "op", "auth", "bin"] {
            if fields.contains_key(reserved) {
                return Err(Error::internal(format!(
                    "op {} must not define the reserved field `{reserved}`",
                    O::NAME
                )));
            }
        }
        Ok(Self {
            id: request_id(),
            op: O::NAME.to_owned(),
            auth,
            fields,
            blobs,
        })
    }

    /// Decodes the typed body.
    ///
    /// # Errors
    /// `unknown_op` when the name differs; `invalid_input` when the fields do
    /// not match.
    pub fn parse<O: Op>(&self) -> Result<O> {
        if self.op != O::NAME {
            return Err(Error::new(
                ErrorCode::UnknownOp,
                format!("expected op `{}`, got `{}`", O::NAME, self.op),
            ));
        }
        serde_json::from_value(Value::Object(self.fields.clone()))
            .map_err(|e| Error::invalid_input(format!("op `{}` has invalid fields: {e}", self.op)))
    }

    /// The auth block, or `invalid_input` naming the op.
    ///
    /// # Errors
    /// `invalid_input` when absent.
    pub fn require_auth(&self) -> Result<&AuthBlock> {
        self.auth.as_ref().ok_or_else(|| {
            Error::invalid_input(format!(
                "op `{}` requires the `auth` block (home, home_token, api_url, context)",
                self.op
            ))
        })
    }

    /// A success reply to this request.
    ///
    /// # Errors
    /// `internal_error` if the result does not serialize.
    pub fn reply<T: Serialize>(&self, result: &T, blobs: Vec<Vec<u8>>) -> Result<Reply> {
        Reply::ok(&self.id, result, blobs)
    }

    /// An error reply to this request.
    #[must_use]
    pub fn reply_err(&self, error: &Error) -> Reply {
        Reply::err(&self.id, error.to_object())
    }
}

impl Reply {
    /// A success reply.
    ///
    /// # Errors
    /// `internal_error` if the result does not serialize.
    pub fn ok<T: Serialize>(id: &str, result: &T, blobs: Vec<Vec<u8>>) -> Result<Self> {
        let value = serde_json::to_value(result)
            .map_err(|e| Error::internal(format!("serializing a reply failed: {e}")))?;
        Ok(Self {
            id: id.to_owned(),
            outcome: Ok(value),
            blobs,
        })
    }

    /// An error reply.
    #[must_use]
    pub fn err(id: &str, error: ErrorObject) -> Self {
        Self {
            id: id.to_owned(),
            outcome: Err(error),
            blobs: Vec::new(),
        }
    }

    /// Decodes a success result, or turns an error reply into an [`Error`]
    /// with [`Origin::Daemon`].
    ///
    /// # Errors
    /// The peer's error, or `unexpected_response` when the result does not
    /// match `T`.
    pub fn into_result<T: DeserializeOwned>(self) -> Result<(T, Vec<Vec<u8>>)> {
        match self.outcome {
            Ok(v) => serde_json::from_value(v)
                .map(|t| (t, self.blobs))
                .map_err(|e| {
                    Error::new(
                        ErrorCode::UnexpectedResponse,
                        format!("peekd's reply does not match this CLI's protocol types: {e}"),
                    )
                    .with_hint("update peek with `apps update 'peek'`; Peek.app updates itself")
                }),
            Err(e) => Err(Error::from_object(e, Origin::Daemon)),
        }
    }
}

impl Event {
    /// Builds an event.
    ///
    /// # Errors
    /// `internal_error` if the body does not serialize to an object.
    pub fn new<E: EventBody>(body: &E, blobs: Vec<Vec<u8>>) -> Result<Self> {
        let fields = to_object(body, E::NAME)?;
        Ok(Self {
            event: E::NAME.to_owned(),
            fields,
            blobs,
        })
    }

    /// Decodes the typed body.
    ///
    /// # Errors
    /// `protocol_error` when the name differs; `invalid_input` when the
    /// fields do not match.
    pub fn parse<E: EventBody>(&self) -> Result<E> {
        if self.event != E::NAME {
            return Err(protocol(format!(
                "expected event `{}`, got `{}`",
                E::NAME,
                self.event
            )));
        }
        serde_json::from_value(Value::Object(self.fields.clone())).map_err(|e| {
            Error::invalid_input(format!("event `{}` has invalid fields: {e}", self.event))
        })
    }
}

impl Message {
    /// Classifies a decoded frame: `op` → request, `ok` → reply, `event` →
    /// event. The envelope version must be 1.
    ///
    /// # Errors
    /// `protocol_error` for anything else.
    pub fn from_frame(frame: Frame) -> Result<Self> {
        let Frame { mut header, blobs } = frame;
        match header.remove("v") {
            Some(Value::Number(n)) if n.as_u64() == Some(ENVELOPE_VERSION) => {}
            Some(other) => {
                return Err(protocol(format!(
                    "envelope version {other} is not supported; this build speaks v={ENVELOPE_VERSION}"
                )));
            }
            None => return Err(protocol("the message has no envelope version `v`")),
        }
        if header.contains_key("op") {
            let id = take_string(&mut header, "id", "a request")?;
            let op = take_string(&mut header, "op", "a request")?;
            let auth = match header.remove("auth") {
                None | Some(Value::Null) => None,
                Some(v) => Some(serde_json::from_value(v).map_err(|e| {
                    Error::invalid_input(format!("the `auth` block is invalid: {e}"))
                })?),
            };
            return Ok(Self::Request(Request {
                id,
                op,
                auth,
                fields: header,
                blobs,
            }));
        }
        if let Some(ok) = header.remove("ok") {
            let id = take_string(&mut header, "id", "a reply")?;
            let outcome = match ok {
                Value::Bool(true) => {
                    Ok(header.remove("result").unwrap_or(Value::Object(Map::new())))
                }
                Value::Bool(false) => {
                    let e = header
                        .remove("error")
                        .ok_or_else(|| protocol("an error reply has no `error`"))?;
                    Err(serde_json::from_value(e).map_err(|e| {
                        protocol(format!("an error reply's `error` is invalid: {e}"))
                    })?)
                }
                _ => return Err(protocol("`ok` must be true or false")),
            };
            return Ok(Self::Reply(Reply { id, outcome, blobs }));
        }
        if header.contains_key("event") {
            let event = take_string(&mut header, "event", "an event")?;
            return Ok(Self::Event(Event {
                event,
                fields: header,
                blobs,
            }));
        }
        Err(protocol(
            "the message is neither a request (op), a reply (ok) nor an event (event)",
        ))
    }

    /// Encodes the message as a frame.
    ///
    /// # Errors
    /// `internal_error` if the auth block does not serialize.
    pub fn into_frame(self) -> Result<Frame> {
        let mut header = Map::new();
        header.insert("v".into(), Value::from(ENVELOPE_VERSION));
        let blobs = match self {
            Self::Request(r) => {
                header.extend(r.fields);
                header.insert("id".into(), Value::String(r.id));
                header.insert("op".into(), Value::String(r.op));
                if let Some(auth) = r.auth {
                    header.insert(
                        "auth".into(),
                        serde_json::to_value(auth).map_err(|e| {
                            Error::internal(format!("serializing the auth block failed: {e}"))
                        })?,
                    );
                }
                r.blobs
            }
            Self::Reply(r) => {
                header.insert("id".into(), Value::String(r.id));
                match r.outcome {
                    Ok(v) => {
                        header.insert("ok".into(), Value::Bool(true));
                        header.insert("result".into(), v);
                    }
                    Err(e) => {
                        header.insert("ok".into(), Value::Bool(false));
                        header.insert(
                            "error".into(),
                            serde_json::to_value(e).map_err(|e| {
                                Error::internal(format!("serializing an error failed: {e}"))
                            })?,
                        );
                    }
                }
                r.blobs
            }
            Self::Event(e) => {
                header.extend(e.fields);
                header.insert("event".into(), Value::String(e.event));
                e.blobs
            }
        };
        Ok(Frame { header, blobs })
    }
}
