//! Identifier formats (BLUEPRINT D25), idempotency keys and Ting keys.
//!
//! Local IDs are a prefix plus the 32-hex simple form of a `UUIDv7`, so they sort
//! by creation time: `ask_…`, `snd_…`, `cmsg_…`, `evt_…`.

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

    /// `peek-login-<hex(blake3(slt))>`, so a retried login replays at IAM.
    ///
    /// Only for real SLTs, which are single-use: the same SLT is only ever
    /// the same login. See [`IdempotencyKey::login_attempt`] for public IDs.
    #[must_use]
    pub fn login(slt: &str) -> Self {
        Self(format!(
            "peek-login-{}",
            blake3::hash(slt.as_bytes()).to_hex()
        ))
    }

    /// `peek-login-<hex(blake3(slt ‖ nonce))>`: a key unique to one login
    /// attempt. A testing environment accepts the Silicon's public ID as the
    /// SLT, and that "SLT" is the same on every login, so a key derived from
    /// it alone would make IAM replay the first login's (possibly revoked)
    /// token pair for ten minutes and then refuse the login with
    /// `idempotency_response_expired` for a day. The attempt's key is stored
    /// in `pending_login`, so in-process retries and `--recover` still replay
    /// the same exchange.
    #[must_use]
    pub fn login_attempt(slt: &str) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(slt.as_bytes());
        hasher.update(&[0]);
        hasher.update(Uuid::new_v4().as_bytes());
        Self(format!("peek-login-{}", hasher.finalize().to_hex()))
    }

    /// `peek-refresh-<hex(blake3(refresh_token))>`: every process holding the
    /// same token derives the same key, so IAM replays one rotation to all.
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
        }
    }
}

/// Maximum Ting key length in bytes.
pub const TING_KEY_MAX_BYTES: usize = 200;

/// Builds a Ting key: `"<actor_id>/<subject_id>/<event>"`.
///
/// Keys are unique per semantic event and include the recipient, because Ting
/// keys are shared by the whole org (BLUEPRINT §3.5).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_parse_and_sort() -> Result<()> {
        let a = AskId::generate();
        let b = AskId::generate();
        assert!(a.as_str().starts_with("ask_"));
        assert_eq!(a.as_str().len(), 36);
        assert_eq!(AskId::parse(a.as_str())?, a);
        assert!(a <= b);
        assert!(SendId::generate().as_str().starts_with("snd_"));
        assert!(MessageId::generate().as_str().starts_with("cmsg_"));
        assert!(EventId::generate().as_str().starts_with("evt_"));
        Ok(())
    }

    #[test]
    fn malformed_ids_are_rejected() {
        let v4 = Uuid::new_v4().simple().to_string();
        assert!(AskId::parse(&format!("ask_{v4}")).is_err(), "must be v7");
        assert!(AskId::parse(&SendId::generate().to_string()).is_err());
        let upper = AskId::generate()
            .as_str()
            .to_uppercase()
            .replace("ASK_", "ask_");
        assert!(AskId::parse(&upper).is_err());
        assert!(AskId::parse("ask_").is_err());
        assert!(serde_json::from_str::<AskId>("\"ask_1\"").is_err());
    }

    #[test]
    fn idempotency_keys() {
        assert!(IdempotencyKey::parse(&"a".repeat(15)).is_err());
        assert!(IdempotencyKey::parse(&"a".repeat(16)).is_ok());
        assert!(IdempotencyKey::parse(&"a".repeat(255)).is_ok());
        assert!(IdempotencyKey::parse(&"a".repeat(256)).is_err());
        assert!(IdempotencyKey::parse("has a space in it!").is_err());
        let k = IdempotencyKey::refresh("ort_abc");
        assert_eq!(k, IdempotencyKey::refresh("ort_abc"));
        assert_ne!(k, IdempotencyKey::refresh("ort_abd"));
        assert_eq!(k.as_str().len(), "peek-refresh-".len() + 64);
        assert!(IdempotencyKey::parse(k.as_str()).is_ok());
        assert!(
            IdempotencyKey::login("oac_x")
                .as_str()
                .starts_with("peek-login-")
        );
        assert!(
            IdempotencyKey::revoke("ort_x")
                .as_str()
                .starts_with("peek-revoke-")
        );
        let e = EventId::generate();
        assert_eq!(
            IdempotencyKey::delivery(&e).as_str(),
            format!("peek-delivery-{e}")
        );
        assert!(IdempotencyKey::parse(IdempotencyKey::generate().as_str()).is_ok());
    }

    #[test]
    fn ting_keys() -> Result<()> {
        let actor = ActorId::parse("si:cleanup")?;
        let ask = AskId::generate();
        let key = ting_key(&actor, ask.as_str(), TingKeyEvent::Answered)?;
        assert_eq!(key, format!("si:cleanup/{ask}/answered"));
        assert!(ting_key(&actor, "", TingKeyEvent::Message).is_err());
        assert!(ting_key(&actor, "a/b", TingKeyEvent::Message).is_err());
        let long = ActorId::parse(&format!("si:{}", "a".repeat(50)))?;
        assert!(ting_key(&long, &"x".repeat(150), TingKeyEvent::ShowDismissed).is_err());
        Ok(())
    }
}
