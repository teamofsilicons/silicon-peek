//! Identifier formats (BLUEPRINT D25), idempotency keys and Ting keys.
//!
//! Local IDs are a prefix plus the 32-hex simple form of a `UUIDv7`, so they sort
//! by creation time: `ask_…`, `snd_…`, `sch_…`, `cmsg_…`, `evt_…`.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::{
    error::{Error, ErrorCode, Result},
    identity::{ActorId, truncate_for_message},
};

macro_rules! prefixed_id {
    ($(#[$doc:meta])* $ty:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $ty(String);

        impl $ty {
            /// The ID prefix.
            pub const PREFIX: &'static str = $prefix;

            /// A new time-ordered ID.
            #[must_use]
            pub fn generate() -> Self {
                Self(format!("{}{}", $prefix, Uuid::now_v7().simple()))
            }

            /// Validates an ID: the prefix, then 32 lowercase hex digits of a
            /// `UUIDv7`.
            ///
            /// # Errors
            /// `invalid_input` naming the expected format.
            pub fn parse(s: &str) -> Result<Self> {
                let valid = s
                    .strip_prefix($prefix)
                    .filter(|rest| {
                        rest.len() == 32
                            && rest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                    .and_then(|rest| Uuid::try_parse(rest).ok())
                    .is_some_and(|u| u.get_version_num() == 7);
                if valid {
                    Ok(Self(s.to_owned()))
                } else {
                    Err(Error::invalid_input(format!(
                        "`{}` is not a valid ID; expected `{}` followed by 32 lowercase hex digits (a UUIDv7)",
                        truncate_for_message(s),
                        $prefix
                    )))
                }
            }

            /// The ID.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $ty {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                Self::parse(s)
            }
        }

        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
            }
        }
    };
}

prefixed_id!(
    /// An ask, created by peekd for every `send --ask`.
    AskId,
    "ask_"
);
prefixed_id!(
    /// A send, created by peekd for every `peek send`.
    SendId,
    "snd_"
);
prefixed_id!(
    /// A scheduled send (`peek send --in/--at`), created by peekd.
    ScheduleId,
    "sch_"
);
prefixed_id!(
    /// A Carbon-initiated message (ctrl+cmd+N, then voice or typing).
    MessageId,
    "cmsg_"
);
prefixed_id!(
    /// A delivery event; also the outbox primary key.
    EventId,
    "evt_"
);

/// An IPC request ID (a hyphenated UUID).
#[must_use]
pub fn request_id() -> String {
    Uuid::now_v7().hyphenated().to_string()
}

/// An `Idempotency-Key`: 16–255 visible ASCII characters.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validates a caller-supplied key (`--idempotency-key`).
    ///
    /// # Errors
    /// `invalid_input` for the wrong length or non-visible characters.
    pub fn parse(s: &str) -> Result<Self> {
        if (16..=255).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic()) {
            Ok(Self(s.to_owned()))
        } else {
            Err(Error::invalid_input(format!(
                "idempotency key must be 16–255 visible ASCII characters (got {} bytes)",
                s.len()
            ))
            .with_hint("reuse the same key only to retry the exact same request"))
        }
    }

    /// A fresh random key (`peek-<uuidv7>`), for one logical mutation.
    #[must_use]
    pub fn generate() -> Self {
        Self(format!("peek-{}", Uuid::now_v7().simple()))
    }

    /// `peek-login-<hex(blake3(slt))>`, so a retried login replays at ACCOUNTS.
    ///
    /// Only for real SLTs, which are single-use: the same SLT is only ever
    /// the same login. Public account IDs are never credentials.
    #[must_use]
    pub fn login(slt: &str) -> Self {
        Self(format!(
            "peek-login-{}",
            blake3::hash(slt.as_bytes()).to_hex()
        ))
    }

    /// `peek-refresh-<hex(blake3(refresh_token))>`: every process holding the
    /// same token derives the same key, so ACCOUNTS replays one rotation to all.
    #[must_use]
    pub fn refresh(refresh_token: &str) -> Self {
        Self(format!(
            "peek-refresh-{}",
            blake3::hash(refresh_token.as_bytes()).to_hex()
        ))
    }

    /// `peek-revoke-<hex(blake3(token))>`.
    #[must_use]
    pub fn revoke(token: &str) -> Self {
        Self(format!(
            "peek-revoke-{}",
            blake3::hash(token.as_bytes()).to_hex()
        ))
    }

    /// `peek-delivery-<event_id>`.
    #[must_use]
    pub fn delivery(event_id: &EventId) -> Self {
        Self(format!("peek-delivery-{event_id}"))
    }

    /// The key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
    }
}

/// The last segment of a Ting key: what happened to the subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TingKeyEvent {
    /// `peek.ask.answered`.
    Answered,
    /// `peek.ask.dismissed`.
    Dismissed,
    /// `peek.ask.expired`.
    Expired,
    /// `peek.message.received`.
    Message,
    /// `peek.speech.finished`.
    SpeechFinished,
    /// `peek.show.dismissed`.
    ShowDismissed,
    /// `peek.send.expired`.
    SendExpired,
    /// `peek.schedule.due`.
    Due,
    /// `peek.send.shown`.
    Shown,
}

impl TingKeyEvent {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::Dismissed => "dismissed",
            Self::Expired => "expired",
            Self::Message => "message",
            Self::SpeechFinished => "speech_finished",
            Self::ShowDismissed => "show_dismissed",
            Self::SendExpired => "send_expired",
            Self::Due => "due",
            Self::Shown => "shown",
        }
    }
}

/// Maximum Ting key length in bytes.
pub const TING_KEY_MAX_BYTES: usize = 200;

/// Builds a Ting key: `"<actor_id>/<subject_id>/<event>"`.
///
/// Keys are unique per semantic event and include the recipient, because Ting
/// keys are shared by the whole account (BLUEPRINT §3.5).
///
/// # Errors
/// `invalid_input` when the subject is empty, contains `/` or control
/// characters, or the key exceeds 200 bytes.
pub fn ting_key(actor: &ActorId, subject_id: &str, event: TingKeyEvent) -> Result<String> {
    if subject_id.is_empty() || subject_id.contains('/') || subject_id.chars().any(char::is_control)
    {
        return Err(Error::invalid_input(format!(
            "Ting key subject `{}` must be a non-empty ID without `/`",
            truncate_for_message(subject_id)
        )));
    }
    let key = format!("{actor}/{subject_id}/{}", event.as_str());
    if key.len() > TING_KEY_MAX_BYTES {
        return Err(Error::new(
            ErrorCode::InvalidInput,
            format!(
                "Ting key is {} bytes; the limit is {TING_KEY_MAX_BYTES}",
                key.len()
            ),
        ));
    }
    Ok(key)
}
