//! Identity and addressing types: actors, accounts, data contexts, API origins,
//! store slot keys and screen positions.
//!
//! Formats follow ACCOUNTS 4.0.0 (BLUEPRINT §2.1): Silicons are `si:<handle>`
//! (handle 3–50), Carbons are `c:<handle>` (handle 3–30), accounts are
//! bare handles (3–50); every handle is `[a-z0-9_-]`.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;
use uuid::Uuid;

use crate::error::{Error, Result};

fn handle_ok(handle: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&handle.len())
        && handle
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

macro_rules! string_newtype_serde {
    ($ty:ident) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                $ty::parse(&s).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
            }
        }
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl FromStr for $ty {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                Self::parse(s)
            }
        }
        impl AsRef<str> for $ty {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

/// Whether an actor is a Silicon or a Carbon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// An AI agent (`si:<handle>`).
    Silicon,
    /// A human (`c:<handle>`).
    Carbon,
}

impl ActorType {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Silicon => "silicon",
            Self::Carbon => "carbon",
        }
    }
}

/// A canonical public actor ID: `si:<handle>` or `c:<handle>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActorId(String);

impl ActorId {
    /// Parses and validates a public actor ID.
    ///
    /// # Errors
    /// `invalid_input` when the prefix, length or characters are wrong.
    pub fn parse(s: &str) -> Result<Self> {
        let ok = if let Some(h) = s.strip_prefix("si:") {
            handle_ok(h, 3, 50)
        } else if let Some(h) = s.strip_prefix("c:") {
            handle_ok(h, 3, 30)
        } else {
            false
        };
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(Error::invalid_input(format!(
                "`{}` is not a public actor ID; expected si:<handle> (3–50 of a-z 0-9 _ -) or c:<handle> (3–30)",
                truncate_for_message(s)
            )))
        }
    }

    /// The ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Silicon or Carbon, from the prefix.
    #[must_use]
    pub fn actor_type(&self) -> ActorType {
        if self.0.starts_with("si:") {
            ActorType::Silicon
        } else {
            ActorType::Carbon
        }
    }

    /// The handle without the prefix.
    #[must_use]
    pub fn handle(&self) -> &str {
        self.0.split_once(':').map_or("", |(_, h)| h)
    }

    /// Whether a string looks like a public ID (`si:` or `c:` prefix). The
    /// backend refuses such an SLT in production (`slt_is_public_id`).
    #[must_use]
    pub fn looks_like_public_id(s: &str) -> bool {
        s.starts_with("si:") || s.starts_with("c:")
    }
}
string_newtype_serde!(ActorId);

/// The actor object carried in sessions and API responses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    /// Silicon or Carbon.
    #[serde(rename = "type")]
    pub actor_type: ActorType,
    /// The canonical public ID.
    pub public_id: ActorId,
}

/// The immutable ACCOUNTS account UUID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(String);
impl AccountId {
    /// Parse a non-nil account UUID.
    pub fn parse(s: &str) -> Result<Self> {
        Uuid::parse_str(s)
            .ok()
            .filter(|id| !id.is_nil())
            .map(|id| Self(id.to_string()))
            .ok_or_else(|| Error::invalid_input("account_id must be a ACCOUNTS UUID"))
    }
    /// The canonical UUID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
string_newtype_serde!(AccountId);

/// Peek has one live data context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Context {
    /// Live account data.
    Production,
}
impl Context {
    /// Read the live data context; retired test environments cannot authenticate.
    pub fn parse(value: &str) -> Result<Self> {
        if value == "production" {
            Ok(Self::Production)
        } else {
            Err(Error::invalid_input(
                "Only the production context is supported",
            ))
        }
    }
    /// Wire value.
    pub fn as_string(&self) -> String {
        "production".to_owned()
    }
    /// Context carried with Ting events.
    pub fn data_context(&self) -> DataContext {
        DataContext::Production
    }
}
impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("production")
    }
}

impl FromStr for Context {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl Serialize for Context {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Context {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
    }
}

/// The coarse context recorded in Ting payloads: `production` or `testing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataContext {
    /// Production.
    Production,
}

/// The production peek-server origin.
pub const DEFAULT_API_URL: &str = "https://backend.peek.teamofsilicons.com";

/// A validated peek-server origin: HTTPS, or HTTP only for loopback hosts; no
/// credentials, path, query or fragment. Stored without a trailing slash.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ApiUrl(String);

impl ApiUrl {
    /// Parses and normalizes an origin.
    ///
    /// # Errors
    /// `invalid_input` explaining which rule the URL breaks.
    pub fn parse(s: &str) -> Result<Self> {
        let hint = "pass an origin such as https://backend.peek.teamofsilicons.com (http:// is allowed only for localhost, 127.0.0.1 or [::1])";
        let url = Url::parse(s).map_err(|e| {
            Error::invalid_input(format!(
                "API URL `{}` is not a URL: {e}",
                truncate_for_message(s)
            ))
            .with_hint(hint)
        })?;
        let loopback = match url.host() {
            Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        let problem = if url.host_str().is_none() {
            Some("has no host")
        } else if !url.username().is_empty() || url.password().is_some() {
            Some("must not contain credentials")
        } else if url.query().is_some() || url.fragment().is_some() {
            Some("must not contain a query or fragment")
        } else if !matches!(url.path(), "" | "/") {
            Some("must be an origin without a path")
        } else if url.port() == Some(0) {
            Some("must not use port 0")
        } else if url.scheme() == "http" && !loopback {
            Some("must use https (http is allowed only for loopback hosts)")
        } else if !matches!(url.scheme(), "http" | "https") {
            Some("must use https")
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(Error::invalid_input(format!(
                "API URL `{}` {problem}",
                truncate_for_message(s)
            ))
            .with_hint(hint));
        }
        Ok(Self(url.as_str().trim_end_matches('/').to_owned()))
    }

    /// The production origin.
    #[must_use]
    pub fn production() -> Self {
        Self(DEFAULT_API_URL.to_owned())
    }

    /// The origin, without a trailing slash.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Joins an absolute path (which must start with `/`).
    #[must_use]
    pub fn join(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }
}
string_newtype_serde!(ApiUrl);

/// The key of one session slot in `session.json`: `"<api_url>#<context>"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotKey {
    api_url: ApiUrl,
    context: Context,
}

impl SlotKey {
    /// Builds a slot key.
    #[must_use]
    pub fn new(api_url: ApiUrl, context: Context) -> Self {
        Self { api_url, context }
    }

    /// Parses `"<api_url>#<context>"`.
    ///
    /// # Errors
    /// `invalid_input` when either half is invalid.
    pub fn parse(s: &str) -> Result<Self> {
        let (url, context) = s.rsplit_once('#').ok_or_else(|| {
            Error::invalid_input(format!(
                "`{}` is not a session slot key; expected <api_url>#<production|env-uuid>",
                truncate_for_message(s)
            ))
        })?;
        Ok(Self::new(ApiUrl::parse(url)?, Context::parse(context)?))
    }

    /// The API origin.
    #[must_use]
    pub fn api_url(&self) -> &ApiUrl {
        &self.api_url
    }

    /// The data context.
    #[must_use]
    pub fn context(&self) -> Context {
        self.context
    }

    /// The canonical string form.
    #[must_use]
    pub fn as_string(&self) -> String {
        format!("{}#{}", self.api_url, self.context)
    }
}

impl fmt::Display for SlotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.api_url, self.context)
    }
}

impl Serialize for SlotKey {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SlotKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
    }
}

/// The default modifier of the per-position Carbon hotkeys (`ctrl+cmd+1…8`).
/// `cmd+1…8` collides with tab switching in browsers and editors, so peek
/// defaults to `ctrl+cmd`; the modifier stays configurable in Peek.app
/// (setting `hotkey_modifier`).
pub const DEFAULT_HOTKEY_MODIFIER: &str = "ctrl+cmd";

/// One of the eight screen positions, 1 (top centre) clockwise to 8 (top left).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotIndex(u8);

impl SlotIndex {
    /// Every position, in order.
    pub const ALL: [SlotIndex; 8] = [
        SlotIndex(1),
        SlotIndex(2),
        SlotIndex(3),
        SlotIndex(4),
        SlotIndex(5),
        SlotIndex(6),
        SlotIndex(7),
        SlotIndex(8),
    ];

    /// Validates a position number.
    ///
    /// # Errors
    /// `invalid_input` outside 1..=8.
    pub fn new(index: u64) -> Result<Self> {
        match u8::try_from(index) {
            Ok(i @ 1..=8) => Ok(Self(i)),
            _ => Err(Error::invalid_input(format!(
                "position {index} does not exist; positions are 1 (top centre) to 8, clockwise"
            ))
            .with_hint("peek register side <1-8>")),
        }
    }

    /// The number, 1..=8.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// The side name used by drawings (`input.slot.side`).
    #[must_use]
    pub const fn side(self) -> Side {
        match self.0 {
            1 => Side::Top,
            2 => Side::TopRight,
            3 => Side::Right,
            4 => Side::BottomRight,
            5 => Side::Bottom,
            6 => Side::BottomLeft,
            7 => Side::Left,
            _ => Side::TopLeft,
        }
    }

    /// The default Carbon hotkey for this position, e.g. `ctrl+cmd+5`
    /// ([`DEFAULT_HOTKEY_MODIFIER`] plus the position).
    #[must_use]
    pub fn default_hotkey(self) -> String {
        self.hotkey(DEFAULT_HOTKEY_MODIFIER)
    }

    /// The Carbon hotkey for this position with `modifier` (the
    /// `hotkey_modifier` setting, e.g. `opt+cmd`): `<modifier>+<position>`.
    #[must_use]
    pub fn hotkey(self, modifier: &str) -> String {
        format!("{modifier}+{}", self.0)
    }
}

impl fmt::Display for SlotIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for SlotIndex {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_u8(self.0)
    }
}

impl<'de> Deserialize<'de> for SlotIndex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let n = u64::deserialize(d)?;
        Self::new(n).map_err(|e| serde::de::Error::custom(e.message().to_owned()))
    }
}

/// A position's side, matching visual.md A4 `input.slot.side`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    /// Position 1.
    Top,
    /// Position 2.
    TopRight,
    /// Position 3.
    Right,
    /// Position 4.
    BottomRight,
    /// Position 5.
    Bottom,
    /// Position 6.
    BottomLeft,
    /// Position 7.
    Left,
    /// Position 8.
    TopLeft,
}

/// A slot and its side, as printed by `register side` and `status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotInfo {
    /// 1..=8.
    pub index: SlotIndex,
    /// The side name.
    pub side: Side,
}

impl From<SlotIndex> for SlotInfo {
    fn from(index: SlotIndex) -> Self {
        Self {
            index,
            side: index.side(),
        }
    }
}

pub(crate) fn truncate_for_message(s: &str) -> String {
    const MAX: usize = 80;
    let mut out: String = s.chars().filter(|c| !c.is_control()).take(MAX).collect();
    if s.chars().count() > MAX {
        out.push('…');
    }
    out
}
